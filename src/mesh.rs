//! Mesh decoding: vertex channels, index buffer and sub-meshes of a Unity `Mesh`.
//!
//! Vertex data is split into streams. Stream `s` starts where the previous one ends, rounded up
//! to 16 bytes, and holds `vertex_count` records of `stride` bytes, where the stride is the sum
//! of its channels' sizes. A channel says which stream it lives in, its byte offset inside the
//! record, a component format and a component count (the low four bits of `dimension`). Channel
//! numbers are fixed by Unity 2019: 0 position, 1 normal, 2 tangent, 3 colour, 4 and 5 the first
//! two UV sets, 12 blend weights, 13 blend indices.

use anyhow::{bail, ensure, Result};

use crate::bundle::Bundle;
use crate::unity::{ChannelInfo, Mesh};

pub const CH_POSITION: usize = 0;
pub const CH_NORMAL: usize = 1;
pub const CH_TANGENT: usize = 2;
pub const CH_COLOR: usize = 3;
pub const CH_UV0: usize = 4;
pub const CH_UV1: usize = 5;
pub const CH_BLEND_WEIGHT: usize = 12;
pub const CH_BLEND_INDEX: usize = 13;

/// Everything decoded from one `Mesh`, still in Unity's coordinate system.
#[derive(Debug, Default)]
pub struct MeshData {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 4]>,
    pub colors: Vec<[f32; 4]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    /// Up to four bone indices per vertex (indices into the renderer's bone list).
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// Triangle lists, one per sub-mesh, with indices into the vertex arrays.
    pub submeshes: Vec<Vec<u32>>,
    /// Row-major 4x4 matrices as stored (`rows[r][c]`).
    pub bind_poses: Vec<[[f32; 4]; 4]>,
}

/// Bytes per component of a vertex format (Unity 2019's `VertexFormat`).
fn format_size(format: u8) -> Result<usize> {
    Ok(match format {
        0 | 10 | 11 => 4,
        1 | 4 | 5 | 8 | 9 => 2,
        2 | 3 | 6 | 7 => 1,
        _ => bail!("unknown vertex format {format}"),
    })
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let man = (h & 0x3FF) as f32;
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 => {
            if man == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// One component of a channel as a float (normalised formats scaled to their range).
fn component(data: &[u8], format: u8) -> f32 {
    match format {
        0 => f32::from_le_bytes(data[..4].try_into().unwrap()),
        1 => half_to_f32(u16::from_le_bytes(data[..2].try_into().unwrap())),
        2 => data[0] as f32 / 255.0,
        3 => (data[0] as i8 as f32 / 127.0).max(-1.0),
        4 => u16::from_le_bytes(data[..2].try_into().unwrap()) as f32 / 65535.0,
        5 => (i16::from_le_bytes(data[..2].try_into().unwrap()) as f32 / 32767.0).max(-1.0),
        6 => data[0] as f32,
        7 => data[0] as i8 as f32,
        8 => u16::from_le_bytes(data[..2].try_into().unwrap()) as f32,
        9 => i16::from_le_bytes(data[..2].try_into().unwrap()) as f32,
        10 => u32::from_le_bytes(data[..4].try_into().unwrap()) as f32,
        11 => i32::from_le_bytes(data[..4].try_into().unwrap()) as f32,
        _ => 0.0,
    }
}

/// One component of a channel as an integer (for bone indices).
fn component_int(data: &[u8], format: u8) -> i64 {
    match format {
        6 => data[0] as i64,
        7 => data[0] as i8 as i64,
        8 => u16::from_le_bytes(data[..2].try_into().unwrap()) as i64,
        9 => i16::from_le_bytes(data[..2].try_into().unwrap()) as i64,
        10 => u32::from_le_bytes(data[..4].try_into().unwrap()) as i64,
        11 => i32::from_le_bytes(data[..4].try_into().unwrap()) as i64,
        _ => component(data, format) as i64,
    }
}

struct Layout {
    /// (offset of the stream in the buffer, stride)
    streams: Vec<(usize, usize)>,
    channels: Vec<ChannelInfo>,
    vertex_count: usize,
}

impl Layout {
    fn new(channels: &[ChannelInfo], vertex_count: usize) -> Result<Layout> {
        let channels: Vec<ChannelInfo> = channels
            .iter()
            .map(|c| ChannelInfo {
                dimension: c.dimension & 0x0F,
                ..*c
            })
            .collect();
        let stream_count = channels
            .iter()
            .filter(|c| c.dimension > 0)
            .map(|c| c.stream as usize + 1)
            .max()
            .unwrap_or(0);
        let mut streams = Vec::new();
        let mut offset = 0usize;
        for s in 0..stream_count {
            let mut stride = 0;
            for c in channels
                .iter()
                .filter(|c| c.stream as usize == s && c.dimension > 0)
            {
                stride += c.dimension as usize * format_size(c.format)?;
            }
            streams.push((offset, stride));
            offset += vertex_count * stride;
            offset = (offset + 15) & !15;
        }
        Ok(Layout {
            streams,
            channels,
            vertex_count,
        })
    }

    /// Raw bytes of channel `ch` for vertex `v`, or `None` when the channel is absent.
    fn record<'a>(&self, data: &'a [u8], ch: usize, v: usize) -> Option<(&'a [u8], ChannelInfo)> {
        let c = *self.channels.get(ch)?;
        if c.dimension == 0 {
            return None;
        }
        let (base, stride) = self.streams[c.stream as usize];
        let start = base + v * stride + c.offset as usize;
        let len = c.dimension as usize * format_size(c.format).ok()?;
        data.get(start..start + len).map(|d| (d, c))
    }

    fn floats(&self, data: &[u8], ch: usize) -> Option<Vec<[f32; 4]>> {
        let c = *self.channels.get(ch)?;
        if c.dimension == 0 {
            return None;
        }
        let size = format_size(c.format).ok()?;
        let mut out = Vec::with_capacity(self.vertex_count);
        for v in 0..self.vertex_count {
            let (rec, _) = self.record(data, ch, v)?;
            let mut e = [0.0f32; 4];
            for (k, slot) in e.iter_mut().enumerate().take(c.dimension as usize) {
                *slot = component(&rec[k * size..], c.format);
            }
            out.push(e);
        }
        Some(out)
    }
}

/// Decodes a mesh. Streamed vertex data (`m_StreamData`) is read from the bundle's `.resS`.
pub fn decode(bundle: &Bundle, mesh: &Mesh) -> Result<MeshData> {
    ensure!(
        mesh.m_MeshCompression == 0,
        "mesh {:?} uses mesh compression {}",
        mesh.m_Name,
        mesh.m_MeshCompression
    );
    let vd = &mesh.m_VertexData;
    let n = vd.m_VertexCount as usize;
    let streamed;
    let data: &[u8] = if vd.m_DataSize.is_empty() && mesh.m_StreamData.size > 0 {
        streamed = bundle.read_stream(&mesh.m_StreamData)?;
        &streamed
    } else {
        &vd.m_DataSize
    };
    let layout = Layout::new(&vd.m_Channels, n)?;
    for (s, &(off, stride)) in layout.streams.iter().enumerate() {
        ensure!(
            off + n * stride <= data.len(),
            "mesh {:?}: stream {s} reaches past the vertex data ({} bytes)",
            mesh.m_Name,
            data.len()
        );
    }
    let mut out = MeshData {
        name: mesh.m_Name.clone(),
        ..Default::default()
    };
    if let Some(v) = layout.floats(data, CH_POSITION) {
        out.positions = v.iter().map(|e| [e[0], e[1], e[2]]).collect();
    }
    if let Some(v) = layout.floats(data, CH_NORMAL) {
        out.normals = v.iter().map(|e| [e[0], e[1], e[2]]).collect();
    }
    if let Some(v) = layout.floats(data, CH_TANGENT) {
        out.tangents = v;
    }
    if let Some(v) = layout.floats(data, CH_COLOR) {
        out.colors = v;
    }
    if let Some(v) = layout.floats(data, CH_UV0) {
        out.uv0 = v.iter().map(|e| [e[0], e[1]]).collect();
    }
    if let Some(v) = layout.floats(data, CH_UV1) {
        out.uv1 = v.iter().map(|e| [e[0], e[1]]).collect();
    }

    // Skinning: indices are always present for a skinned mesh, weights only when a vertex has
    // more than one influence (a single index means the bone has weight 1).
    let weights = layout.floats(data, CH_BLEND_WEIGHT);
    if layout
        .channels
        .get(CH_BLEND_INDEX)
        .is_some_and(|c| c.dimension > 0)
    {
        let c = layout.channels[CH_BLEND_INDEX];
        let size = format_size(c.format)?;
        ensure!(c.dimension <= 4, "more than four bone influences");
        for v in 0..n {
            let (rec, _) = layout.record(data, CH_BLEND_INDEX, v).unwrap();
            let mut j = [0u16; 4];
            for (k, slot) in j.iter_mut().enumerate().take(c.dimension as usize) {
                let idx = component_int(&rec[k * size..], c.format);
                ensure!(
                    (0..=u16::MAX as i64).contains(&idx),
                    "bone index {idx} out of range"
                );
                *slot = idx as u16;
            }
            out.joints.push(j);
            out.weights.push(match &weights {
                Some(w) => w[v],
                None => [1.0, 0.0, 0.0, 0.0],
            });
        }
    }

    // Indices
    let wide = match mesh.m_IndexFormat {
        0 => false,
        1 => true,
        f => bail!("mesh {:?}: index format {f}", mesh.m_Name),
    };
    let ib = &mesh.m_IndexBuffer;
    for (i, sm) in mesh.m_SubMeshes.iter().enumerate() {
        ensure!(
            sm.topology == 0,
            "mesh {:?}: sub-mesh {i} has topology {} (only triangles are read)",
            mesh.m_Name,
            sm.topology
        );
        ensure!(
            sm.indexCount % 3 == 0,
            "mesh {:?}: partial triangle",
            mesh.m_Name
        );
        let w = if wide { 4 } else { 2 };
        let start = sm.firstByte as usize;
        let end = start + sm.indexCount as usize * w;
        ensure!(
            end <= ib.len(),
            "mesh {:?}: sub-mesh {i} outside the index buffer",
            mesh.m_Name
        );
        let mut idx = Vec::with_capacity(sm.indexCount as usize);
        for chunk in ib[start..end].chunks_exact(w) {
            let raw = if wide {
                u32::from_le_bytes(chunk.try_into().unwrap())
            } else {
                u16::from_le_bytes(chunk.try_into().unwrap()) as u32
            };
            let v = raw + sm.baseVertex;
            ensure!(
                (v as usize) < n,
                "mesh {:?}: index {v} past {n} vertices",
                mesh.m_Name
            );
            idx.push(v);
        }
        out.submeshes.push(idx);
    }
    out.bind_poses = mesh.m_BindPose.iter().map(|m| m.rows()).collect();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x3555), 0.333251953125);
    }

    #[test]
    fn stream_layout_aligns_to_16() {
        // Stream 0: position (3 floats = 12 bytes) for 3 vertices = 36 bytes, next stream at 48
        let ch = |stream, offset, format, dimension| ChannelInfo {
            stream,
            offset,
            format,
            dimension,
        };
        let mut channels = vec![ch(0, 0, 0, 3)];
        channels.resize(4, ch(0, 0, 0, 0));
        channels.push(ch(1, 0, 0, 2));
        let l = Layout::new(&channels, 3).unwrap();
        assert_eq!(l.streams, [(0, 12), (48, 8)]);
    }

    #[test]
    fn channel_reads_normalised_and_wide_formats() {
        assert_eq!(component(&[255], 2), 1.0);
        assert_eq!(component(&[0x80], 3), -1.0);
        assert_eq!(component(&[0xFF, 0xFF], 4), 1.0);
        assert_eq!(component_int(&[5, 0, 0, 0], 10), 5);
    }
}

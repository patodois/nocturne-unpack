//! Mesh decoding: vertex channels, index buffer and sub-meshes of a Unity `Mesh`.
//!
//! Vertex data is split into streams. Stream `s` starts where the previous one ends, rounded up
//! to 16 bytes, and holds `vertex_count` records of `stride` bytes, where the stride is the sum
//! of its channels' sizes. A channel says which stream it lives in, its byte offset inside the
//! record, a component format and a component count (the low four bits of `dimension`). Channel
//! numbers are fixed by Unity 2019: 0 position, 1 normal, 2 tangent, 3 colour, 4 and 5 the first
//! two UV sets, 12 blend weights, 13 blend indices.

use anyhow::{anyhow, bail, ensure, Result};

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
    /// Blend shape channels the mesh has (their data is not read)
    pub blend_shapes: usize,
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
        // A record is `stride` bytes, so a channel that sticks out of it would read into the
        // next vertex, or past the end of the stream for the last one
        for (i, c) in channels.iter().enumerate().filter(|(_, c)| c.dimension > 0) {
            let end = c.offset as usize + c.dimension as usize * format_size(c.format)?;
            ensure!(
                end <= streams[c.stream as usize].1,
                "vertex channel {i} ends at byte {end} of a {} byte record of stream {}",
                streams[c.stream as usize].1,
                c.stream
            );
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
    if mesh.m_VertexData.m_DataSize.is_empty() && mesh.m_StreamData.size > 0 {
        let streamed = bundle.read_stream(&mesh.m_StreamData)?;
        decode_with(mesh, &streamed)
    } else {
        decode_with(mesh, &mesh.m_VertexData.m_DataSize)
    }
}

/// Decodes a mesh whose vertex buffer is `data` (inline, or read from a stream).
pub fn decode_with(mesh: &Mesh, data: &[u8]) -> Result<MeshData> {
    ensure!(
        mesh.m_MeshCompression == 0,
        "mesh {:?} uses mesh compression {}",
        mesh.m_Name,
        mesh.m_MeshCompression
    );
    let vd = &mesh.m_VertexData;
    let n = vd.m_VertexCount as usize;
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
            let (rec, _) = layout
                .record(data, CH_BLEND_INDEX, v)
                .ok_or_else(|| anyhow!("blend index of vertex {v} lies outside the vertex data"))?;
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
            let v = raw.checked_add(sm.baseVertex).ok_or_else(|| {
                anyhow!(
                    "mesh {:?}: index {raw} plus base vertex {} does not fit 32 bits",
                    mesh.m_Name,
                    sm.baseVertex
                )
            })?;
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
    out.blend_shapes = mesh.m_Shapes.channels.len();
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
        assert!((half_to_f32(0x3555) - 1.0 / 3.0).abs() < 1e-3);
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

    fn synthetic_mesh() -> (Mesh, Vec<u8>) {
        use crate::unity::{SubMesh, VertexData};
        let ch = |stream, offset, format, dimension| ChannelInfo {
            stream,
            offset,
            format,
            dimension,
        };
        // Stream 0: position (3 floats) and normal (3 floats) = 24 bytes per vertex
        // Stream 1: UV0 as two halves = 4 bytes per vertex
        // Stream 2: weights (2 floats) and bone indices (2 x u32) = 16 bytes per vertex
        let mut channels = vec![ch(0, 0, 0, 3), ch(0, 12, 0, 3)];
        channels.resize(4, ch(0, 0, 0, 0));
        channels.push(ch(1, 0, 1, 2));
        channels.resize(12, ch(0, 0, 0, 0));
        channels.push(ch(2, 0, 0, 2));
        channels.push(ch(2, 8, 10, 2));
        let mut data = Vec::new();
        let f = |v: f32| v.to_le_bytes();
        for v in 0..3u32 {
            for x in [v as f32, 1.0, 2.0, 0.0, 0.0, 1.0] {
                data.extend_from_slice(&f(x));
            }
        }
        while data.len() % 16 != 0 {
            data.push(0);
        }
        for _ in 0..3 {
            data.extend_from_slice(&0x3C00u16.to_le_bytes()); // u = 1.0
            data.extend_from_slice(&0x3800u16.to_le_bytes()); // v = 0.5
        }
        while data.len() % 16 != 0 {
            data.push(0);
        }
        for v in 0..3u32 {
            data.extend_from_slice(&f(0.25));
            data.extend_from_slice(&f(0.75));
            data.extend_from_slice(&v.to_le_bytes());
            data.extend_from_slice(&(v + 1).to_le_bytes());
        }
        let mesh = Mesh {
            m_Name: "synthetic".into(),
            m_SubMeshes: vec![SubMesh {
                firstByte: 0,
                indexCount: 3,
                topology: 0,
                baseVertex: 0,
                firstVertex: 0,
                vertexCount: 3,
            }],
            m_Shapes: Default::default(),
            m_BindPose: vec![],
            m_BoneNameHashes: vec![],
            m_RootBoneNameHash: 0,
            m_MeshCompression: 0,
            m_IndexFormat: 0,
            m_IndexBuffer: [0u16, 2, 1].iter().flat_map(|i| i.to_le_bytes()).collect(),
            m_VertexData: VertexData {
                m_VertexCount: 3,
                m_Channels: channels,
                m_DataSize: vec![],
            },
            m_CompressedMesh: Default::default(),
            m_StreamData: Default::default(),
        };
        (mesh, data)
    }

    #[test]
    fn decodes_streams_formats_and_skin() {
        let (mesh, data) = synthetic_mesh();
        let d = decode_with(&mesh, &data).unwrap();
        assert_eq!(
            d.positions,
            [[0.0, 1.0, 2.0], [1.0, 1.0, 2.0], [2.0, 1.0, 2.0]]
        );
        assert_eq!(d.normals[2], [0.0, 0.0, 1.0]);
        assert_eq!(d.uv0, [[1.0, 0.5]; 3]);
        assert_eq!(d.joints[2], [2, 3, 0, 0]);
        assert_eq!(d.weights[1], [0.25, 0.75, 0.0, 0.0]);
        assert_eq!(d.submeshes, [vec![0, 2, 1]]);
    }

    #[test]
    fn short_vertex_data_and_bad_indices_are_errors() {
        let (mesh, data) = synthetic_mesh();
        assert!(decode_with(&mesh, &data[..data.len() - 16]).is_err());
        let (mut mesh, data) = synthetic_mesh();
        mesh.m_IndexBuffer = [0u16, 9, 1].iter().flat_map(|i| i.to_le_bytes()).collect();
        let e = decode_with(&mesh, &data).unwrap_err().to_string();
        assert!(e.contains("index 9"), "{e}");
        let (mut mesh, data) = synthetic_mesh();
        mesh.m_MeshCompression = 1;
        assert!(decode_with(&mesh, &data).is_err());
    }

    #[test]
    fn a_channel_that_sticks_out_of_its_record_is_an_error() {
        // The bone indices (2 x u32 = 8 bytes) at byte 12 of a 16 byte record end at 20: the last
        // vertex would read past the data
        let (mut mesh, data) = synthetic_mesh();
        mesh.m_VertexData.m_Channels[CH_BLEND_INDEX].offset = 12;
        let e = decode_with(&mesh, &data).unwrap_err().to_string();
        assert!(e.contains("channel 13"), "{e}");
        // Another channel as well, and the layout itself
        let (mut mesh, data) = synthetic_mesh();
        mesh.m_VertexData.m_Channels[CH_NORMAL].offset = 20;
        assert!(decode_with(&mesh, &data).is_err());
    }

    #[test]
    fn bone_indices_that_stick_out_of_the_last_record_are_an_error_not_a_panic() {
        // One stream of 14 byte records: a position (12 bytes) and two u8 bone indices at byte
        // 13, which end at 15. The bytes every earlier vertex reads are in range (and zero), so
        // only the last vertex runs past the 42 bytes of data
        let ch = |stream, offset, format, dimension| ChannelInfo {
            stream,
            offset,
            format,
            dimension,
        };
        let mut channels = vec![ch(0, 0, 0, 3)];
        channels.resize(13, ch(0, 0, 0, 0));
        channels.push(ch(0, 13, 6, 2));
        let (mut mesh, _) = synthetic_mesh();
        mesh.m_VertexData.m_Channels = channels;
        mesh.m_IndexBuffer = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        let e = decode_with(&mesh, &[0u8; 42]).unwrap_err().to_string();
        assert!(e.contains("channel 13"), "{e}");
    }

    #[test]
    fn index_plus_base_vertex_must_fit_32_bits() {
        let (mut mesh, data) = synthetic_mesh();
        mesh.m_SubMeshes[0].baseVertex = u32::MAX;
        mesh.m_IndexBuffer = [2u16, 2, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
        // 2 + 0xFFFF_FFFF wraps to 1 in release builds, which is a valid vertex (and panics in
        // debug builds)
        let e = decode_with(&mesh, &data).unwrap_err().to_string();
        assert!(e.contains("base vertex"), "{e}");
    }

    #[test]
    fn channel_reads_normalised_and_wide_formats() {
        assert_eq!(component(&[255], 2), 1.0);
        assert_eq!(component(&[0x80], 3), -1.0);
        assert_eq!(component(&[0xFF, 0xFF], 4), 1.0);
        assert_eq!(component_int(&[5, 0, 0, 0], 10), 5);
    }
}

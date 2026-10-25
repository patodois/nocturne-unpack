//! Texture2D decoding to 8-bit RGBA.
//!
//! The largest mip level comes first in the image data, which is either inline (`image data`) or
//! a range of the bundle's `.resS` (`m_StreamData`). Unity stores rows bottom to top; the
//! decoder returns them top to bottom, the order PNG and glTF use.
//!
//! Formats that appear in the game's bundles: Alpha8 (1), RGB24 (3), RGBA32 (4), DXT1 (10),
//! DXT5 (12) and BC7 (25).

use anyhow::{bail, ensure, Result};

use crate::bundle::Bundle;
use crate::unity::Texture2D;

pub const FMT_ALPHA8: i32 = 1;
pub const FMT_RGB24: i32 = 3;
pub const FMT_RGBA32: i32 = 4;
pub const FMT_DXT1: i32 = 10;
pub const FMT_DXT5: i32 = 12;
pub const FMT_BC7: i32 = 25;

#[derive(Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// Rows top to bottom, 4 bytes per pixel.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

pub fn format_name(format: i32) -> &'static str {
    match format {
        FMT_ALPHA8 => "Alpha8",
        FMT_RGB24 => "RGB24",
        FMT_RGBA32 => "RGBA32",
        FMT_DXT1 => "DXT1",
        FMT_DXT5 => "DXT5",
        FMT_BC7 => "BC7",
        _ => "other",
    }
}

/// Bytes of the largest mip level, or `None` for a format this module cannot decode.
pub fn top_level_size(format: i32, w: usize, h: usize) -> Option<usize> {
    let blocks = w.div_ceil(4) * h.div_ceil(4);
    Some(match format {
        FMT_ALPHA8 => w * h,
        FMT_RGB24 => w * h * 3,
        FMT_RGBA32 => w * h * 4,
        FMT_DXT1 => blocks * 8,
        FMT_DXT5 | FMT_BC7 => blocks * 16,
        _ => return None,
    })
}

/// Decodes the largest level of a texture.
pub fn decode(bundle: &Bundle, tex: &Texture2D) -> Result<Image> {
    ensure!(
        tex.m_Width > 0 && tex.m_Height > 0,
        "texture {:?} is empty",
        tex.m_Name
    );
    ensure!(
        tex.m_TextureDimension <= 2,
        "texture {:?} has dimension {}",
        tex.m_Name,
        tex.m_TextureDimension
    );
    let (w, h) = (tex.m_Width as usize, tex.m_Height as usize);
    let streamed;
    let data: &[u8] = if tex.image_data.is_empty() && tex.m_StreamData.size > 0 {
        streamed = bundle.read_stream(&tex.m_StreamData)?;
        &streamed
    } else {
        &tex.image_data
    };
    decode_pixels(tex.m_TextureFormat, w, h, data)
        .map_err(|e| anyhow::anyhow!("texture {:?}: {e}", tex.m_Name))
}

/// Decodes the first level of raw pixel data in one of the supported formats.
pub fn decode_pixels(format: i32, w: usize, h: usize, data: &[u8]) -> Result<Image> {
    let Some(need) = top_level_size(format, w, h) else {
        bail!("format {format} is not supported");
    };
    ensure!(
        data.len() >= need,
        "{} bytes for a {w}x{h} {} level that needs {need}",
        data.len(),
        format_name(format)
    );
    let data = &data[..need];
    // Rows are written bottom to top: Unity's first row is the bottom one
    let mut rgba = vec![0u8; w * h * 4];
    let stride = w * 4;
    let mut put = |y: usize, row: &mut dyn Iterator<Item = [u8; 4]>| {
        let at = (h - 1 - y) * stride;
        for (x, px) in row.enumerate() {
            rgba[at + x * 4..at + x * 4 + 4].copy_from_slice(&px);
        }
    };
    match format {
        FMT_ALPHA8 => {
            for y in 0..h {
                put(
                    y,
                    &mut data[y * w..(y + 1) * w].iter().map(|&a| [255, 255, 255, a]),
                );
            }
        }
        FMT_RGB24 => {
            for y in 0..h {
                let row = &data[y * w * 3..(y + 1) * w * 3];
                put(
                    y,
                    &mut row
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|p| [p[0], p[1], p[2], 255]),
                );
            }
        }
        FMT_RGBA32 => {
            for y in 0..h {
                let row = &data[y * stride..(y + 1) * stride];
                put(y, &mut row.as_chunks::<4>().0.iter().copied());
            }
        }
        _ => {
            let (bc, block_bytes) = match format {
                FMT_DXT1 => (bcn_decode::Format::Bc1, 8),
                FMT_DXT5 => (bcn_decode::Format::Bc3, 16),
                _ => (bcn_decode::Format::Bc7, 16),
            };
            // The decoder works on bands of block rows (blocks are stored row by row), so the
            // scratch buffer stays small for big textures
            let row_bytes = w.div_ceil(4) * block_bytes;
            let block_rows = h.div_ceil(4);
            const BAND: usize = 16;
            let mut band = vec![0u8; stride * BAND * 4];
            let mut r0 = 0;
            while r0 < block_rows {
                let rows = BAND.min(block_rows - r0);
                let y0 = r0 * 4;
                let band_h = (rows * 4).min(h - y0);
                let src = &data[r0 * row_bytes..(r0 + rows) * row_bytes];
                let out = &mut band[..stride * band_h];
                bcn_decode::decode_rgba8(bc, src, w, band_h, out)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                for (i, row) in out.chunks_exact(stride).enumerate() {
                    put(y0 + i, &mut row.as_chunks::<4>().0.iter().copied());
                }
                r0 += rows;
            }
        }
    }
    Ok(Image {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

/// Writes an image as PNG.
pub fn write_png(path: &std::path::Path, img: &Image) -> Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img.rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb24_is_flipped_and_opaque() {
        // 1x2: bottom row red, top row green (Unity stores the bottom row first)
        let img = decode_pixels(FMT_RGB24, 1, 2, &[255, 0, 0, 0, 255, 0]).unwrap();
        assert_eq!(img.rgba, [0, 255, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn dxt1_solid_block() {
        // One block, both colours pure red (RGB565 0xF800), all indices 0
        let block = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let img = decode_pixels(FMT_DXT1, 4, 4, &block).unwrap();
        assert_eq!(img.rgba.len(), 64);
        assert_eq!(&img.rgba[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn dxt1_bands_keep_their_order() {
        // 17 block rows (more than one band of 16): block row i is solid red (i + 1) / 32
        let mut data = Vec::new();
        for i in 0..17u16 {
            let c = (i + 1) << 11;
            data.extend_from_slice(&c.to_le_bytes());
            data.extend_from_slice(&c.to_le_bytes());
            data.extend_from_slice(&[0; 4]);
        }
        let img = decode_pixels(FMT_DXT1, 4, 68, &data).unwrap();
        // The first row of the data is the bottom of the image
        let red = |y: usize| img.rgba[y * 16];
        let want = |i: u16| (((i + 1) as u32 * 255 + 15) / 31) as u8;
        assert!(red(67).abs_diff(want(0)) <= 1);
        assert!(red(0).abs_diff(want(16)) <= 1);
        assert!(red(67 - 4 * 16).abs_diff(want(16)) <= 1);
    }

    #[test]
    fn dxt5_and_bc7_crop_odd_sizes_and_flip_rows() {
        // 5x6 = 2x2 blocks, each solid with a different red (5-bit values 8, 16, 24 and 31)
        let reds = [8u16, 16, 24, 31];
        let mut data = Vec::new();
        for r in reds {
            let c = r << 11;
            data.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0, 0, 0, 0]); // alpha 255, 255
            data.extend_from_slice(&c.to_le_bytes());
            data.extend_from_slice(&c.to_le_bytes());
            data.extend_from_slice(&[0; 4]);
        }
        let img = decode_pixels(FMT_DXT5, 5, 6, &data).unwrap();
        assert_eq!(img.rgba.len(), 5 * 6 * 4);
        let red = |x: usize, y: usize| img.rgba[(y * 5 + x) * 4];
        // The first data row is the bottom of the image: block row 0 (blocks 0 and 1) covers the
        // image rows 5 to 2, block row 1 (blocks 2 and 3) only rows 1 and 0 (cropped from 4 to 2)
        let widen = |v: u16| ((v << 3) | (v >> 2)) as u8;
        assert_eq!(red(0, 5), widen(8));
        assert_eq!(red(4, 5), widen(16));
        assert_eq!(red(0, 2), widen(8));
        assert_eq!(red(0, 1), widen(24));
        assert_eq!(red(4, 0), widen(31));
        assert_eq!(img.rgba[3], 255);

        // BC7 mode 6 with equal endpoints: R 127 and A 127 with the P-bit 1 give 255 and G = B = 0
        // with the P-bit give 1, since the P-bit is the lowest bit of every channel
        let mut v: u128 = 1 << 6;
        v |= 127 << 7; // R0
        v |= 127 << 14; // R1
        v |= 127 << 49; // A0
        v |= 127 << 56; // A1
        v |= 1 << 63; // P-bit of endpoint 0
        v |= 1 << 64; // P-bit of endpoint 1
        let img = decode_pixels(FMT_BC7, 3, 2, &v.to_le_bytes()).unwrap();
        assert_eq!(img.rgba.len(), 3 * 2 * 4);
        assert!(img.rgba.chunks(4).all(|p| p == [255, 1, 1, 255]));
    }

    #[test]
    fn short_data_is_an_error() {
        assert!(decode_pixels(FMT_DXT5, 8, 8, &[0; 16]).is_err());
        assert!(decode_pixels(99, 4, 4, &[0; 64]).is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(top_level_size(FMT_DXT1, 256, 256), Some(256 * 256 / 2));
        assert_eq!(top_level_size(FMT_DXT5, 256, 128), Some(256 * 128));
        assert_eq!(top_level_size(FMT_RGBA32, 3, 3), Some(36));
    }
}

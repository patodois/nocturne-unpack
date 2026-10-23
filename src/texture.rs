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
            type Decoder = fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>;
            let decode: Decoder = match format {
                FMT_DXT1 => texture2ddecoder::decode_bc1,
                FMT_DXT5 => texture2ddecoder::decode_bc3,
                _ => texture2ddecoder::decode_bc7,
            };
            // The decoder works on whole blocks, so a band of block rows at a time (blocks are
            // stored row by row) keeps the scratch buffer small for big textures
            let block_bytes = if format == FMT_DXT1 { 8 } else { 16 };
            let bw = w.div_ceil(4) * 4;
            let block_rows = h.div_ceil(4);
            const BAND: usize = 16;
            let mut pixels = vec![0u32; bw * BAND * 4];
            let mut r0 = 0;
            while r0 < block_rows {
                let rows = BAND.min(block_rows - r0);
                let src = &data[r0 * (bw / 4) * block_bytes..(r0 + rows) * (bw / 4) * block_bytes];
                decode(src, bw, rows * 4, &mut pixels[..bw * rows * 4])
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                for (i, row) in pixels[..bw * rows * 4].chunks_exact(bw).enumerate() {
                    let y = r0 * 4 + i;
                    if y >= h {
                        break;
                    }
                    put(
                        y,
                        &mut row[..w].iter().map(|p| {
                            // BGRA in little-endian order
                            let [b, g, r, a] = p.to_le_bytes();
                            [r, g, b, a]
                        }),
                    );
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

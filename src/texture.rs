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
    let mut rgba = Vec::with_capacity(w * h * 4);
    match format {
        FMT_ALPHA8 => {
            for &a in data {
                rgba.extend_from_slice(&[255, 255, 255, a]);
            }
        }
        FMT_RGB24 => {
            for p in data.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        FMT_RGBA32 => rgba.extend_from_slice(data),
        _ => {
            type Decoder = fn(&[u8], usize, usize, &mut [u32]) -> Result<(), &'static str>;
            let decode: Decoder = match format {
                FMT_DXT1 => texture2ddecoder::decode_bc1,
                FMT_DXT5 => texture2ddecoder::decode_bc3,
                _ => texture2ddecoder::decode_bc7,
            };
            // The decoder works on whole blocks; pad to a multiple of 4 and crop
            let (bw, bh) = (w.div_ceil(4) * 4, h.div_ceil(4) * 4);
            let mut pixels = vec![0u32; bw * bh];
            decode(data, bw, bh, &mut pixels).map_err(|e| anyhow::anyhow!("{e}"))?;
            for row in pixels.chunks_exact(bw).take(h) {
                for p in &row[..w] {
                    // BGRA in little-endian order
                    let [b, g, r, a] = p.to_le_bytes();
                    rgba.extend_from_slice(&[r, g, b, a]);
                }
            }
        }
    }
    // Flip: Unity's first row is the bottom one
    let stride = w * 4;
    let mut flipped = Vec::with_capacity(rgba.len());
    for row in rgba.chunks_exact(stride).rev() {
        flipped.extend_from_slice(row);
    }
    Ok(Image {
        width: w as u32,
        height: h as u32,
        rgba: flipped,
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

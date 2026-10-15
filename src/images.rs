//! Cover image processing: every upload is decoded, cropped to 16:9 and re-encoded as JPEG.
//! Re-encoding strips metadata and anything that isn't pixels, and keeps files small.

use std::{io::Cursor, path::Path};

use image::{imageops::FilterType, ImageFormat, ImageReader, Limits};

pub const MAX_UPLOAD: usize = 5 * 1024 * 1024;
const WIDTH: u32 = 640;
const HEIGHT: u32 = 360;

/// Processes an upload and stores it in `dir`, returning the stored file name.
pub fn store_cover(bytes: &[u8], dir: &Path) -> Result<String, String> {
    if bytes.len() > MAX_UPLOAD {
        return Err("Image is larger than 5 MB.".into());
    }
    let format = image::guess_format(bytes).map_err(|_| "Unrecognised image format.".to_string())?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif) {
        return Err("Only PNG, JPEG, WebP and GIF images are accepted.".into());
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(8000);
    limits.max_image_height = Some(8000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let img = reader.decode().map_err(|_| "The image could not be decoded.".to_string())?;
    let img = img.resize_to_fill(WIDTH, HEIGHT, FilterType::Lanczos3).to_rgb8();

    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82)
        .encode_image(&img)
        .map_err(|e| format!("Failed to encode image: {e}"))?;

    let name = format!("{}.jpg", &crate::util::sha256_hex(&out)[..24]);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, &out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dir.join(&name)).map_err(|e| e.to_string())?;
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_and_reencodes() {
        let img = image::RgbaImage::from_pixel(1000, 1000, image::Rgba([200, 30, 30, 255]));
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();
        let dir = std::env::temp_dir().join(format!("decompsite-test-{}", std::process::id()));
        let name = store_cover(&png, &dir).unwrap();
        let out = image::open(dir.join(&name)).unwrap();
        assert_eq!((out.width(), out.height()), (WIDTH, HEIGHT));
        assert!(store_cover(b"<svg></svg>", &dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

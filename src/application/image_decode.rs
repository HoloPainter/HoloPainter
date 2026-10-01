use std::{path::Path, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use image::ImageFormat;

pub const SUPPORTED_RASTER_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterImageFormat {
    Png,
    Jpeg,
}

impl RasterImageFormat {
    pub fn from_path(path: &Path) -> Result<Self> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .context("image file has no supported extension")?;
        Self::from_extension(extension)
    }

    pub fn from_extension(extension: &str) -> Result<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            _ => bail!("only PNG and JPEG images are supported"),
        }
    }

    const fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
        }
    }
}

pub fn is_supported_raster_image_path(path: &Path) -> bool {
    RasterImageFormat::from_path(path).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedRgba8Image {
    pub file_name: String,
    pub size: [u32; 2],
    pub rgba8: Arc<[u8]>,
}

pub fn decode_raster_image(
    path: &Path,
    max_texture_dimension_2d: u32,
) -> Result<DecodedRgba8Image> {
    let format = RasterImageFormat::from_path(path)?;
    let bytes = std::fs::read(path).with_context(|| format!("reading image {}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Image");
    decode_raster_image_bytes(&bytes, file_name, format, max_texture_dimension_2d)
}

pub(crate) fn decode_png_bytes(
    bytes: &[u8],
    file_name: &str,
    max_texture_dimension_2d: u32,
) -> Result<DecodedRgba8Image> {
    decode_raster_image_bytes(
        bytes,
        file_name,
        RasterImageFormat::Png,
        max_texture_dimension_2d,
    )
}

fn decode_raster_image_bytes(
    bytes: &[u8],
    file_name: &str,
    format: RasterImageFormat,
    max_texture_dimension_2d: u32,
) -> Result<DecodedRgba8Image> {
    let decoded = image::load_from_memory_with_format(bytes, format.image_format())
        .with_context(|| format!("decoding PNG/JPEG image {file_name:?}"))?;
    let rgba = decoded.to_rgba8();

    prepare_straight_rgba8_image(
        file_name.to_owned(),
        [rgba.width(), rgba.height()],
        rgba.into_raw(),
        max_texture_dimension_2d,
    )
}

pub fn prepare_rgba8_image(
    file_name: impl Into<String>,
    size: [u32; 2],
    mut rgba8: Vec<u8>,
    max_texture_dimension_2d: u32,
) -> Result<DecodedRgba8Image> {
    ensure!(size[0] > 0 && size[1] > 0, "invalid image dimensions");
    ensure!(
        size[0] <= max_texture_dimension_2d && size[1] <= max_texture_dimension_2d,
        "image size {}x{} exceeds GPU texture limit {}",
        size[0],
        size[1],
        max_texture_dimension_2d
    );
    let expected_len = crate::core::image::rgba8_len(size)?;
    ensure!(
        rgba8.len() == expected_len,
        "RGBA8 pixel length mismatch: expected {expected_len}, got {}",
        rgba8.len()
    );
    premultiply_rgba8(&mut rgba8);

    Ok(DecodedRgba8Image {
        file_name: file_name.into(),
        size,
        rgba8: Arc::from(rgba8),
    })
}

pub fn prepare_straight_rgba8_image(
    file_name: impl Into<String>,
    size: [u32; 2],
    rgba8: Vec<u8>,
    max_texture_dimension_2d: u32,
) -> Result<DecodedRgba8Image> {
    ensure!(size[0] > 0 && size[1] > 0, "invalid image dimensions");
    ensure!(
        size[0] <= max_texture_dimension_2d && size[1] <= max_texture_dimension_2d,
        "image size {}x{} exceeds GPU texture limit {}",
        size[0],
        size[1],
        max_texture_dimension_2d
    );
    let expected_len = crate::core::image::rgba8_len(size)?;
    ensure!(
        rgba8.len() == expected_len,
        "RGBA8 pixel length mismatch: expected {expected_len}, got {}",
        rgba8.len()
    );
    Ok(DecodedRgba8Image {
        file_name: file_name.into(),
        size,
        rgba8: Arc::from(rgba8),
    })
}

fn premultiply_rgba8(rgba8: &mut [u8]) {
    for pixel in rgba8.chunks_exact_mut(4) {
        let alpha = pixel[3] as u16;
        for channel in &mut pixel[..3] {
            *channel = ((*channel as u16 * alpha + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    fn png_bytes(rgba8: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(rgba8, 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    fn jpeg_bytes(rgb8: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 100)
            .write_image(rgb8, 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    #[test]
    fn premultiply_preserves_opaque_and_scales_translucent_rgb() {
        let mut pixels = vec![100, 50, 20, 255, 200, 100, 50, 128, 9, 8, 7, 0];
        premultiply_rgba8(&mut pixels);
        assert_eq!(&pixels[..4], &[100, 50, 20, 255]);
        assert_eq!(&pixels[4..8], &[100, 50, 25, 128]);
        assert_eq!(&pixels[8..], &[0, 0, 0, 0]);
    }

    #[test]
    fn raster_image_extensions_are_case_insensitive_and_explicit() {
        for path in [
            "image.png",
            "image.PNG",
            "image.jpg",
            "image.JPG",
            "image.jpeg",
            "image.JPEG",
        ] {
            assert!(is_supported_raster_image_path(Path::new(path)), "{path}");
        }
        assert!(!is_supported_raster_image_path(Path::new("image.gif")));
        assert!(!is_supported_raster_image_path(Path::new("image")));
    }

    #[test]
    fn png_and_jpeg_share_rgba8_decode_output() {
        let directory = tempfile::tempdir().unwrap();
        let png_path = directory.path().join("transparent.PNG");
        std::fs::write(&png_path, png_bytes(&[200, 100, 50, 128])).unwrap();
        let png = decode_raster_image(&png_path, 4096).unwrap();
        assert_eq!(png.file_name, "transparent.PNG");
        assert_eq!(png.size, [1, 1]);
        assert_eq!(png.rgba8.as_ref(), &[200, 100, 50, 128]);

        let jpeg_path = directory.path().join("opaque.JpEg");
        std::fs::write(&jpeg_path, jpeg_bytes(&[200, 100, 50])).unwrap();
        let jpeg = decode_raster_image(&jpeg_path, 4096).unwrap();
        assert_eq!(jpeg.file_name, "opaque.JpEg");
        assert_eq!(jpeg.size, [1, 1]);
        assert_eq!(jpeg.rgba8[3], 255);
    }

    #[test]
    fn decode_obeys_the_explicit_path_format_allowlist() {
        let directory = tempfile::tempdir().unwrap();
        let mismatched = directory.path().join("actually-png.jpg");
        std::fs::write(&mismatched, png_bytes(&[1, 2, 3, 4])).unwrap();
        assert!(decode_raster_image(&mismatched, 4096).is_err());

        let unsupported = directory.path().join("actually-jpeg.gif");
        std::fs::write(&unsupported, jpeg_bytes(&[1, 2, 3])).unwrap();
        assert!(decode_raster_image(&unsupported, 4096).is_err());
    }

    #[test]
    fn rgba8_image_validates_and_premultiplies_pixels() {
        let image =
            prepare_rgba8_image("Clipboard Image", [1, 1], vec![200, 100, 50, 128], 4096).unwrap();
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.rgba8.as_ref(), &[100, 50, 25, 128]);
    }

    #[test]
    fn rgba8_image_rejects_invalid_dimensions_and_pixels() {
        assert!(prepare_rgba8_image("image", [0, 1], Vec::new(), 4096).is_err());
        assert!(prepare_rgba8_image("image", [2, 2], vec![0; 4], 4096).is_err());
        assert!(prepare_rgba8_image("image", [2, 2], vec![0; 16], 1).is_err());
    }
}

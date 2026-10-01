use std::{path::Path, sync::Arc};

use anyhow::Result;

use super::{
    Command, DecodedRgba8Image, RasterImageFormat, decode_raster_image, prepare_rgba8_image,
    prepare_straight_rgba8_image,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedImageImportPayload {
    pub file_name: String,
    pub layer_name: String,
    pub size: [u32; 2],
    pub rgba8_straight: Arc<[u8]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedImagePayload {
    pub file_name: String,
    pub layer_name: String,
    pub size: [u32; 2],
    pub rgba8: Arc<[u8]>,
    pub initial_origin: Option<[u32; 2]>,
}

pub fn load_rgba8_as_command(
    file_name: impl Into<String>,
    layer_name: impl Into<String>,
    size: [u32; 2],
    rgba8: Vec<u8>,
    max_texture_dimension_2d: u32,
) -> Result<Command> {
    load_rgba8_as_command_at(
        file_name,
        layer_name,
        size,
        rgba8,
        max_texture_dimension_2d,
        None,
    )
}

pub fn load_rgba8_as_command_at(
    file_name: impl Into<String>,
    layer_name: impl Into<String>,
    size: [u32; 2],
    rgba8: Vec<u8>,
    max_texture_dimension_2d: u32,
    initial_origin: Option<[u32; 2]>,
) -> Result<Command> {
    let image = prepare_straight_rgba8_image(file_name, size, rgba8, max_texture_dimension_2d)?;
    Ok(import_command(
        image.file_name,
        layer_name.into(),
        image.size,
        image.rgba8,
        initial_origin,
    ))
}

pub fn load_rgba8_as_paste_command_at(
    file_name: impl Into<String>,
    layer_name: impl Into<String>,
    size: [u32; 2],
    rgba8: Vec<u8>,
    max_texture_dimension_2d: u32,
    initial_origin: Option<[u32; 2]>,
) -> Result<Command> {
    let image = prepare_rgba8_image(file_name, size, rgba8, max_texture_dimension_2d)?;
    Ok(paste_command(
        image.file_name,
        layer_name.into(),
        image.size,
        image.rgba8,
        initial_origin,
    ))
}

pub fn load_image_as_command(path: &Path, max_texture_dimension_2d: u32) -> Result<Command> {
    let image = decode_raster_image(path, max_texture_dimension_2d)?;
    Ok(load_decoded_image_as_command(
        image,
        imported_layer_name(path),
        None,
    ))
}

pub(crate) fn load_decoded_image_as_command(
    image: DecodedRgba8Image,
    layer_name: impl Into<String>,
    initial_origin: Option<[u32; 2]>,
) -> Command {
    import_command(
        image.file_name,
        layer_name.into(),
        image.size,
        image.rgba8,
        initial_origin,
    )
}

pub(crate) fn imported_layer_name(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| {
            !value
                .strip_prefix('.')
                .is_some_and(|extension| RasterImageFormat::from_extension(extension).is_ok())
        })
        .unwrap_or("Imported Image")
        .to_owned()
}

fn import_command(
    file_name: String,
    layer_name: String,
    size: [u32; 2],
    rgba8: Arc<[u8]>,
    _initial_origin: Option<[u32; 2]>,
) -> Command {
    Command::BeginEmbeddedImageImport {
        image: EmbeddedImageImportPayload {
            file_name,
            layer_name,
            size,
            rgba8_straight: rgba8,
        },
    }
}

fn paste_command(
    file_name: String,
    layer_name: String,
    size: [u32; 2],
    rgba8: Arc<[u8]>,
    initial_origin: Option<[u32; 2]>,
) -> Command {
    Command::PasteImageAsLayer {
        image: ImportedImagePayload {
            file_name,
            layer_name,
            size,
            rgba8,
            initial_origin,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;

    #[test]
    fn rgba8_import_command_preserves_straight_alpha_pixels() {
        let command = load_rgba8_as_command(
            "Clipboard Image",
            "Clipboard Image",
            [1, 1],
            vec![200, 100, 50, 128],
            4096,
        )
        .unwrap();
        let Command::BeginEmbeddedImageImport { image } = command else {
            panic!("expected embedded image command");
        };
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.rgba8_straight.as_ref(), &[200, 100, 50, 128]);
    }

    #[test]
    fn rgba8_command_rejects_invalid_dimensions_and_pixels() {
        assert!(load_rgba8_as_command("image", "image", [0, 1], Vec::new(), 4096).is_err());
        assert!(load_rgba8_as_command("image", "image", [2, 2], vec![0; 4], 4096).is_err());
        assert!(load_rgba8_as_command("image", "image", [2, 2], vec![0; 16], 1).is_err());
    }

    #[test]
    fn decoded_image_command_preserves_straight_pixels() {
        let image =
            prepare_straight_rgba8_image("image.png", [1, 1], vec![200, 100, 50, 128], 4096)
                .unwrap();
        let command = load_decoded_image_as_command(image, "Imported", Some([7, 9]));
        let Command::BeginEmbeddedImageImport { image } = command else {
            panic!("expected embedded image command");
        };
        assert_eq!(image.layer_name, "Imported");
        assert_eq!(image.rgba8_straight.as_ref(), &[200, 100, 50, 128]);
    }

    #[test]
    fn imported_layer_name_uses_trimmed_stem_or_fallback() {
        assert_eq!(imported_layer_name(Path::new("folder/photo.png")), "photo");
        assert_eq!(imported_layer_name(Path::new("folder/photo.JPG")), "photo");
        assert_eq!(imported_layer_name(Path::new(".png")), "Imported Image");
        assert_eq!(imported_layer_name(Path::new(".PNG")), "Imported Image");
        assert_eq!(imported_layer_name(Path::new(".jpg")), "Imported Image");
        assert_eq!(imported_layer_name(Path::new(".JPEG")), "Imported Image");
    }

    #[test]
    fn jpeg_import_uses_the_common_embedded_image_command() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.JPEG");
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 100)
            .write_image(&[40, 80, 120], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        std::fs::write(&path, encoded).unwrap();

        let command = load_image_as_command(&path, 4096).unwrap();
        let Command::BeginEmbeddedImageImport { image } = command else {
            panic!("expected embedded image command");
        };
        assert_eq!(image.file_name, "photo.JPEG");
        assert_eq!(image.layer_name, "photo");
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.rgba8_straight[3], 255);
    }

    #[test]
    fn embedded_import_ignores_drop_origin() {
        let command =
            load_rgba8_as_command_at("image", "image", [1, 1], vec![0; 4], 4096, Some([12, 34]))
                .unwrap();
        let Command::BeginEmbeddedImageImport { image } = command else {
            panic!("expected embedded image command");
        };
        assert_eq!(image.size, [1, 1]);
    }

    #[test]
    fn rgba8_paste_command_validates_premultiplies_and_preserves_origin() {
        let command = load_rgba8_as_paste_command_at(
            "Clipboard Image",
            "Clipboard Image",
            [1, 1],
            vec![200, 100, 50, 128],
            4096,
            Some([12, 34]),
        )
        .unwrap();
        let Command::PasteImageAsLayer { image } = command else {
            panic!("expected pasted image command");
        };
        assert_eq!(image.initial_origin, Some([12, 34]));
        assert_eq!(image.rgba8.as_ref(), &[100, 50, 25, 128]);
    }
}

use std::collections::HashMap;

use anyhow::{Context as _, Result, ensure};
use eframe::egui;

pub const APP_ICON_PNG_BYTES: &[u8] = include_bytes!("../../assets/app_icon/app_icon_256.png");

pub struct UiIconRegistry {
    textures: HashMap<&'static str, egui::TextureHandle>,
    rotate_cursor: egui::CustomCursorImage,
}

impl UiIconRegistry {
    pub fn new(ctx: &egui::Context) -> Result<Self> {
        let mut textures = HashMap::new();
        for icon in BUILTIN_ICONS {
            let image = load_icon_image(icon.id, icon.png_bytes)?;
            let texture = ctx.load_texture(icon.id, image, egui::TextureOptions::LINEAR);
            textures.insert(icon.id, texture);
        }
        let rotate_cursor = load_cursor_image(
            "builtin.cursor.rotate_left",
            ROTATE_CURSOR_PNG_BYTES,
            [16, 16],
        )?;
        Ok(Self {
            textures,
            rotate_cursor,
        })
    }

    pub fn texture(&self, icon_id: &str) -> Option<&egui::TextureHandle> {
        self.textures.get(icon_id)
    }

    pub fn rotate_cursor(&self) -> &egui::CustomCursorImage {
        &self.rotate_cursor
    }
}

struct BuiltinIconAsset {
    id: &'static str,
    png_bytes: &'static [u8],
}

const ROTATE_CURSOR_PNG_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/icons/png/32/rotate-left.png"
));

const BUILTIN_ICONS: &[BuiltinIconAsset] = &[
    BuiltinIconAsset {
        id: "builtin.icon.app",
        png_bytes: APP_ICON_PNG_BYTES,
    },
    BuiltinIconAsset {
        id: "builtin.icon.open_in_new",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/open_in_new.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.photo_camera",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/photo_camera.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.brush",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/brush.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.fragrance",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/fragrance.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.eraser",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/eraser.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.stylus_brush",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/stylus_brush.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.sticker",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/sticker.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.select",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/select.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.colorize",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/colorize.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.colors",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/colors.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.shapes",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/shapes.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.transform",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/transform.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.cancel",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/cancel.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.check_circle",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/check_circle.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.new_layer",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/new_layer.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.new_folder",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/new_folder.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.adjustment_brightness_contrast",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/brightness_6.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.adjustment_levels",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/equalizer.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.adjustment_hsv",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/discover_tune.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.adjustment_curves",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/line_curve.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.adjustment_invert",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/invert_colors.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.adjustment_gradient_map",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/gradient.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.content_copy",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/content_copy.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.layer_merge",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/layer_merge.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.square_dot",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/square_dot.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.add",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/add.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.delete",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/delete.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.lock",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/lock.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.visibility",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/visibility.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.visibility_off",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/visibility_off.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.folder",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/folder.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.image",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/image.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.folder_open",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/folder_open.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.flip",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/flip.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.mesh",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/mesh.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.refresh",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/refresh.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.help",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/help.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.mouse_left_click",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/mouse_left_click.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.mouse_right_click",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/mouse_right_click.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.mouse_middle_click",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/mouse_middle_click.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.scroll",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/scroll.png"
        )),
    },
    BuiltinIconAsset {
        id: "builtin.icon.masked_transitions",
        png_bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/icons/png/24/masked_transitions.png"
        )),
    },
];

fn load_cursor_image(
    cursor_id: &str,
    png_bytes: &[u8],
    hotspot: [u16; 2],
) -> Result<egui::CustomCursorImage> {
    let image = image::load_from_memory(png_bytes)
        .with_context(|| format!("decoding builtin cursor {cursor_id}"))?
        .into_rgba8();
    let size = [
        u16::try_from(image.width())
            .with_context(|| format!("builtin cursor {cursor_id} width exceeds u16"))?,
        u16::try_from(image.height())
            .with_context(|| format!("builtin cursor {cursor_id} height exceeds u16"))?,
    ];
    ensure!(
        hotspot[0] < size[0] && hotspot[1] < size[1],
        "builtin cursor {cursor_id} hotspot is outside the image"
    );
    Ok(egui::CustomCursorImage {
        rgba: image.into_raw().into(),
        size,
        hotspot,
    })
}

fn load_icon_image(icon_id: &str, png_bytes: &[u8]) -> Result<egui::ColorImage> {
    let image = image::load_from_memory(png_bytes)
        .with_context(|| format!("decoding builtin icon {icon_id}"))?
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        image.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_cursor_asset_has_expected_size_and_hotspot() {
        let cursor = load_cursor_image(
            "builtin.cursor.rotate_left",
            ROTATE_CURSOR_PNG_BYTES,
            [16, 16],
        )
        .unwrap();

        assert_eq!(cursor.size, [32, 32]);
        assert_eq!(cursor.hotspot, [16, 16]);
        assert_eq!(cursor.rgba.len(), 32 * 32 * 4);
    }
}

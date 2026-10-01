use anyhow::{Context, Result, ensure};

use ag_psd::{psd as ag, write_psd};

use crate::core::{
    adjustment::{Adjustment, CurveChannel, GradientMapAdjustment, LevelsChannel},
    composite::{GroupCompositeMode, LayerBlendMode},
};

use super::model::{PsdExportDocument, PsdExportLayer, PsdExportLayerContent, PsdExportMask};

pub fn serialize_psd(document: &PsdExportDocument) -> Result<Vec<u8>> {
    ensure!(
        document.width > 0 && document.height > 0,
        "PSD dimensions must be non-zero"
    );
    validate_rgba_len(&document.composite_rgba8, [document.width, document.height])?;
    let size = [document.width, document.height];
    let psd = ag::Psd {
        width: f64::from(document.width),
        height: f64::from(document.height),
        channels: Some(4.0),
        bits_per_channel: Some(8.0),
        color_mode: Some(ag::ColorMode::Rgb),
        children: Some(
            document
                .layers
                .iter()
                .rev()
                .map(|layer| convert_layer(layer, size))
                .collect::<Result<Vec<_>>>()?,
        ),
        image_data: Some(pixel_data(size, document.composite_rgba8.clone())?),
        image_resources: Some(ag::ImageResources {
            icc_untagged_profile: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    };
    Ok(write_psd(
        &psd,
        &ag::WriteOptions {
            no_background: Some(true),
            compress: Some(false),
            generate_thumbnail: Some(true),
            ..Default::default()
        },
    ))
}

fn convert_layer(layer: &PsdExportLayer, size: [u32; 2]) -> Result<ag::Layer> {
    let mut additional_info = ag::LayerAdditionalInfo {
        name: Some(layer.name.clone()),
        mask: layer
            .mask
            .as_ref()
            .map(|mask| convert_mask(mask, size))
            .transpose()?,
        protected_info: layer.locked.then(|| ag::ProtectedInfo {
            transparency: Some(true),
            composite: Some(true),
            position: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut image_data = None;
    let mut children = None;
    let blend_mode = match &layer.content {
        PsdExportLayerContent::Raster { rgba8 } => {
            image_data = Some(pixel_data(size, rgba8.clone())?);
            map_blend_mode(layer.blend_mode)
        }
        PsdExportLayerContent::SolidFill { color } => {
            additional_info.vector_fill = Some(ag::VectorContent::Color(ag::Color::Rgb(ag::Rgb {
                r: f64::from(color[0].clamp(0.0, 1.0) * 255.0),
                g: f64::from(color[1].clamp(0.0, 1.0) * 255.0),
                b: f64::from(color[2].clamp(0.0, 1.0) * 255.0),
            })));
            image_data = Some(pixel_data(size, solid_fill_rgba8(size, *color)?)?);
            map_blend_mode(layer.blend_mode)
        }
        PsdExportLayerContent::Adjustment { adjustment } => {
            additional_info.adjustment = Some(convert_adjustment(adjustment)?);
            map_blend_mode(layer.blend_mode)
        }
        PsdExportLayerContent::Group {
            composite_mode,
            children: group_children,
        } => {
            children = Some(
                group_children
                    .iter()
                    .rev()
                    .map(|child| convert_layer(child, size))
                    .collect::<Result<Vec<_>>>()?,
            );
            match composite_mode {
                GroupCompositeMode::PassThrough => ag::BlendMode::PassThrough,
                GroupCompositeMode::Isolated => map_blend_mode(layer.blend_mode),
            }
        }
    };
    Ok(ag::Layer {
        additional_info,
        top: Some(0.0),
        left: Some(0.0),
        bottom: Some(f64::from(size[1])),
        right: Some(f64::from(size[0])),
        blend_mode: Some(blend_mode),
        opacity: Some(f64::from(layer.opacity.clamp(0.0, 1.0))),
        transparency_protected: layer.locked.then_some(true),
        hidden: Some(!layer.visible),
        image_data,
        children,
        opened: matches!(layer.content, PsdExportLayerContent::Group { .. }).then_some(true),
        ..Default::default()
    })
}

fn convert_mask(mask: &PsdExportMask, size: [u32; 2]) -> Result<ag::LayerMaskData> {
    let expected = pixel_count(size)?;
    ensure!(
        mask.r8.len() == expected,
        "PSD mask byte length does not match canvas"
    );
    let mut rgba8 = Vec::with_capacity(expected * 4);
    for &value in &mask.r8 {
        rgba8.extend_from_slice(&[value, value, value, 255]);
    }
    Ok(ag::LayerMaskData {
        top: Some(0.0),
        left: Some(0.0),
        bottom: Some(f64::from(size[1])),
        right: Some(f64::from(size[0])),
        default_color: Some(255.0),
        disabled: Some(!mask.enabled),
        position_relative_to_layer: Some(false),
        from_vector_data: Some(false),
        image_data: Some(pixel_data(size, rgba8)?),
        ..Default::default()
    })
}

fn convert_adjustment(adjustment: &Adjustment) -> Result<ag::AdjustmentLayer> {
    Ok(match adjustment {
        Adjustment::BrightnessContrast(value) => {
            ag::AdjustmentLayer::Brightness(ag::BrightnessAdjustment {
                brightness: Some(f64::from(value.brightness)),
                contrast: Some(f64::from(value.contrast)),
                use_legacy: Some(false),
                ..Default::default()
            })
        }
        Adjustment::Levels(value) => ag::AdjustmentLayer::Levels(ag::LevelsAdjustment {
            rgb: Some(convert_levels_channel(value.master)),
            red: Some(convert_levels_channel(value.red)),
            green: Some(convert_levels_channel(value.green)),
            blue: Some(convert_levels_channel(value.blue)),
            ..Default::default()
        }),
        Adjustment::Curves(value) => ag::AdjustmentLayer::Curves(ag::CurvesAdjustment {
            rgb: Some(convert_curve_channel(value.master)),
            red: Some(convert_curve_channel(value.red)),
            green: Some(convert_curve_channel(value.green)),
            blue: Some(convert_curve_channel(value.blue)),
            ..Default::default()
        }),
        Adjustment::HueSaturation(value) => {
            ag::AdjustmentLayer::HueSaturation(ag::HueSaturationAdjustment {
                master: Some(ag::HueSaturationAdjustmentChannel {
                    hue: f64::from(value.hue),
                    saturation: f64::from(value.saturation),
                    lightness: f64::from(value.lightness),
                    ..Default::default()
                }),
                ..Default::default()
            })
        }
        Adjustment::Invert => ag::AdjustmentLayer::Invert(ag::InvertAdjustment),
        Adjustment::GradientMap(value) => convert_gradient_map(value),
        Adjustment::UvMirror(_) => anyhow::bail!("UV Mirror must not reach PSD conversion"),
    })
}

fn convert_levels_channel(channel: LevelsChannel) -> ag::LevelsAdjustmentChannel {
    ag::LevelsAdjustmentChannel {
        shadow_input: f64::from(channel.input_black),
        highlight_input: f64::from(channel.input_white),
        shadow_output: f64::from(channel.output_black),
        highlight_output: f64::from(channel.output_white),
        midtone_input: f64::from(channel.gamma),
    }
}

fn convert_curve_channel(channel: CurveChannel) -> ag::CurvesAdjustmentChannel {
    channel
        .points()
        .iter()
        .map(|point| ag::CurvesPoint {
            input: f64::from(point.input),
            output: f64::from(point.output),
        })
        .collect()
}

fn convert_gradient_map(value: &GradientMapAdjustment) -> ag::AdjustmentLayer {
    let color_stops = value
        .stops()
        .iter()
        .map(|stop| ag::ColorStop {
            color: ag::Color::Rgb(ag::Rgb {
                r: f64::from(stop.color[0]),
                g: f64::from(stop.color[1]),
                b: f64::from(stop.color[2]),
            }),
            location: f64::from(stop.location) / 4096.0,
            midpoint: f64::from(stop.midpoint) / 100.0,
        })
        .collect();
    ag::AdjustmentLayer::GradientMap(ag::GradientMapAdjustment {
        name: Some("HoloPainter Gradient Map".to_owned()),
        gradient_type: ag::GradientMapType::Solid,
        dither: Some(value.dither),
        reverse: Some(value.reverse),
        method: None,
        smoothness: Some(1.0),
        color_stops: Some(color_stops),
        opacity_stops: Some(vec![
            ag::OpacityStop {
                opacity: 1.0,
                location: 0.0,
                midpoint: 0.5,
            },
            ag::OpacityStop {
                opacity: 1.0,
                location: 1.0,
                midpoint: 0.5,
            },
        ]),
        roughness: None,
        color_model: None,
        random_seed: None,
        restrict_colors: None,
        add_transparency: None,
        min: None,
        max: None,
    })
}

fn map_blend_mode(mode: LayerBlendMode) -> ag::BlendMode {
    match mode {
        LayerBlendMode::Normal => ag::BlendMode::Normal,
        LayerBlendMode::Darken => ag::BlendMode::Darken,
        LayerBlendMode::Multiply => ag::BlendMode::Multiply,
        LayerBlendMode::Lighten => ag::BlendMode::Lighten,
        LayerBlendMode::Screen => ag::BlendMode::Screen,
        LayerBlendMode::ColorDodge => ag::BlendMode::ColorDodge,
        LayerBlendMode::LinearDodge => ag::BlendMode::LinearDodge,
        LayerBlendMode::Overlay => ag::BlendMode::Overlay,
        LayerBlendMode::SoftLight => ag::BlendMode::SoftLight,
        LayerBlendMode::HardLight => ag::BlendMode::HardLight,
        LayerBlendMode::Color => ag::BlendMode::Color,
    }
}

fn solid_fill_rgba8(size: [u32; 2], color: [f32; 3]) -> Result<Vec<u8>> {
    let pixel = [
        float_to_u8(color[0]),
        float_to_u8(color[1]),
        float_to_u8(color[2]),
        255,
    ];
    Ok((0..pixel_count(size)?).flat_map(|_| pixel).collect())
}

fn float_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn pixel_data(size: [u32; 2], data: Vec<u8>) -> Result<ag::PixelData> {
    validate_rgba_len(&data, size)?;
    Ok(ag::PixelData {
        width: size[0],
        height: size[1],
        data,
    })
}

fn validate_rgba_len(rgba8: &[u8], size: [u32; 2]) -> Result<()> {
    let expected = pixel_count(size)?
        .checked_mul(4)
        .context("RGBA byte length overflows")?;
    ensure!(
        rgba8.len() == expected,
        "RGBA byte length does not match canvas"
    );
    Ok(())
}

fn pixel_count(size: [u32; 2]) -> Result<usize> {
    ensure!(!size.contains(&0), "pixel dimensions must be non-zero");
    usize::try_from(size[0])?
        .checked_mul(usize::try_from(size[1])?)
        .context("pixel count overflows")
}

#[cfg(test)]
mod tests {
    use ag_psd::{
        psd::{Compression, ReadOptions},
        read_psd,
    };

    use super::*;

    #[test]
    fn serializer_round_trip_preserves_layer_structure_and_native_metadata() {
        let document = PsdExportDocument {
            width: 2,
            height: 2,
            layers: vec![PsdExportLayer {
                name: "日本語 🖌️".to_owned(),
                visible: true,
                locked: true,
                opacity: 0.5,
                blend_mode: LayerBlendMode::Multiply,
                mask: Some(PsdExportMask {
                    enabled: false,
                    r8: vec![0, 64, 128, 255],
                }),
                content: PsdExportLayerContent::SolidFill {
                    color: [0.25, 0.5, 0.75],
                },
            }],
            composite_rgba8: vec![0; 16],
        };

        let bytes = serialize_psd(&document).unwrap();
        let decoded = read_psd(
            &bytes,
            &ReadOptions {
                use_image_data: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        let layer = &decoded.children.unwrap()[0];
        assert_eq!(layer.additional_info.name.as_deref(), Some("日本語 🖌️"));
        assert_eq!(layer.blend_mode, Some(ag::BlendMode::Multiply));
        assert_eq!(
            layer.additional_info.mask.as_ref().unwrap().disabled,
            Some(true)
        );
        assert!(matches!(
            layer.additional_info.vector_fill,
            Some(ag::VectorContent::Color(_))
        ));
        assert_eq!(
            layer
                .additional_info
                .protected_info
                .as_ref()
                .unwrap()
                .position,
            Some(true)
        );
    }

    #[test]
    fn ag_psd_boundary_reverses_top_first_layers_at_every_hierarchy_level() {
        let document = PsdExportDocument {
            width: 1,
            height: 1,
            layers: vec![
                PsdExportLayer {
                    name: "Top Group".to_owned(),
                    visible: true,
                    locked: false,
                    opacity: 1.0,
                    blend_mode: LayerBlendMode::Normal,
                    mask: None,
                    content: PsdExportLayerContent::Group {
                        composite_mode: GroupCompositeMode::PassThrough,
                        children: vec![
                            raster_layer("Top Child", 10),
                            raster_layer("Bottom Child", 20),
                        ],
                    },
                },
                raster_layer("Bottom Layer", 30),
            ],
            composite_rgba8: vec![0, 0, 0, 0],
        };

        let decoded = read_psd(
            &serialize_psd(&document).unwrap(),
            &ReadOptions {
                use_image_data: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        let layers = decoded.children.unwrap();
        assert_eq!(
            layers[0].additional_info.name.as_deref(),
            Some("Bottom Layer")
        );
        assert_eq!(layers[1].additional_info.name.as_deref(), Some("Top Group"));
        let children = layers[1].children.as_ref().unwrap();
        assert_eq!(
            children[0].additional_info.name.as_deref(),
            Some("Bottom Child")
        );
        assert_eq!(
            children[1].additional_info.name.as_deref(),
            Some("Top Child")
        );
    }

    #[test]
    fn serializer_uses_rle_for_layer_channels_and_composite() {
        let document = PsdExportDocument {
            width: 1,
            height: 1,
            layers: vec![raster_layer("Raster", 42)],
            composite_rgba8: vec![42, 0, 0, 255],
        };

        let decoded = read_psd(
            &serialize_psd(&document).unwrap(),
            &ReadOptions {
                use_raw_data: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        let layer = &decoded.children.as_ref().unwrap()[0];
        assert!(
            layer
                .raw_data
                .as_ref()
                .unwrap()
                .channels
                .iter()
                .all(|channel| channel.compression == Compression::RleCompressed)
        );
        assert_eq!(
            decoded.raw_composite_data.as_deref().unwrap().get(..2),
            Some([0_u8, 1].as_slice())
        );
    }

    fn raster_layer(name: &str, red: u8) -> PsdExportLayer {
        PsdExportLayer {
            name: name.to_owned(),
            visible: true,
            locked: false,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
            mask: None,
            content: PsdExportLayerContent::Raster {
                rgba8: vec![red, 0, 0, 255],
            },
        }
    }
}

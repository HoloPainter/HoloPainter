use std::path::PathBuf;

use ag_psd::psd::{
    AdjustmentLayer, BlendMode, BrightnessAdjustment, Color, ColorMode, ColorStop,
    CurvesAdjustment, CurvesPoint, GradientMapAdjustment, GradientMapType, HueSaturationAdjustment,
    HueSaturationAdjustmentChannel, InvertAdjustment, Layer, LayerAdditionalInfo, LayerMaskData,
    LevelsAdjustment, LevelsAdjustmentChannel, OpacityStop, PixelData, ProtectedInfo, Psd,
    ReadOptions, Rgb, VectorContent, WriteOptions,
};
use ag_psd::{read_psd, write_psd};

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

#[test]
fn ag_psd_native_export_contract_round_trips() {
    let source = compatibility_document();
    let bytes = write_psd(&source, &write_options());
    let decoded = read_psd(
        &bytes,
        &ReadOptions {
            use_image_data: Some(true),
            ..Default::default()
        },
    )
    .expect("compatibility PSD must be readable by ag-psd");

    assert_eq!(decoded.width, f64::from(WIDTH));
    assert_eq!(decoded.height, f64::from(HEIGHT));
    assert_eq!(decoded.color_mode, Some(ColorMode::Rgb));
    assert_eq!(decoded.bits_per_channel, Some(8.0));
    assert_eq!(
        decoded.image_data.as_ref().map(|data| data.data.len()),
        Some(64 * 64 * 4)
    );

    let root = find_layer(&decoded, "互換性 🖌️");
    assert_eq!(root.blend_mode, Some(BlendMode::PassThrough));
    assert!(
        root.children
            .as_ref()
            .is_some_and(|children| children.len() >= 10)
    );

    let masked = find_layer(&decoded, "Raster + Mask");
    let mask = masked
        .additional_info
        .mask
        .as_ref()
        .expect("raster mask must round-trip");
    assert_eq!(mask.disabled, Some(false));
    assert_eq!(
        mask.image_data.as_ref().map(|data| data.data.len()),
        Some(64 * 64 * 4)
    );
    assert!((masked.opacity.expect("opacity must round-trip") - 0.75).abs() <= 1.0 / 255.0);
    assert_eq!(masked.hidden, Some(false));
    let protection = masked
        .additional_info
        .protected_info
        .as_ref()
        .expect("lock protection must round-trip");
    assert_eq!(protection.transparency, Some(true));
    assert_eq!(protection.composite, Some(true));
    assert_eq!(protection.position, Some(true));

    assert!(matches!(
        find_layer(&decoded, "Solid Fill")
            .additional_info
            .vector_fill,
        Some(VectorContent::Color(_))
    ));
    assert!(matches!(
        find_layer(&decoded, "Brightness Contrast")
            .additional_info
            .adjustment,
        Some(AdjustmentLayer::Brightness(_))
    ));
    assert!(matches!(
        find_layer(&decoded, "Levels").additional_info.adjustment,
        Some(AdjustmentLayer::Levels(_))
    ));
    assert!(matches!(
        find_layer(&decoded, "Curves").additional_info.adjustment,
        Some(AdjustmentLayer::Curves(_))
    ));
    assert!(matches!(
        find_layer(&decoded, "Hue Saturation")
            .additional_info
            .adjustment,
        Some(AdjustmentLayer::HueSaturation(_))
    ));
    assert!(matches!(
        find_layer(&decoded, "Invert").additional_info.adjustment,
        Some(AdjustmentLayer::Invert(_))
    ));
    assert!(matches!(
        find_layer(&decoded, "Gradient Map")
            .additional_info
            .adjustment,
        Some(AdjustmentLayer::GradientMap(_))
    ));

    for (name, mode) in blend_modes() {
        assert_eq!(find_layer(&decoded, name).blend_mode, Some(mode));
    }
}

#[test]
#[ignore = "writes a fixture for manual Adobe Photoshop compatibility testing"]
fn write_photoshop_compatibility_fixture() {
    let output = manual_fixture_path();
    std::fs::create_dir_all(output.parent().expect("fixture path has a parent"))
        .expect("create compatibility fixture directory");
    std::fs::write(
        &output,
        write_psd(&compatibility_document(), &write_options()),
    )
    .expect("write compatibility fixture");
    eprintln!("{}", output.display());
}

fn compatibility_document() -> Psd {
    let rgba = checkerboard();
    let mask = radial_mask_rgba();
    let mut children = vec![
        raster_layer("Raster + Alpha", rgba.clone(), BlendMode::Normal),
        Layer {
            additional_info: LayerAdditionalInfo {
                name: Some("Raster + Mask".to_owned()),
                mask: Some(LayerMaskData {
                    top: Some(0.0),
                    left: Some(0.0),
                    bottom: Some(f64::from(HEIGHT)),
                    right: Some(f64::from(WIDTH)),
                    default_color: Some(255.0),
                    disabled: Some(false),
                    position_relative_to_layer: Some(false),
                    from_vector_data: Some(false),
                    image_data: Some(pixel_data(mask)),
                    ..Default::default()
                }),
                protected_info: Some(ProtectedInfo {
                    transparency: Some(true),
                    composite: Some(true),
                    position: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            top: Some(0.0),
            left: Some(0.0),
            bottom: Some(f64::from(HEIGHT)),
            right: Some(f64::from(WIDTH)),
            blend_mode: Some(BlendMode::Multiply),
            opacity: Some(0.75),
            hidden: Some(false),
            image_data: Some(pixel_data(rgba.clone())),
            ..Default::default()
        },
        solid_fill_layer(),
        adjustment_layer(
            "Brightness Contrast",
            AdjustmentLayer::Brightness(BrightnessAdjustment {
                brightness: Some(12.0),
                contrast: Some(18.0),
                use_legacy: Some(false),
                ..Default::default()
            }),
        ),
        adjustment_layer(
            "Levels",
            AdjustmentLayer::Levels(LevelsAdjustment {
                rgb: Some(levels_channel(12.0, 242.0, 0.9, 4.0, 250.0)),
                red: Some(levels_channel(5.0, 250.0, 1.1, 0.0, 255.0)),
                green: Some(levels_channel(0.0, 245.0, 1.0, 3.0, 252.0)),
                blue: Some(levels_channel(8.0, 255.0, 1.2, 0.0, 248.0)),
                ..Default::default()
            }),
        ),
        adjustment_layer(
            "Curves",
            AdjustmentLayer::Curves(CurvesAdjustment {
                rgb: Some(curve_points(&[(0, 0), (96, 80), (180, 205), (255, 255)])),
                red: Some(curve_points(&[(0, 4), (128, 140), (255, 250)])),
                green: Some(curve_points(&[(0, 0), (128, 120), (255, 255)])),
                blue: Some(curve_points(&[(0, 8), (128, 128), (255, 245)])),
                ..Default::default()
            }),
        ),
        adjustment_layer(
            "Hue Saturation",
            AdjustmentLayer::HueSaturation(HueSaturationAdjustment {
                master: Some(HueSaturationAdjustmentChannel {
                    hue: 15.0,
                    saturation: 20.0,
                    lightness: -5.0,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        ),
        adjustment_layer("Invert", AdjustmentLayer::Invert(InvertAdjustment)),
        adjustment_layer("Gradient Map", gradient_map()),
    ];
    children.push(Layer {
        additional_info: LayerAdditionalInfo {
            name: Some("Nested Group".to_owned()),
            ..Default::default()
        },
        blend_mode: Some(BlendMode::Normal),
        opacity: Some(0.8),
        hidden: Some(false),
        opened: Some(true),
        children: Some(
            blend_modes()
                .into_iter()
                .map(|(name, mode)| raster_layer(name, rgba.clone(), mode))
                .collect(),
        ),
        ..Default::default()
    });

    Psd {
        width: f64::from(WIDTH),
        height: f64::from(HEIGHT),
        channels: Some(4.0),
        bits_per_channel: Some(8.0),
        color_mode: Some(ColorMode::Rgb),
        children: Some(vec![Layer {
            additional_info: LayerAdditionalInfo {
                name: Some("互換性 🖌️".to_owned()),
                ..Default::default()
            },
            blend_mode: Some(BlendMode::PassThrough),
            opacity: Some(1.0),
            hidden: Some(false),
            opened: Some(true),
            children: Some(children),
            ..Default::default()
        }]),
        image_data: Some(pixel_data(rgba)),
        ..Default::default()
    }
}

fn solid_fill_layer() -> Layer {
    Layer {
        additional_info: LayerAdditionalInfo {
            name: Some("Solid Fill".to_owned()),
            vector_fill: Some(VectorContent::Color(Color::Rgb(Rgb {
                r: 48.0,
                g: 128.0,
                b: 220.0,
            }))),
            ..Default::default()
        },
        top: Some(0.0),
        left: Some(0.0),
        bottom: Some(f64::from(HEIGHT)),
        right: Some(f64::from(WIDTH)),
        blend_mode: Some(BlendMode::Normal),
        opacity: Some(0.6),
        hidden: Some(false),
        image_data: Some(pixel_data(solid_rgba([48, 128, 220, 255]))),
        ..Default::default()
    }
}

fn adjustment_layer(name: &str, adjustment: AdjustmentLayer) -> Layer {
    Layer {
        additional_info: LayerAdditionalInfo {
            name: Some(name.to_owned()),
            adjustment: Some(adjustment),
            ..Default::default()
        },
        top: Some(0.0),
        left: Some(0.0),
        bottom: Some(f64::from(HEIGHT)),
        right: Some(f64::from(WIDTH)),
        blend_mode: Some(BlendMode::Normal),
        opacity: Some(1.0),
        hidden: Some(false),
        ..Default::default()
    }
}

fn gradient_map() -> AdjustmentLayer {
    AdjustmentLayer::GradientMap(GradientMapAdjustment {
        name: Some("HoloPainter Gradient".to_owned()),
        gradient_type: GradientMapType::Solid,
        dither: Some(true),
        reverse: Some(false),
        method: None,
        smoothness: Some(1.0),
        color_stops: Some(vec![
            ColorStop {
                color: Color::Rgb(Rgb {
                    r: 0.0,
                    g: 0.0,
                    b: 24.0,
                }),
                location: 0.0,
                midpoint: 0.5,
            },
            ColorStop {
                color: Color::Rgb(Rgb {
                    r: 255.0,
                    g: 80.0,
                    b: 24.0,
                }),
                location: 0.55,
                midpoint: 0.4,
            },
            ColorStop {
                color: Color::Rgb(Rgb {
                    r: 255.0,
                    g: 255.0,
                    b: 240.0,
                }),
                location: 1.0,
                midpoint: 0.5,
            },
        ]),
        opacity_stops: Some(vec![
            OpacityStop {
                opacity: 1.0,
                location: 0.0,
                midpoint: 0.5,
            },
            OpacityStop {
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

fn raster_layer(name: &str, rgba: Vec<u8>, mode: BlendMode) -> Layer {
    Layer {
        additional_info: LayerAdditionalInfo {
            name: Some(name.to_owned()),
            ..Default::default()
        },
        top: Some(0.0),
        left: Some(0.0),
        bottom: Some(f64::from(HEIGHT)),
        right: Some(f64::from(WIDTH)),
        blend_mode: Some(mode),
        opacity: Some(1.0),
        hidden: Some(false),
        image_data: Some(pixel_data(rgba)),
        ..Default::default()
    }
}

fn levels_channel(
    shadow_input: f64,
    highlight_input: f64,
    midtone_input: f64,
    shadow_output: f64,
    highlight_output: f64,
) -> LevelsAdjustmentChannel {
    LevelsAdjustmentChannel {
        shadow_input,
        highlight_input,
        shadow_output,
        highlight_output,
        midtone_input,
    }
}

fn curve_points(points: &[(u8, u8)]) -> Vec<CurvesPoint> {
    points
        .iter()
        .map(|&(input, output)| CurvesPoint {
            input: f64::from(input),
            output: f64::from(output),
        })
        .collect()
}

fn blend_modes() -> [(&'static str, BlendMode); 11] {
    [
        ("Blend Normal", BlendMode::Normal),
        ("Blend Darken", BlendMode::Darken),
        ("Blend Multiply", BlendMode::Multiply),
        ("Blend Lighten", BlendMode::Lighten),
        ("Blend Screen", BlendMode::Screen),
        ("Blend Color Dodge", BlendMode::ColorDodge),
        ("Blend Linear Dodge", BlendMode::LinearDodge),
        ("Blend Overlay", BlendMode::Overlay),
        ("Blend Soft Light", BlendMode::SoftLight),
        ("Blend Hard Light", BlendMode::HardLight),
        ("Blend Color", BlendMode::Color),
    ]
}

fn checkerboard() -> Vec<u8> {
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let light = ((x / 8) + (y / 8)) % 2 == 0;
            let alpha = ((x + y) * 255 / (WIDTH + HEIGHT - 2)) as u8;
            let pixel = if light {
                [240, 90, 50, alpha]
            } else {
                [30, 110, 230, alpha]
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    rgba
}

fn radial_mask_rgba() -> Vec<u8> {
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    let center = (WIDTH as f32 - 1.0) * 0.5;
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let distance = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            let value = ((1.0 - distance / center).clamp(0.0, 1.0) * 255.0).round() as u8;
            rgba.extend_from_slice(&[value, value, value, 255]);
        }
    }
    rgba
}

fn solid_rgba(color: [u8; 4]) -> Vec<u8> {
    (0..WIDTH * HEIGHT).flat_map(|_| color).collect()
}

fn pixel_data(data: Vec<u8>) -> PixelData {
    PixelData {
        width: WIDTH,
        height: HEIGHT,
        data,
    }
}

fn write_options() -> WriteOptions {
    WriteOptions {
        no_background: Some(true),
        compress: Some(false),
        generate_thumbnail: Some(true),
        ..Default::default()
    }
}

fn find_layer<'a>(psd: &'a Psd, name: &str) -> &'a Layer {
    fn visit<'a>(layers: &'a [Layer], name: &str) -> Option<&'a Layer> {
        for layer in layers {
            if layer.additional_info.name.as_deref() == Some(name) {
                return Some(layer);
            }
            if let Some(found) = layer
                .children
                .as_deref()
                .and_then(|children| visit(children, name))
            {
                return Some(found);
            }
        }
        None
    }

    visit(psd.children.as_deref().unwrap_or_default(), name)
        .unwrap_or_else(|| panic!("missing layer {name:?}"))
}

fn manual_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("compatibility_spike")
        .join("ag_psd_compatibility_spike.psd")
}

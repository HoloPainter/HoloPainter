use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectV1 {
    pub format_version: u32,
    pub project_id: String,
    pub created_with: String,
    pub editor_state: ProjectEditorStateV1,
    pub id_counters: IdCountersV1,
    pub mesh: MeshV1,
    pub materials: Vec<MaterialV1>,
    pub embedded_images: Vec<EmbeddedImageV1>,
    pub layers: Vec<LayerV1>,
    pub surfaces: Vec<SurfaceV1>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectEditorStateV1 {
    pub document_focus: DocumentFocusStateV1,
    pub view: EditorViewStateV1,
    pub layer_tree: LayerTreeEditorStateV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentFocusStateV1 {
    pub active_layer_id: u64,
    pub active_layer_part: ActiveLayerPartV1,
    pub focused_material_id: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorViewStateV1 {
    pub viewport_3d: Viewport3dEditorStateV1,
    pub uv: UvEditorStateV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Viewport3dEditorStateV1 {
    pub camera: Camera3dStateV1,
    pub shading: ViewportShadingV1,
    pub gizmo_visible: bool,
    pub background_color: [f32; 3],
    pub wireframe: WireframeViewStateV1,
    pub scene_visibility: SceneVisibilityStateV1,
    pub surface_mirror: SurfaceMirrorViewStateV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UvEditorStateV1 {
    pub transform: UvViewTransformV1,
    pub background_color: [f32; 3],
    pub wireframe: WireframeViewStateV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WireframeViewStateV1 {
    pub visible: bool,
    pub color: [f32; 3],
    pub opacity: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneVisibilityStateV1 {
    pub hidden_mesh_object_ids: Vec<u64>,
    pub hidden_material_ids: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurfaceMirrorViewStateV1 {
    pub x_enabled: bool,
    pub x_plane: f32,
    pub x_plane_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerTreeEditorStateV1 {
    pub collapsed_layer_group_ids: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActiveLayerPartV1 {
    Content,
    LayerMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera3dStateV1 {
    pub target: [f32; 3],
    pub orientation: [f32; 4],
    pub distance: f32,
    pub projection: CameraProjectionV1,
    pub fov_y_radians: f32,
    pub orthographic_height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraProjectionV1 {
    Perspective,
    Orthographic,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct UvViewTransformV1 {
    pub center_uv: [f32; 2],
    pub zoom: f32,
    pub rotation_radians: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewportShadingV1 {
    Unlit,
    Shade,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdCountersV1 {
    pub mesh_object: u64,
    pub material: u64,
    pub layer: u64,
    pub embedded_image: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddedImageV1 {
    pub id: u64,
    pub file_name: String,
    pub width: u32,
    pub height: u32,
    pub pixel_format: EmbeddedImagePixelFormatV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmbeddedImagePixelFormatV1 {
    Rgba8Straight,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedImageTransformV1 {
    pub center_uv: [f32; 2],
    pub size_uv: [f32; 2],
    pub rotation_radians: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshV1 {
    pub vertex_count: u32,
    pub triangle_count: u64,
    pub position: VertexArrayV1,
    pub normal: VertexArrayV1,
    pub texcoord0: TexcoordArrayV1,
    pub indices: IndexArrayV1,
    pub objects: Vec<MeshObjectV1>,
    pub primitives: Vec<PrimitiveV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexArrayV1 {
    pub offset: u64,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TexcoordArrayV1 {
    pub offset: u64,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexArrayV1 {
    pub offset: u64,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshObjectV1 {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrimitiveV1 {
    pub object_id: u64,
    pub material_id: u64,
    pub first_index: u64,
    pub index_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialV1 {
    pub id: u64,
    pub name: String,
    pub texture_size: TextureSizeV1,
    pub render_settings: MaterialRenderSettingsV1,
    pub export_image_file_name: Option<String>,
    pub export_psd_file_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextureSizeV1 {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialRenderSettingsV1 {
    pub double_sided: bool,
    pub render_mode: MaterialRenderModeV1,
    pub alpha_cutoff: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterialRenderModeV1 {
    Opaque,
    Cutoff,
    Blend,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerV1 {
    pub id: u64,
    pub parent_id: Option<u64>,
    pub order: u32,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub blend_mode: BlendModeV1,
    pub material_mask: Option<Vec<u64>>,
    pub mask: Option<LayerMaskStateV1>,
    pub content: LayerContentV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerMaskStateV1 {
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendModeV1 {
    Normal,
    Darken,
    Multiply,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    Overlay,
    SoftLight,
    HardLight,
    Color,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayerContentV1 {
    Raster,
    EmbeddedImage {
        image_id: u64,
        transform: EmbeddedImageTransformV1,
    },
    SolidFill {
        color: [f32; 3],
    },
    Adjustment {
        adjustment: AdjustmentV1,
    },
    Group {
        composite_mode: GroupCompositeModeV1,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupCompositeModeV1 {
    Isolated,
    PassThrough,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AdjustmentV1 {
    BrightnessContrast {
        brightness: i16,
        contrast: i16,
    },
    Levels {
        master: LevelsChannelV1,
        red: LevelsChannelV1,
        green: LevelsChannelV1,
        blue: LevelsChannelV1,
    },
    Curves {
        master: Vec<CurvePointV1>,
        red: Vec<CurvePointV1>,
        green: Vec<CurvePointV1>,
        blue: Vec<CurvePointV1>,
    },
    HueSaturation {
        hue: i16,
        saturation: i16,
        lightness: i16,
    },
    Invert,
    GradientMap {
        reverse: bool,
        dither: bool,
        stops: Vec<GradientStopV1>,
    },
    UvMirror {
        axis: UvAxisV1,
        position: f32,
        direction: UvMirrorDirectionV1,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LevelsChannelV1 {
    pub input_black: u8,
    pub input_white: u8,
    pub gamma: f32,
    pub output_black: u8,
    pub output_white: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CurvePointV1 {
    pub input: u8,
    pub output: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStopV1 {
    pub location: u16,
    pub midpoint: u16,
    pub color: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UvAxisV1 {
    X,
    Y,
}

#[cfg(test)]
mod adjustment_tests {
    use super::*;

    #[test]
    fn photoshop_adjustment_units_round_trip_without_conversion() {
        let adjustments = vec![
            AdjustmentV1::BrightnessContrast {
                brightness: 120,
                contrast: -40,
            },
            AdjustmentV1::Levels {
                master: LevelsChannelV1 {
                    input_black: 12,
                    input_white: 240,
                    gamma: 2.2,
                    output_black: 7,
                    output_white: 248,
                },
                red: LevelsChannelV1 {
                    input_black: 0,
                    input_white: 255,
                    gamma: 1.0,
                    output_black: 0,
                    output_white: 255,
                },
                green: LevelsChannelV1 {
                    input_black: 0,
                    input_white: 255,
                    gamma: 1.0,
                    output_black: 0,
                    output_white: 255,
                },
                blue: LevelsChannelV1 {
                    input_black: 0,
                    input_white: 255,
                    gamma: 1.0,
                    output_black: 0,
                    output_white: 255,
                },
            },
            AdjustmentV1::Curves {
                master: vec![
                    CurvePointV1 {
                        input: 0,
                        output: 0,
                    },
                    CurvePointV1 {
                        input: 128,
                        output: 144,
                    },
                    CurvePointV1 {
                        input: 255,
                        output: 255,
                    },
                ],
                red: vec![
                    CurvePointV1 {
                        input: 0,
                        output: 0,
                    },
                    CurvePointV1 {
                        input: 255,
                        output: 255,
                    },
                ],
                green: vec![
                    CurvePointV1 {
                        input: 0,
                        output: 0,
                    },
                    CurvePointV1 {
                        input: 255,
                        output: 255,
                    },
                ],
                blue: vec![
                    CurvePointV1 {
                        input: 0,
                        output: 0,
                    },
                    CurvePointV1 {
                        input: 255,
                        output: 255,
                    },
                ],
            },
            AdjustmentV1::HueSaturation {
                hue: 20,
                saturation: 30,
                lightness: 10,
            },
            AdjustmentV1::GradientMap {
                reverse: true,
                dither: true,
                stops: vec![
                    GradientStopV1 {
                        location: 0,
                        midpoint: 50,
                        color: [0, 0, 0],
                    },
                    GradientStopV1 {
                        location: 4096,
                        midpoint: 35,
                        color: [255, 240, 220],
                    },
                ],
            },
        ];
        for adjustment in adjustments {
            let source = ron::ser::to_string(&adjustment).unwrap();
            let decoded: AdjustmentV1 = ron::from_str(&source).unwrap();
            assert_eq!(decoded, adjustment);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UvMirrorDirectionV1 {
    PositiveToNegative,
    NegativeToPositive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceV1 {
    pub layer_id: u64,
    pub material_id: u64,
    pub role: SurfaceRoleV1,
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormatV1,
    pub tile_size: u32,
    pub default_fill: SurfaceDefaultV1,
    pub tiles: Vec<TileV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SurfaceRoleV1 {
    Raster,
    LayerMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormatV1 {
    Rgba8Unorm,
    R8Unorm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SurfaceDefaultV1 {
    TransparentBlack,
    MaskHidden,
    MaskRevealed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileV1 {
    pub x: u32,
    pub y: u32,
    pub compressed_size: u32,
}

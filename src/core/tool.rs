use crate::core::{
    brush_engine::BrushEngineRegistry, brush_preset::BrushPresetCatalog,
    composite::SelectionCompositeMode, stroke::StrokeSpace, stroke_preset::StrokeToolPreset,
    texture::TextureCatalog, tool_layout::ToolLayoutFileV1,
};

const UV_AND_SURFACE: &[StrokeSpace] = &[StrokeSpace::Uv, StrokeSpace::Surface];
const UV_ONLY: &[StrokeSpace] = &[StrokeSpace::Uv];
const SURFACE_ONLY: &[StrokeSpace] = &[StrokeSpace::Surface];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolSection {
    Paint,
    Filter,
    Fill,
    Shape,
    Select,
    Transform,
    View,
    Navigation,
    Sampler,
}

impl ToolSection {
    pub const fn toolbox_order() -> &'static [ToolSection] {
        &[
            ToolSection::Paint,
            ToolSection::Filter,
            ToolSection::Fill,
            ToolSection::Shape,
            ToolSection::Select,
            ToolSection::Transform,
            ToolSection::View,
            ToolSection::Navigation,
            ToolSection::Sampler,
        ]
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Paint => "paint",
            Self::Filter => "filter",
            Self::Fill => "fill",
            Self::Shape => "shape",
            Self::Select => "select",
            Self::Transform => "transform",
            Self::View => "view",
            Self::Navigation => "navigation",
            Self::Sampler => "sampler",
        }
    }
    pub const fn default_name(self) -> &'static str {
        match self {
            Self::Paint => "Paint",
            Self::Filter => "Filter",
            Self::Fill => "Fill",
            Self::Shape => "Shape",
            Self::Select => "Select",
            Self::Transform => "Transform",
            Self::View => "View",
            Self::Navigation => "Navigation",
            Self::Sampler => "Sampler",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolId {
    EmptyGroup(usize),
    SurfaceDecal,
    ViewProjectionDecal,
    FillMaterial,
    FillMesh,
    FillPolygon,
    RectanglePaint,
    RectangleErase,
    RectangleSelection,
    LassoPaint,
    LassoErase,
    LassoSelection,
    Transform,
    ColorPicker,
    BrushPreset(usize),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub id: ToolId,
    pub config_id: String,
    pub name: String,
    pub section: ToolSection,
    pub behavior: ToolBehavior,
    pub supported_spaces: Vec<StrokeSpace>,
}

impl ToolDefinition {
    pub fn is_stroke(&self) -> bool {
        matches!(self.behavior, ToolBehavior::Stroke { .. })
    }

    pub fn surface_hit_requirement(&self) -> SurfaceHitRequirement {
        match self.behavior {
            ToolBehavior::NoOp => SurfaceHitRequirement::None,
            ToolBehavior::Stroke { .. } | ToolBehavior::Fill { .. } => {
                SurfaceHitRequirement::Required
            }
            ToolBehavior::Decal {
                kind: DecalKind::Surface,
            } => SurfaceHitRequirement::Required,
            ToolBehavior::Shape { .. }
            | ToolBehavior::Selection { .. }
            | ToolBehavior::Decal {
                kind: DecalKind::ViewProjection,
            }
            | ToolBehavior::Transform => SurfaceHitRequirement::None,
            ToolBehavior::ColorPicker => SurfaceHitRequirement::Optional,
        }
    }

    pub fn supports_space(&self, space: StrokeSpace) -> bool {
        self.supported_spaces.contains(&space)
    }

    pub fn supports_surface_mirror(&self) -> bool {
        if !self.supports_space(StrokeSpace::Surface) {
            return false;
        }
        match &self.behavior {
            ToolBehavior::NoOp => false,
            ToolBehavior::Stroke { .. } => true,
            ToolBehavior::Shape {
                shape: ShapeToolKind::Rectangle,
                ..
            } => true,
            ToolBehavior::Shape {
                shape: ShapeToolKind::Lasso,
                ..
            } => true,
            ToolBehavior::Fill { .. }
            | ToolBehavior::Selection { .. }
            | ToolBehavior::Decal { .. }
            | ToolBehavior::Transform
            | ToolBehavior::ColorPicker => false,
        }
    }

    pub fn edits_paint_target(&self) -> bool {
        matches!(
            &self.behavior,
            ToolBehavior::Stroke { .. }
                | ToolBehavior::Fill { .. }
                | ToolBehavior::Shape { .. }
                | ToolBehavior::Decal { .. }
                | ToolBehavior::Transform
        )
    }

    pub fn edits_paint_target_in_space(&self, space: StrokeSpace) -> bool {
        match &self.behavior {
            ToolBehavior::Stroke { .. } | ToolBehavior::Shape { .. } => self.supports_space(space),
            ToolBehavior::Fill { .. } => self.supports_space(space),
            ToolBehavior::Decal { .. } | ToolBehavior::Transform => self.supports_space(space),
            ToolBehavior::Selection { .. } | ToolBehavior::ColorPicker => false,
            ToolBehavior::NoOp => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ToolBehavior {
    NoOp,
    Stroke {
        preset_index: usize,
    },
    Fill {
        scope: FillScope,
    },
    Shape {
        shape: ShapeToolKind,
        paint_mode: ShapePaintMode,
    },
    Selection {
        shape: SelectionShapeKind,
    },
    Decal {
        kind: DecalKind,
    },
    Transform,
    ColorPicker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecalKind {
    Surface,
    ViewProjection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillScope {
    Material,
    Mesh,
    Polygon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceHitRequirement {
    None,
    Required,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeToolKind {
    Rectangle,
    Lasso,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapePaintMode {
    Paint,
    Erase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionShapeKind {
    Rectangle,
    Lasso,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorSampleSource {
    #[default]
    View,
    CompositeTexture,
    CurrentLayer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorPickerToolOptions {
    pub source: ColorSampleSource,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FillToolOptions {
    pub opacity: f32,
}

impl Default for FillToolOptions {
    fn default() -> Self {
        Self { opacity: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapeToolOptions {
    pub opacity: f32,
}

impl Default for ShapeToolOptions {
    fn default() -> Self {
        Self { opacity: 1.0 }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionToolOptions {
    pub operation: SelectionCompositeMode,
}

impl Default for SelectionToolOptions {
    fn default() -> Self {
        Self {
            operation: SelectionCompositeMode::Replace,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DefaultTools {
    pub presets: Vec<StrokeToolPreset>,
    pub default_brush_preset: StrokeToolPreset,
    pub catalog: Vec<ToolDefinition>,
    pub shelf: ToolShelf,
    pub(crate) brush_engines: BrushEngineRegistry,
    pub(crate) brush_presets: BrushPresetCatalog,
    pub(crate) brush_textures: TextureCatalog,
    pub(crate) tool_layout: ToolLayoutFileV1,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolShelf {
    pub groups: Vec<ToolGroup>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolGroup {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    pub entries: Vec<ToolEntry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolEntry {
    pub tool_id: ToolId,
    pub name: String,
}

pub fn default_tools() -> DefaultTools {
    crate::core::tool_catalog::load_default_tools_from_resources()
        .expect("loading brush tool catalog")
}

pub(crate) fn builtin_surface_decal_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::SurfaceDecal,
        config_id: "builtin.tool.decal.surface".to_owned(),
        name: "Surface Decal".to_owned(),
        section: ToolSection::Paint,
        behavior: ToolBehavior::Decal {
            kind: DecalKind::Surface,
        },
        supported_spaces: SURFACE_ONLY.to_vec(),
    }
}

pub(crate) fn builtin_view_projection_decal_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::ViewProjectionDecal,
        config_id: "builtin.tool.decal.view_projection".to_owned(),
        name: "View Projection Decal".to_owned(),
        section: ToolSection::Paint,
        behavior: ToolBehavior::Decal {
            kind: DecalKind::ViewProjection,
        },
        supported_spaces: SURFACE_ONLY.to_vec(),
    }
}

pub(crate) fn builtin_fill_material_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::FillMaterial,
        config_id: "builtin.tool.fill.material".to_owned(),
        name: "Fill Material".to_owned(),
        section: ToolSection::Fill,
        behavior: ToolBehavior::Fill {
            scope: FillScope::Material,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_fill_mesh_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::FillMesh,
        config_id: "builtin.tool.fill.mesh".to_owned(),
        name: "Fill Mesh".to_owned(),
        section: ToolSection::Fill,
        behavior: ToolBehavior::Fill {
            scope: FillScope::Mesh,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_fill_polygon_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::FillPolygon,
        config_id: "builtin.tool.fill.polygon".to_owned(),
        name: "Fill Polygon".to_owned(),
        section: ToolSection::Fill,
        behavior: ToolBehavior::Fill {
            scope: FillScope::Polygon,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_rectangle_paint_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::RectanglePaint,
        config_id: "builtin.tool.shape.rectangle_paint".to_owned(),
        name: "Rectangle Paint".to_owned(),
        section: ToolSection::Shape,
        behavior: ToolBehavior::Shape {
            shape: ShapeToolKind::Rectangle,
            paint_mode: ShapePaintMode::Paint,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_rectangle_erase_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::RectangleErase,
        config_id: "builtin.tool.shape.rectangle_erase".to_owned(),
        name: "Rectangle Erase".to_owned(),
        section: ToolSection::Shape,
        behavior: ToolBehavior::Shape {
            shape: ShapeToolKind::Rectangle,
            paint_mode: ShapePaintMode::Erase,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_rectangle_selection_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::RectangleSelection,
        config_id: "builtin.tool.selection.rectangle".to_owned(),
        name: "Rectangle Selection".to_owned(),
        section: ToolSection::Select,
        behavior: ToolBehavior::Selection {
            shape: SelectionShapeKind::Rectangle,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_lasso_paint_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::LassoPaint,
        config_id: "builtin.tool.shape.lasso_paint".to_owned(),
        name: "Lasso Paint".to_owned(),
        section: ToolSection::Shape,
        behavior: ToolBehavior::Shape {
            shape: ShapeToolKind::Lasso,
            paint_mode: ShapePaintMode::Paint,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_lasso_erase_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::LassoErase,
        config_id: "builtin.tool.shape.lasso_erase".to_owned(),
        name: "Lasso Erase".to_owned(),
        section: ToolSection::Shape,
        behavior: ToolBehavior::Shape {
            shape: ShapeToolKind::Lasso,
            paint_mode: ShapePaintMode::Erase,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_transform_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::Transform,
        config_id: "builtin.tool.transform".to_owned(),
        name: "Transform".to_owned(),
        section: ToolSection::Transform,
        behavior: ToolBehavior::Transform,
        supported_spaces: UV_ONLY.to_vec(),
    }
}

pub(crate) fn builtin_color_picker_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::ColorPicker,
        config_id: "builtin.tool.sampler.color_picker".to_owned(),
        name: "Color Picker".to_owned(),
        section: ToolSection::Sampler,
        behavior: ToolBehavior::ColorPicker,
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

pub(crate) fn builtin_lasso_selection_tool() -> ToolDefinition {
    ToolDefinition {
        id: ToolId::LassoSelection,
        config_id: "builtin.tool.selection.lasso".to_owned(),
        name: "Lasso Selection".to_owned(),
        section: ToolSection::Select,
        behavior: ToolBehavior::Selection {
            shape: SelectionShapeKind::Lasso,
        },
        supported_spaces: UV_AND_SURFACE.to_vec(),
    }
}

impl ToolShelf {
    pub fn from_catalog(catalog: &[ToolDefinition]) -> Self {
        let groups = ToolSection::toolbox_order()
            .iter()
            .filter_map(|section| {
                let entries: Vec<_> = catalog
                    .iter()
                    .filter(|tool| tool.section == *section)
                    .map(|tool| ToolEntry {
                        tool_id: tool.id,
                        name: tool.name.clone(),
                    })
                    .collect();
                (!entries.is_empty()).then(|| ToolGroup {
                    id: format!("builtin.section.{}", section.id()),
                    name: section.default_name().to_owned(),
                    icon: None,
                    entries,
                })
            })
            .collect();
        Self { groups }
    }

    pub fn group_containing(&self, tool_id: ToolId) -> Option<usize> {
        if let ToolId::EmptyGroup(index) = tool_id {
            return self.groups.get(index).map(|_| index);
        }
        self.groups
            .iter()
            .position(|group| group.entries.iter().any(|entry| entry.tool_id == tool_id))
    }
}

pub(crate) fn empty_group_config_id(group_id: &str) -> String {
    format!("runtime.empty_group.{group_id}")
}

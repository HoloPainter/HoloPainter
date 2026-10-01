use glam::{Mat4, Vec2};

use crate::core::{
    camera::OrbitCamera,
    composite::{GroupCompositeMode, LayerBlendMode, SelectionCompositeMode},
    decal::{DecalImageAsset, DecalToolOptions},
    document::{ImportedAsset, MeshId, SurfaceHit},
    image::MaterialPayload,
    stroke::StrokeSpace,
    stroke_preset::StrokeToolPreset,
    surface::{LayerMaskInitMode, LayerMaterialMask},
    tool::{
        ColorPickerToolOptions, FillToolOptions, SelectionToolOptions, ShapeToolOptions, ToolId,
    },
    uv_view::UvViewTransform,
    viewport_shading::ViewportShading,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputHoldToken(pub u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputModifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub command: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportInputContext {
    pub size: [u32; 2],
    pub view_proj: Mat4,
    pub inv_view_proj: Mat4,
    pub camera_world: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvViewInputContext {
    pub size: [u32; 2],
    pub canvas_size: [u32; 2],
    pub transform: UvViewTransform,
}

impl UvViewInputContext {
    pub fn gesture_view_size(self) -> [u32; 2] {
        [
            ((self.canvas_size[0].max(1) as f32 * self.transform.zoom).round() as u32).max(1),
            ((self.canvas_size[1].max(1) as f32 * self.transform.zoom).round() as u32).max(1),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewPointerPhase {
    Click,
    Down,
    Move,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCancelReason {
    Escape,
    FocusLost,
    PointerCaptureLost,
    TouchCancelled,
    ToolChanged,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewPointerMeta {
    pub phase: ViewPointerPhase,
    pub position_px: Vec2,
    pub pressure: f32,
    pub time_s: f64,
    pub modifiers: InputModifiers,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewPointerEvent {
    Viewport3d {
        meta: ViewPointerMeta,
        view: ViewportInputContext,
    },
    Uv {
        meta: ViewPointerMeta,
        view: UvViewInputContext,
    },
    Cancel {
        reason: ToolCancelReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerSampleMeta {
    pub pressure: f32,
    pub time_s: f64,
    pub modifiers: InputModifiers,
    /// Input-filter position in screen pixels for every stroke space.
    pub screen_px: Vec2,
}

impl PointerSampleMeta {
    pub fn new(screen_px: Vec2, pressure: f32, time_s: f64, modifiers: InputModifiers) -> Self {
        Self {
            pressure: pressure.clamp(0.0, 1.0),
            time_s,
            modifiers,
            screen_px,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PointerSample {
    pub meta: PointerSampleMeta,
    pub kind: PointerSampleKind,
}

#[derive(Debug, Clone, Copy)]
pub enum PointerSampleKind {
    Uv {
        uv: Vec2,
        view: UvViewInputContext,
    },
    Surface {
        view: ViewportInputContext,
        hit: Option<SurfaceHit>,
    },
}

impl PointerSample {
    pub fn uv(position: Vec2, modifiers: InputModifiers) -> Self {
        Self::uv_with_pressure(position, 1.0, modifiers)
    }

    pub fn uv_with_pressure(position: Vec2, pressure: f32, modifiers: InputModifiers) -> Self {
        Self::uv_with_pressure_at(position, position, [1, 1], pressure, 0.0, modifiers)
    }

    pub fn uv_with_pressure_at(
        uv: Vec2,
        screen_px: Vec2,
        view_size: [u32; 2],
        pressure: f32,
        time_s: f64,
        modifiers: InputModifiers,
    ) -> Self {
        Self::uv_with_context_at(
            uv,
            screen_px,
            UvViewInputContext {
                size: view_size,
                canvas_size: view_size,
                transform: UvViewTransform::default(),
            },
            pressure,
            time_s,
            modifiers,
        )
    }

    pub fn uv_with_context_at(
        uv: Vec2,
        screen_px: Vec2,
        view: UvViewInputContext,
        pressure: f32,
        time_s: f64,
        modifiers: InputModifiers,
    ) -> Self {
        Self {
            meta: PointerSampleMeta::new(screen_px, pressure, time_s, modifiers),
            kind: PointerSampleKind::Uv { uv, view },
        }
    }

    pub fn surface_with_pressure(
        position: Vec2,
        pressure: f32,
        modifiers: InputModifiers,
        view: ViewportInputContext,
        surface_hit: SurfaceHit,
    ) -> Self {
        Self::surface_with_pressure_at(position, pressure, 0.0, modifiers, view, surface_hit)
    }

    pub fn surface_with_pressure_at(
        position: Vec2,
        pressure: f32,
        time_s: f64,
        modifiers: InputModifiers,
        view: ViewportInputContext,
        surface_hit: SurfaceHit,
    ) -> Self {
        Self {
            meta: PointerSampleMeta::new(position, pressure, time_s, modifiers),
            kind: PointerSampleKind::Surface {
                view,
                hit: Some(surface_hit),
            },
        }
    }

    pub fn surface_without_hit_with_pressure(
        position: Vec2,
        pressure: f32,
        modifiers: InputModifiers,
        view: ViewportInputContext,
    ) -> Self {
        Self::surface_without_hit_with_pressure_at(position, pressure, 0.0, modifiers, view)
    }

    pub fn surface_without_hit_with_pressure_at(
        position: Vec2,
        pressure: f32,
        time_s: f64,
        modifiers: InputModifiers,
        view: ViewportInputContext,
    ) -> Self {
        Self {
            meta: PointerSampleMeta::new(position, pressure, time_s, modifiers),
            kind: PointerSampleKind::Surface { view, hit: None },
        }
    }

    pub fn space(&self) -> StrokeSpace {
        match self.kind {
            PointerSampleKind::Uv { .. } => StrokeSpace::Uv,
            PointerSampleKind::Surface { .. } => StrokeSpace::Surface,
        }
    }

    pub fn position(&self) -> Vec2 {
        match self.kind {
            PointerSampleKind::Uv { uv, .. } => uv,
            PointerSampleKind::Surface { .. } => self.meta.screen_px,
        }
    }

    pub fn screen_px(&self) -> Vec2 {
        self.meta.screen_px
    }

    pub fn uv_view_size(&self) -> Option<[u32; 2]> {
        match self.kind {
            PointerSampleKind::Uv { view, .. } => Some(view.size),
            PointerSampleKind::Surface { .. } => None,
        }
    }

    pub fn uv_view_context(&self) -> Option<UvViewInputContext> {
        match self.kind {
            PointerSampleKind::Uv { view, .. } => Some(view),
            PointerSampleKind::Surface { .. } => None,
        }
    }

    pub fn pressure(&self) -> f32 {
        self.meta.pressure
    }

    pub fn time_s(&self) -> f64 {
        self.meta.time_s
    }

    pub fn modifiers(&self) -> InputModifiers {
        self.meta.modifiers
    }

    pub fn viewport_context(&self) -> Option<ViewportInputContext> {
        match self.kind {
            PointerSampleKind::Surface { view, .. } => Some(view),
            PointerSampleKind::Uv { .. } => None,
        }
    }

    pub fn surface_hit(&self) -> Option<SurfaceHit> {
        match self.kind {
            PointerSampleKind::Surface { hit, .. } => hit,
            PointerSampleKind::Uv { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ToolInputEvent {
    PointerDown(PointerSample),
    PointerMove(PointerSample),
    PointerUp(PointerSample),
    Cancel { reason: ToolCancelReason },
}

#[derive(Debug, Clone)]
pub enum Command {
    BeginEmbeddedImageImport {
        image: super::EmbeddedImageImportPayload,
    },
    BeginImportedImageTransform {
        image: super::ImportedImagePayload,
    },
    PasteImageAsLayer {
        image: super::ImportedImagePayload,
    },
    CutSurfacePixels {
        target: crate::core::surface::PaintSurfaceId,
    },
    DeleteSelectedPixels,
    PasteLayerMask {
        layer_id: crate::core::surface::LayerId,
        material_index: usize,
        payload: super::LayerMaskClipboardPayload,
    },
    AssetLoaded {
        asset: ImportedAsset,
        materials: Vec<MaterialPayload>,
    },
    ProjectLoaded(crate::project::LoadedProject),
    ReloadMesh {
        request: super::MeshReloadRequest,
    },
    SetFocusedMaterial(usize),
    SetMaterialRenderSettings {
        material_index: usize,
        settings: crate::core::material::MaterialRenderSettings,
    },
    ResizeMaterialTexture {
        material_index: usize,
        texture_size: [u32; 2],
    },
    SetMaterialExportImageFileNames {
        names: Vec<(crate::core::material::MaterialId, String)>,
    },
    SetMaterialExportPsdFileNames {
        names: Vec<(crate::core::material::MaterialId, String)>,
    },
    SetTheme(crate::settings::AppTheme),
    SetLanguage(crate::settings::LanguagePreference),
    SetTabletBackend(crate::settings::TabletBackend),
    SetTabletPressureCurve(crate::core::curve::Curve),
    SetViewportGizmoVisible(bool),
    SetViewportShading(ViewportShading),
    SetViewportWireframeVisible(bool),
    SetViewportWireframeStyle(crate::core::wireframe::WireframeStyle),
    SetViewportBackgroundColor([f32; 3]),
    SetViewportMeshVisible {
        mesh_id: MeshId,
        visible: bool,
    },
    SetViewportMaterialVisible {
        material_index: usize,
        visible: bool,
    },
    SetUvWireframeVisible(bool),
    SetUvWireframeStyle(crate::core::wireframe::WireframeStyle),
    SetUvViewBackgroundColor([f32; 3]),
    SetStatusMessage(super::StatusMessage),
    SetCurrentColor([f32; 3]),
    SetDecalImage(std::sync::Arc<DecalImageAsset>),
    ClearDecalImage,
    UpdateDecalToolOptions(DecalToolOptions),
    BeginViewProjectionDecal {
        viewport_size: [u32; 2],
    },
    RequestViewProjectionDecal,
    SyncViewProjectionDecalViewport {
        viewport_size: [u32; 2],
    },
    ApplyActiveDecal,
    CancelActiveDecal,
    SetActiveTool(ToolId),
    BeginMomentaryTool {
        token: InputHoldToken,
        tool_id: ToolId,
    },
    CompleteMomentaryTool {
        token: InputHoldToken,
        tool_id: ToolId,
        select_tool: bool,
    },
    BeginTransformMode,
    BeginTransientToolOverride {
        token: InputHoldToken,
        id: String,
    },
    EndTransientToolOverride {
        token: InputHoldToken,
    },
    SetActiveToolPresetIndex(usize),
    UpdateActiveToolPreset(StrokeToolPreset),
    UpdateFillToolOptions(FillToolOptions),
    UpdateShapeToolOptions(ShapeToolOptions),
    UpdateSelectionToolOptions(SelectionToolOptions),
    UpdateColorPickerToolOptions(ColorPickerToolOptions),
    SetSurfaceMirrorXEnabled(bool),
    SetSurfaceMirrorXPlane(f32),
    SetSurfaceMirrorXPlaneVisible(bool),
    UpdateRuntimeStrokePreset {
        preset_index: usize,
        preset: StrokeToolPreset,
    },
    SetRuntimeBrushEngine {
        preset_index: usize,
        preset_id: String,
        engine_id: String,
    },
    ResetBrushPreset {
        preset_index: usize,
        preset_id: String,
    },
    CreateBrushPreset {
        group_id: String,
        source_preset_id: String,
        new_preset_id: String,
        preset: StrokeToolPreset,
    },
    DeleteBrushPreset {
        preset_id: String,
    },
    MoveToolGroup {
        from_index: usize,
        to_index: usize,
    },
    MoveToolEntry {
        group_id: String,
        from_index: usize,
        to_index: usize,
    },
    SetCamera(OrbitCamera),
    ResetViewportCamera,
    FrameAllViewport,
    SetUvViewTransform(UvViewTransform),
    ResetUvViewTransform,
    ViewPointer(ViewPointerEvent),
    ToolInput(ToolInputEvent),
    AdvanceActiveStroke {
        time_s: f64,
    },
    ApplyActiveTransform,
    CancelActiveTransform,
    BeginTransformNumericEdit,
    UpdateTransformNumeric(TransformNumericEdit),
    CommitTransformNumericEdit,
    FillMaterial {
        material_index: usize,
        opacity: f32,
    },
    FillMesh {
        mesh_id: MeshId,
        opacity: f32,
    },
    RectanglePaintUv {
        min_uv: Vec2,
        max_uv: Vec2,
        opacity: f32,
    },
    RectangleEraseUv {
        min_uv: Vec2,
        max_uv: Vec2,
        opacity: f32,
    },
    RectangleSelectUv {
        min_uv: Vec2,
        max_uv: Vec2,
        operation: SelectionCompositeMode,
    },
    SelectAll,
    InvertSelection,
    DeselectSelection,
    SelectFromLayerTransparency {
        layer_id: crate::core::surface::LayerId,
    },
    ApplySurfaceBlur {
        radius_px: f32,
    },
    ApplySpatialBlur {
        radius_px: f32,
        cross_meshes: bool,
        orientation: crate::core::surface_filter::SpatialBlurOrientation,
        maximum_normal_angle_degrees: f32,
    },
    ApplyAdjustmentFilter {
        layer_id: crate::core::surface::LayerId,
        adjustment: crate::core::adjustment::Adjustment,
    },
    BeginAdjustmentFilterSession {
        layer_id: crate::core::surface::LayerId,
        kind: crate::core::adjustment::AdjustmentKind,
    },
    PreviewAdjustmentFilter {
        layer_id: crate::core::surface::LayerId,
        adjustment: Option<crate::core::adjustment::Adjustment>,
    },
    CommitAdjustmentFilterSession {
        layer_id: crate::core::surface::LayerId,
        adjustment: crate::core::adjustment::Adjustment,
    },
    CancelAdjustmentFilterSession {
        layer_id: crate::core::surface::LayerId,
    },
    Undo,
    Redo,
    ClearDocument,
    AddLayer,
    AddSolidFillLayer {
        color: [f32; 3],
    },
    AddAdjustmentLayer {
        kind: crate::core::adjustment::AdjustmentKind,
    },
    AddGroup,
    DeleteLayer {
        layer_id: crate::core::surface::LayerId,
    },
    DeleteLayers {
        layer_ids: Vec<crate::core::surface::LayerId>,
    },
    DuplicateLayer {
        layer_id: crate::core::surface::LayerId,
    },
    DuplicateLayers {
        layer_ids: Vec<crate::core::surface::LayerId>,
        primary_layer_id: crate::core::surface::LayerId,
    },
    MergeLayers {
        layer_ids: Vec<crate::core::surface::LayerId>,
    },
    RasterizeLayer {
        layer_id: crate::core::surface::LayerId,
    },
    ApplyLayerMask {
        layer_id: crate::core::surface::LayerId,
    },
    AddLayerMask {
        layer_id: crate::core::surface::LayerId,
        mode: LayerMaskInitMode,
    },
    AddLayerMaskFromSelection {
        layer_id: crate::core::surface::LayerId,
    },
    DeleteLayerMask {
        layer_id: crate::core::surface::LayerId,
    },
    MoveLayerMask {
        source_layer_id: crate::core::surface::LayerId,
        target_layer_id: crate::core::surface::LayerId,
    },
    SelectLayer {
        layer_id: crate::core::surface::LayerId,
    },
    SelectSolidFillLayer {
        layer_id: crate::core::surface::LayerId,
    },
    SelectAdjustmentLayer {
        layer_id: crate::core::surface::LayerId,
    },
    SelectLayerMask {
        layer_id: crate::core::surface::LayerId,
    },
    SetLayerMaskEnabled {
        layer_id: crate::core::surface::LayerId,
        enabled: bool,
    },
    SelectLayerForStructure {
        layer_id: crate::core::surface::LayerId,
    },
    RenameLayer {
        layer_id: crate::core::surface::LayerId,
        name: String,
    },
    SetLayerVisible {
        layer_id: crate::core::surface::LayerId,
        visible: bool,
    },
    SetLayerLocked {
        layer_id: crate::core::surface::LayerId,
        locked: bool,
    },
    SetLayersLocked {
        layer_ids: Vec<crate::core::surface::LayerId>,
        locked: bool,
    },
    SetLayerMaterialMask {
        layer_id: crate::core::surface::LayerId,
        material_mask: LayerMaterialMask,
    },
    SetEmbeddedImageTransform {
        layer_id: crate::core::surface::LayerId,
        transform: crate::core::embedded_image::EmbeddedImageTransform,
    },
    SetLayerOpacity {
        layer_id: crate::core::surface::LayerId,
        opacity: f32,
    },
    SetSolidFillColor {
        layer_id: crate::core::surface::LayerId,
        color: [f32; 3],
        edit_session: u64,
    },
    SetAdjustment {
        layer_id: crate::core::surface::LayerId,
        adjustment: crate::core::adjustment::Adjustment,
        edit_session: u64,
    },
    SetLayersOpacity {
        layer_ids: Vec<crate::core::surface::LayerId>,
        opacity: f32,
    },
    SetLayerBlendMode {
        layer_id: crate::core::surface::LayerId,
        blend_mode: LayerBlendMode,
    },
    SetLayersBlendMode {
        layer_ids: Vec<crate::core::surface::LayerId>,
        blend_mode: LayerBlendMode,
    },
    SetLayerGroupCompositeMode {
        layer_id: crate::core::surface::LayerId,
        mode: GroupCompositeMode,
    },
    SetLayersGroupCompositeMode {
        layer_ids: Vec<crate::core::surface::LayerId>,
        mode: GroupCompositeMode,
    },
    MoveLayer {
        layer_id: crate::core::surface::LayerId,
        new_parent: crate::core::surface::LayerId,
        new_index: usize,
    },
    MoveLayers {
        layer_ids: Vec<crate::core::surface::LayerId>,
        new_parent: crate::core::surface::LayerId,
        new_index: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandDomain {
    Import,
    DocumentReset,
    Document,
    RendererSettings,
    Material,
    Settings,
    View,
    Status,
    Tool,
    InteractionCleanup,
    Transform,
    Paint,
    Selection,
    Filter,
    AdjustmentSessionControl,
    History,
    Layer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandBlocker {
    RenderCommitPending,
    FatalRendererError,
    AdjustmentFilterSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CommandPolicy {
    domain: CommandDomain,
}

impl CommandPolicy {
    pub(crate) fn domain(self) -> CommandDomain {
        self.domain
    }

    pub(crate) fn allowed_during(self, blocker: CommandBlocker) -> bool {
        use CommandBlocker::{AdjustmentFilterSession, FatalRendererError, RenderCommitPending};
        use CommandDomain::{
            AdjustmentSessionControl, InteractionCleanup, RendererSettings, Settings, Status, View,
        };

        matches!(
            (self.domain, blocker),
            (View | Status | InteractionCleanup | Settings, _)
                | (RendererSettings, RenderCommitPending | FatalRendererError)
                | (AdjustmentSessionControl, AdjustmentFilterSession)
        )
    }

    pub(crate) fn clears_pending_transaction(self) -> bool {
        matches!(
            self.domain,
            CommandDomain::DocumentReset | CommandDomain::History
        )
    }
}

impl Command {
    pub(crate) fn policy(&self) -> CommandPolicy {
        use CommandDomain::*;

        let domain = match self {
            Self::BeginEmbeddedImageImport { .. }
            | Self::BeginImportedImageTransform { .. }
            | Self::PasteImageAsLayer { .. } => Import,

            Self::AssetLoaded { .. }
            | Self::ProjectLoaded(_)
            | Self::ReloadMesh { .. }
            | Self::ClearDocument => DocumentReset,
            Self::SetFocusedMaterial(_) => Document,
            Self::SetMaterialRenderSettings { .. } => RendererSettings,
            Self::ResizeMaterialTexture { .. }
            | Self::SetMaterialExportImageFileNames { .. }
            | Self::SetMaterialExportPsdFileNames { .. } => Material,
            Self::SetTheme(_)
            | Self::SetLanguage(_)
            | Self::SetTabletBackend(_)
            | Self::SetTabletPressureCurve(_) => Settings,

            Self::SetViewportGizmoVisible(_)
            | Self::SetViewportShading(_)
            | Self::SetViewportWireframeVisible(_)
            | Self::SetViewportWireframeStyle(_)
            | Self::SetViewportBackgroundColor(_)
            | Self::SetViewportMeshVisible { .. }
            | Self::SetViewportMaterialVisible { .. }
            | Self::SetUvWireframeVisible(_)
            | Self::SetUvWireframeStyle(_)
            | Self::SetUvViewBackgroundColor(_)
            | Self::SetCamera(_)
            | Self::ResetViewportCamera
            | Self::FrameAllViewport
            | Self::SetUvViewTransform(_)
            | Self::ResetUvViewTransform => View,
            Self::SetStatusMessage(_) => Status,

            Self::CompleteMomentaryTool { .. } | Self::EndTransientToolOverride { .. } => {
                InteractionCleanup
            }
            Self::SetCurrentColor(_)
            | Self::SetDecalImage(_)
            | Self::ClearDecalImage
            | Self::UpdateDecalToolOptions(_)
            | Self::BeginViewProjectionDecal { .. }
            | Self::RequestViewProjectionDecal
            | Self::SyncViewProjectionDecalViewport { .. }
            | Self::ApplyActiveDecal
            | Self::CancelActiveDecal
            | Self::SetActiveTool(_)
            | Self::BeginMomentaryTool { .. }
            | Self::BeginTransientToolOverride { .. }
            | Self::SetActiveToolPresetIndex(_)
            | Self::UpdateActiveToolPreset(_)
            | Self::UpdateFillToolOptions(_)
            | Self::UpdateShapeToolOptions(_)
            | Self::UpdateSelectionToolOptions(_)
            | Self::UpdateColorPickerToolOptions(_)
            | Self::SetSurfaceMirrorXEnabled(_)
            | Self::SetSurfaceMirrorXPlane(_)
            | Self::SetSurfaceMirrorXPlaneVisible(_)
            | Self::UpdateRuntimeStrokePreset { .. }
            | Self::SetRuntimeBrushEngine { .. }
            | Self::ResetBrushPreset { .. }
            | Self::CreateBrushPreset { .. }
            | Self::DeleteBrushPreset { .. }
            | Self::MoveToolGroup { .. }
            | Self::MoveToolEntry { .. }
            | Self::ViewPointer(_)
            | Self::ToolInput(_)
            | Self::AdvanceActiveStroke { .. } => Tool,

            Self::BeginTransformMode
            | Self::ApplyActiveTransform
            | Self::CancelActiveTransform
            | Self::BeginTransformNumericEdit
            | Self::CommitTransformNumericEdit
            | Self::UpdateTransformNumeric(_) => Transform,

            Self::FillMaterial { .. }
            | Self::FillMesh { .. }
            | Self::RectanglePaintUv { .. }
            | Self::RectangleEraseUv { .. } => Paint,

            Self::CutSurfacePixels { .. }
            | Self::DeleteSelectedPixels
            | Self::RectangleSelectUv { .. }
            | Self::SelectAll
            | Self::InvertSelection
            | Self::DeselectSelection
            | Self::SelectFromLayerTransparency { .. } => Selection,

            Self::PreviewAdjustmentFilter { .. }
            | Self::CommitAdjustmentFilterSession { .. }
            | Self::CancelAdjustmentFilterSession { .. } => AdjustmentSessionControl,
            Self::ApplySurfaceBlur { .. }
            | Self::ApplySpatialBlur { .. }
            | Self::ApplyAdjustmentFilter { .. }
            | Self::BeginAdjustmentFilterSession { .. } => Filter,

            Self::Undo | Self::Redo => History,

            Self::PasteLayerMask { .. }
            | Self::AddLayer
            | Self::AddSolidFillLayer { .. }
            | Self::AddAdjustmentLayer { .. }
            | Self::AddGroup
            | Self::DeleteLayer { .. }
            | Self::DeleteLayers { .. }
            | Self::DuplicateLayer { .. }
            | Self::DuplicateLayers { .. }
            | Self::MergeLayers { .. }
            | Self::RasterizeLayer { .. }
            | Self::ApplyLayerMask { .. }
            | Self::AddLayerMask { .. }
            | Self::AddLayerMaskFromSelection { .. }
            | Self::DeleteLayerMask { .. }
            | Self::MoveLayerMask { .. }
            | Self::SelectLayer { .. }
            | Self::SelectSolidFillLayer { .. }
            | Self::SelectAdjustmentLayer { .. }
            | Self::SelectLayerMask { .. }
            | Self::SetLayerMaskEnabled { .. }
            | Self::SelectLayerForStructure { .. }
            | Self::RenameLayer { .. }
            | Self::SetLayerVisible { .. }
            | Self::SetLayerLocked { .. }
            | Self::SetLayersLocked { .. }
            | Self::SetLayerMaterialMask { .. }
            | Self::SetEmbeddedImageTransform { .. }
            | Self::SetLayerOpacity { .. }
            | Self::SetSolidFillColor { .. }
            | Self::SetAdjustment { .. }
            | Self::SetLayersOpacity { .. }
            | Self::SetLayerBlendMode { .. }
            | Self::SetLayersBlendMode { .. }
            | Self::SetLayerGroupCompositeMode { .. }
            | Self::SetLayersGroupCompositeMode { .. }
            | Self::MoveLayer { .. }
            | Self::MoveLayers { .. } => Layer,
        };

        CommandPolicy { domain }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformNumericEdit {
    CenterX(f32),
    CenterY(f32),
    Width(f32),
    Height(f32),
    RotationDegrees(f32),
}

impl TransformNumericEdit {
    pub fn value(self) -> f32 {
        match self {
            Self::CenterX(value)
            | Self::CenterY(value)
            | Self::Width(value)
            | Self::Height(value)
            | Self::RotationDegrees(value) => value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, CommandBlocker, CommandDomain, CommandPolicy};

    fn policy(domain: CommandDomain) -> CommandPolicy {
        CommandPolicy { domain }
    }

    #[test]
    fn camera_framing_commands_are_passive_view_changes() {
        for command in [
            Command::ResetViewportCamera,
            Command::FrameAllViewport,
            Command::ResetUvViewTransform,
        ] {
            assert_eq!(command.policy().domain(), CommandDomain::View);
        }
    }

    #[test]
    fn user_settings_are_non_project_commands() {
        for command in [
            Command::SetTheme(crate::settings::AppTheme::Light),
            Command::SetLanguage(crate::settings::LanguagePreference::Japanese),
            Command::SetTabletBackend(crate::settings::TabletBackend::WindowsInk),
            Command::SetTabletPressureCurve(Default::default()),
        ] {
            assert_eq!(command.policy().domain(), CommandDomain::Settings);
            for blocker in [
                CommandBlocker::RenderCommitPending,
                CommandBlocker::FatalRendererError,
                CommandBlocker::AdjustmentFilterSession,
            ] {
                assert!(command.policy().allowed_during(blocker));
            }
        }
    }

    #[test]
    fn tool_layout_reorders_are_non_project_tool_commands() {
        for command in [
            Command::MoveToolGroup {
                from_index: 0,
                to_index: 1,
            },
            Command::MoveToolEntry {
                group_id: "tool.fill".to_owned(),
                from_index: 0,
                to_index: 1,
            },
        ] {
            assert_eq!(command.policy().domain(), CommandDomain::Tool);
        }
    }

    #[test]
    fn export_file_name_metadata_is_a_project_material_change() {
        for command in [
            Command::SetMaterialExportImageFileNames {
                names: vec![(crate::core::material::MaterialId(1), "Body.png".to_owned())],
            },
            Command::SetMaterialExportPsdFileNames {
                names: vec![(crate::core::material::MaterialId(1), "Body.psd".to_owned())],
            },
        ] {
            assert_eq!(command.policy().domain(), CommandDomain::Material);
        }
    }

    #[test]
    fn passive_commands_are_allowed_during_every_blocker() {
        for domain in [
            CommandDomain::View,
            CommandDomain::Status,
            CommandDomain::InteractionCleanup,
            CommandDomain::Settings,
        ] {
            for blocker in [
                CommandBlocker::RenderCommitPending,
                CommandBlocker::FatalRendererError,
                CommandBlocker::AdjustmentFilterSession,
            ] {
                assert!(policy(domain).allowed_during(blocker));
            }
        }
    }

    #[test]
    fn specialized_commands_are_only_allowed_during_their_intended_blocker() {
        let renderer_settings = policy(CommandDomain::RendererSettings);
        assert!(renderer_settings.allowed_during(CommandBlocker::RenderCommitPending));
        assert!(renderer_settings.allowed_during(CommandBlocker::FatalRendererError));
        assert!(!renderer_settings.allowed_during(CommandBlocker::AdjustmentFilterSession));

        let adjustment_control = policy(CommandDomain::AdjustmentSessionControl);
        assert!(adjustment_control.allowed_during(CommandBlocker::AdjustmentFilterSession));
        assert!(!adjustment_control.allowed_during(CommandBlocker::RenderCommitPending));
        assert!(!adjustment_control.allowed_during(CommandBlocker::FatalRendererError));
    }

    #[test]
    fn editing_commands_are_blocked_and_only_reset_domains_clear_pending_transactions() {
        for domain in [
            CommandDomain::Import,
            CommandDomain::Document,
            CommandDomain::Material,
            CommandDomain::Tool,
            CommandDomain::Transform,
            CommandDomain::Paint,
            CommandDomain::Selection,
            CommandDomain::Filter,
            CommandDomain::Layer,
        ] {
            let policy = policy(domain);
            assert!(!policy.allowed_during(CommandBlocker::RenderCommitPending));
            assert!(!policy.allowed_during(CommandBlocker::FatalRendererError));
            assert!(!policy.allowed_during(CommandBlocker::AdjustmentFilterSession));
            assert!(!policy.clears_pending_transaction());
        }

        assert!(policy(CommandDomain::DocumentReset).clears_pending_transaction());
        assert!(policy(CommandDomain::History).clears_pending_transaction());
    }
}

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::Arc,
};

use glam::{Mat4, Vec2};

use crate::{
    core::{
        adjustment::AdjustmentKind,
        brush_engine::BrushEngineRegistry,
        brush_preset::{BrushPresetCatalog, BrushPresetDefinition},
        camera::OrbitCamera,
        composite::SelectionCompositeMode,
        damage::DamageMap,
        decal::{DecalImageAsset, DecalToolOptions, DecalTransform, ViewProjectionDecalTransform},
        document::{ActiveLayerTarget, Document, ImportedAsset, MeshId, SurfaceHit},
        geometry::RectU32,
        selection::ActiveSelection,
        stroke::{PaintSurfaceSet, StrokeDab, StrokeRenderDescriptor, StrokeSpace, SurfaceDab},
        stroke_preset::StrokeToolPreset,
        stroke_style::ResolvedStrokeStyle,
        surface::{LayerId, PaintSurfaceId, PaintSurfaceRole},
        texture::TextureCatalog,
        tool::{
            ColorPickerToolOptions, DefaultTools, FillScope, FillToolOptions, SelectionShapeKind,
            SelectionToolOptions, ShapeToolOptions, ToolBehavior, ToolDefinition, ToolId,
            ToolShelf, empty_group_config_id,
        },
        tool_catalog::{resolve_default_tools, spaces_from_engine},
        tool_layout::{ToolEntryDefinition, ToolGroupDefinition, ToolLayoutFileV1},
        transform::{TransformHandle, TransformScaleConstraint, UvTransform, UvTransformPreview},
        viewport_shading::ViewportShading,
        viewport_visibility::ViewportSceneVisibility,
        wireframe::WireframeStyle,
    },
    project::{
        DocumentFocusState, EditorViewState, LayerTreeEditorState, ProjectEditorState,
        default_editor_view_state,
    },
    settings::UserSettings,
};

pub(in crate::application) use super::paint_edit::{PaintTargetScope, ResolvedPaintTarget};
use super::{
    HistoryTransaction, InputHoldToken, history_capture::PixelHistoryCapture,
    stroke::input_filter::StrokeInputFilterRuntime,
};

#[derive(Debug)]
pub struct AppState {
    pub(in crate::application) history: HistoryStore,
    pub(in crate::application) document: DocumentSession,
    pub(in crate::application) tool: ToolState,
    pub(in crate::application) view: ViewState,
    pub(in crate::application) settings: UserSettings,
    pub(in crate::application) status: StatusState,
    pub(in crate::application) adjustment_filter_session: Option<AdjustmentFilterSession>,
}

#[derive(Debug, Clone)]
pub(crate) struct UserBrushResourceSnapshot {
    tool: ToolState,
    settings: UserSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct AdjustmentFilterSession {
    pub(in crate::application) document_generation: u64,
    pub(in crate::application) layer_id: LayerId,
    pub(in crate::application) kind: AdjustmentKind,
    pub(in crate::application) target_role: PaintSurfaceRole,
    pub(in crate::application) surfaces: Vec<PaintSurfaceId>,
    pub(in crate::application) active_selection: ActiveSelection,
    pub(in crate::application) renderer_preview_started: bool,
}

#[derive(Debug, Clone, Default)]
pub(in crate::application) struct DocumentSession {
    pub(in crate::application) document: Option<Document>,
    pub(in crate::application) editor: EditorDocumentState,
    generation: u64,
}

#[derive(Debug, Clone)]
pub(in crate::application) struct ToolState {
    pub(in crate::application) tool_presets: Vec<StrokeToolPreset>,
    pub(in crate::application) default_brush_preset: StrokeToolPreset,
    pub(in crate::application) tool_catalog: Vec<ToolDefinition>,
    pub(in crate::application) tool_shelf: ToolShelf,
    pub(in crate::application) brush_engines: BrushEngineRegistry,
    pub(in crate::application) brush_presets: BrushPresetCatalog,
    pub(in crate::application) brush_textures: TextureCatalog,
    pub(in crate::application) tool_layout: ToolLayoutFileV1,
    pub(in crate::application) group_representative_tool_ids: Vec<ToolId>,
    pub(in crate::application) active_tool_id: ToolId,
    pub(in crate::application) momentary_tool_overrides: Vec<MomentaryToolOverride>,
    pub(in crate::application) modal_tool: Option<ModalToolMode>,
    pub(in crate::application) transient_tool_overrides: Vec<ActiveTransientToolOverride>,
    pub(in crate::application) decal_options: DecalToolOptions,
    pub(in crate::application) decal_image: Option<Arc<DecalImageAsset>>,
    pub(in crate::application) view_projection_decal_start_requested: bool,
    pub(in crate::application) fill_options: FillToolOptions,
    pub(in crate::application) shape_options: ShapeToolOptions,
    pub(in crate::application) selection_options: SelectionToolOptions,
    pub(in crate::application) color_picker_options: ColorPickerToolOptions,
    pub(in crate::application) current_color: [f32; 3],
    pub(in crate::application) tool_session: ToolSession,
    transform_idle_cache: RefCell<Option<TransformIdleCache>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::application) struct MomentaryToolOverride {
    pub(in crate::application) token: InputHoldToken,
    pub(in crate::application) tool_id: ToolId,
    pub(in crate::application) pending_completion: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::application) enum ModalToolMode {
    Transform,
}

impl ModalToolMode {
    fn tool_id(self) -> ToolId {
        match self {
            Self::Transform => ToolId::Transform,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::application) struct ActiveTransientToolOverride {
    pub(in crate::application) token: InputHoldToken,
    pub(in crate::application) id: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceMirrorOptions {
    pub x_enabled: bool,
    pub x_plane: f32,
    pub show_x_plane: bool,
}

#[derive(Debug, Clone)]
pub(in crate::application) struct ViewState {
    pub(in crate::application) camera: OrbitCamera,
    pub(in crate::application) shading: ViewportShading,
    pub(in crate::application) gizmo_visible: bool,
    pub(in crate::application) background_color: [f32; 3],
    pub(in crate::application) viewport_wireframe_visible: bool,
    pub(in crate::application) viewport_wireframe: WireframeStyle,
    pub(in crate::application) scene_visibility: ViewportSceneVisibility,
    pub(in crate::application) surface_mirror_x_enabled: bool,
    pub(in crate::application) surface_mirror_x_plane: f32,
    pub(in crate::application) surface_mirror_x_plane_visible: bool,
    pub(in crate::application) uv_view_transform: crate::core::uv_view::UvViewTransform,
    pub(in crate::application) uv_background_color: [f32; 3],
    pub(in crate::application) uv_wireframe_visible: bool,
    pub(in crate::application) uv_wireframe: WireframeStyle,
    pub(in crate::application) uv_view_transform_initialized: bool,
}

#[derive(Debug, Clone)]
pub(in crate::application) struct StatusState {
    pub(in crate::application) message: super::StatusMessage,
}

#[derive(Debug, Clone, Default)]
pub(in crate::application) struct HistoryStore {
    pub(in crate::application) undo_stack: Vec<HistoryTransaction>,
    pub(in crate::application) redo_stack: Vec<HistoryTransaction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::application) struct EditorDocumentState {
    pub(in crate::application) focused_material_index: crate::core::material::MaterialIndex,
    pub(in crate::application) active_layer_id: crate::core::surface::LayerId,
    pub(in crate::application) active_part: crate::core::document::ActiveLayerPart,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) enum ToolSession {
    Idle,
    Stroke(StrokeSession),
    Fill(FillStrokeSession),
    ShapeDrag(ShapeDragSession),
    SelectionDrag(SelectionDragSession),
    Decal(DecalSession),
    ViewProjectionDecal(ViewProjectionDecalSession),
    Transform(TransformSession),
    EmbeddedImageTransform(EmbeddedImageTransformSession),
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct FillStrokeSession {
    pub(in crate::application) scope: FillScope,
    pub(in crate::application) input_space: FillStrokeInputSpace,
    pub(in crate::application) last_screen_px: Vec2,
    pub(in crate::application) visited_targets: HashSet<FillTargetKey>,
    pub(in crate::application) affected_surfaces: Vec<PaintSurfaceId>,
    pub(in crate::application) damage: DamageMap,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) enum FillStrokeInputSpace {
    Uv {
        view: crate::application::UvViewInputContext,
        material_index: crate::core::material::MaterialIndex,
        scene_visibility: ViewportSceneVisibility,
    },
    Surface {
        view: crate::application::ViewportInputContext,
        scene_visibility: ViewportSceneVisibility,
    },
}

impl FillStrokeInputSpace {
    pub(in crate::application) fn space(&self) -> StrokeSpace {
        match self {
            Self::Uv { .. } => StrokeSpace::Uv,
            Self::Surface { .. } => StrokeSpace::Surface,
        }
    }

    pub(in crate::application) fn scene_visibility(&self) -> &ViewportSceneVisibility {
        match self {
            Self::Uv {
                scene_visibility, ..
            }
            | Self::Surface {
                scene_visibility, ..
            } => scene_visibility,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(in crate::application) enum FillTargetKey {
    Polygon {
        mesh_id: MeshId,
        triangle_index: usize,
    },
    Mesh {
        mesh_id: MeshId,
    },
    Material {
        material_index: crate::core::material::MaterialIndex,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct DecalSession {
    pub(in crate::application) transform: DecalTransform,
    pub(in crate::application) source_hit: SurfaceHit,
    pub(in crate::application) gesture: Option<DecalGesture>,
    pub(in crate::application) scene_visibility: ViewportSceneVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) struct DecalGesture {
    pub(in crate::application) handle: TransformHandle,
    pub(in crate::application) pointer_start_px: Vec2,
    pub(in crate::application) base_transform: DecalTransform,
    pub(in crate::application) grab_local: Vec2,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct ViewProjectionDecalSession {
    pub(in crate::application) transform: ViewProjectionDecalTransform,
    pub(in crate::application) gesture: Option<ViewProjectionDecalGesture>,
    pub(in crate::application) scene_visibility: ViewportSceneVisibility,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) struct ViewProjectionDecalGesture {
    pub(in crate::application) handle: TransformHandle,
    pub(in crate::application) pointer_start_px: Vec2,
    pub(in crate::application) base_transform: ViewProjectionDecalTransform,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct TransformIdleCache {
    pub(in crate::application) target: PaintSurfaceId,
    pub(in crate::application) surface_revision: u64,
    pub(in crate::application) selection_revision: u64,
    pub(in crate::application) active_selection: ActiveSelection,
    pub(in crate::application) source_bounds: Option<RectU32>,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) enum TransformSessionSource {
    ExistingSurface,
    ImportedImage {
        before: super::LayerTreeSnapshot,
        added_surfaces: Vec<PaintSurfaceId>,
        file_name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct TransformSession {
    pub(in crate::application) target: PaintSurfaceId,
    pub(in crate::application) material_index: crate::core::material::MaterialIndex,
    pub(in crate::application) texture_size: [u32; 2],
    pub(in crate::application) source_bounds: RectU32,
    pub(in crate::application) source: TransformSessionSource,
    pub(in crate::application) selection_before: ActiveSelection,
    pub(in crate::application) transform: UvTransform,
    pub(in crate::application) scale_sign: Vec2,
    pub(in crate::application) scale_constraint: TransformScaleConstraint,
    pub(in crate::application) gesture: Option<TransformGesture>,
    pub(in crate::application) last_preview_time_s: Option<f64>,
    pub(in crate::application) last_submitted_transform: Option<UvTransform>,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct EmbeddedImageTransformSession {
    pub(in crate::application) layer_id: LayerId,
    pub(in crate::application) material_index: crate::core::material::MaterialIndex,
    pub(in crate::application) texture_size: [u32; 2],
    pub(in crate::application) source_size: [u32; 2],
    pub(in crate::application) before: crate::core::embedded_image::EmbeddedImageTransform,
    pub(in crate::application) transform: UvTransform,
    pub(in crate::application) scale_sign: Vec2,
    pub(in crate::application) scale_constraint: TransformScaleConstraint,
    pub(in crate::application) gesture: Option<TransformGesture>,
    pub(in crate::application) last_preview_time_s: Option<f64>,
    pub(in crate::application) last_submitted_transform: Option<UvTransform>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) struct TransformGesture {
    pub(in crate::application) handle: TransformHandle,
    pub(in crate::application) start_texture_px: Vec2,
    pub(in crate::application) base_transform: UvTransform,
    pub(in crate::application) base_scale_sign: Vec2,
}

impl TransformSession {
    pub(in crate::application) fn preview(&self) -> UvTransformPreview {
        let origin = Vec2::new(
            self.source_bounds.origin[0] as f32,
            self.source_bounds.origin[1] as f32,
        );
        let end = origin
            + Vec2::new(
                self.source_bounds.size[0] as f32,
                self.source_bounds.size[1] as f32,
            );
        let texture = Vec2::new(
            self.texture_size[0].max(1) as f32,
            self.texture_size[1].max(1) as f32,
        );
        let corners_px = [
            origin,
            Vec2::new(end.x, origin.y),
            end,
            Vec2::new(origin.x, end.y),
        ];
        let pivot = Vec2::new(
            self.source_bounds.origin[0] as f32 + self.source_bounds.size[0] as f32 * 0.5,
            self.source_bounds.origin[1] as f32 + self.source_bounds.size[1] as f32 * 0.5,
        );
        UvTransformPreview {
            corners_uv: corners_px.map(|corner| self.transform.transform_point(corner) / texture),
            pivot_uv: self.transform.transform_point(pivot) / texture,
            active_handle: self.gesture.map(|gesture| gesture.handle),
        }
    }
}

impl EmbeddedImageTransformSession {
    pub(in crate::application) fn source_bounds(&self) -> RectU32 {
        RectU32::full(self.source_size)
    }

    pub(in crate::application) fn preview(&self) -> UvTransformPreview {
        let bounds = self.source_bounds();
        let origin = Vec2::new(bounds.origin[0] as f32, bounds.origin[1] as f32);
        let end = origin + Vec2::new(bounds.size[0] as f32, bounds.size[1] as f32);
        let texture = Vec2::new(self.texture_size[0] as f32, self.texture_size[1] as f32);
        let corners_px = [
            origin,
            Vec2::new(end.x, origin.y),
            end,
            Vec2::new(origin.x, end.y),
        ];
        UvTransformPreview {
            corners_uv: corners_px.map(|corner| self.transform.transform_point(corner) / texture),
            pivot_uv: self.transform.transform_point((origin + end) * 0.5) / texture,
            active_handle: self.gesture.map(|gesture| gesture.handle),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) enum CancelledToolSession {
    None,
    Stroke,
    Fill,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct ShapeDragSession {
    pub(in crate::application) tool_id: ToolId,
    pub(in crate::application) gesture: SpatialGesture,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct SelectionDragSession {
    pub(in crate::application) shape: SelectionShapeKind,
    pub(in crate::application) operation: crate::core::composite::SelectionCompositeMode,
    pub(in crate::application) gesture: SpatialGesture,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct SpatialGesture {
    pub(in crate::application) space: GestureSpaceContext,
    pub(in crate::application) geometry: GestureGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) enum GestureSpaceContext {
    Uv {
        view_size: [u32; 2],
    },
    Surface {
        view: crate::application::ViewportInputContext,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) enum GestureGeometry {
    Rectangle { start: Vec2, current: Vec2 },
    Polyline { points: Vec<Vec2>, current: Vec2 },
}

impl SpatialGesture {
    fn rectangle(space: GestureSpaceContext, start: Vec2) -> Self {
        Self {
            space,
            geometry: GestureGeometry::Rectangle {
                start,
                current: start,
            },
        }
    }

    pub(in crate::application) fn space(&self) -> StrokeSpace {
        match self.space {
            GestureSpaceContext::Uv { .. } => StrokeSpace::Uv,
            GestureSpaceContext::Surface { .. } => StrokeSpace::Surface,
        }
    }

    pub(in crate::application) fn update(&mut self, position: Vec2) {
        match &mut self.geometry {
            GestureGeometry::Rectangle { current, .. } => *current = position,
            GestureGeometry::Polyline { points, current } => {
                *current = position;
                let Some(previous) = points.last().copied() else {
                    points.push(position);
                    return;
                };
                let distance_px = match self.space {
                    GestureSpaceContext::Uv { view_size } => {
                        let scale =
                            Vec2::new(view_size[0].max(1) as f32, view_size[1].max(1) as f32);
                        ((position - previous) * scale).length()
                    }
                    GestureSpaceContext::Surface { .. } => position.distance(previous),
                };
                if distance_px >= 2.0 && points.len() < 4096 {
                    points.push(position);
                }
            }
        }
    }

    pub(in crate::application) fn polyline(space: GestureSpaceContext, first: Vec2) -> Self {
        Self {
            space,
            geometry: GestureGeometry::Polyline {
                points: vec![first],
                current: first,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) enum FinishedRectDrag {
    Uv {
        start: Vec2,
        end: Vec2,
    },
    Surface {
        start_px: Vec2,
        end_px: Vec2,
        view: crate::application::ViewportInputContext,
    },
}

impl FinishedRectDrag {
    pub(in crate::application) fn distance_squared(self) -> f32 {
        match self {
            Self::Uv { start, end }
            | Self::Surface {
                start_px: start,
                end_px: end,
                ..
            } => start.distance_squared(end),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) enum FinishedLassoDrag {
    Uv {
        points_uv: Vec<Vec2>,
        view_size: [u32; 2],
    },
    Surface {
        points_px: Vec<Vec2>,
        view: crate::application::ViewportInputContext,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceLassoDragPreview {
    pub points_px: Vec<Vec2>,
    pub viewport_size: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceRectDragPreview {
    pub start_px: Vec2,
    pub current_px: Vec2,
    pub viewport_size: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) enum ContinuousStrokeAnchor {
    Uv(StrokeDab),
    Surface(SurfaceDab),
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct ContinuousStrokeRuntime {
    pub(in crate::application) anchor: Option<ContinuousStrokeAnchor>,
    pub(in crate::application) next_emit_time_s: Option<f64>,
    pub(in crate::application) interval_s: f64,
}

impl ContinuousStrokeRuntime {
    pub(in crate::application) fn new(
        anchor: ContinuousStrokeAnchor,
        time_s: f64,
        rate_hz: f32,
    ) -> Self {
        let interval_s = 1.0 / f64::from(rate_hz.max(1e-3));
        Self {
            anchor: Some(anchor),
            next_emit_time_s: Some(time_s + interval_s),
            interval_s,
        }
    }

    pub(in crate::application) fn update_anchor(
        &mut self,
        anchor: Option<ContinuousStrokeAnchor>,
        time_s: f64,
        reset_deadline: bool,
    ) {
        let was_paused = self.anchor.is_none();
        self.anchor = anchor;
        if self.anchor.is_none() {
            self.next_emit_time_s = None;
        } else if was_paused || reset_deadline {
            self.next_emit_time_s = Some(time_s + self.interval_s);
        }
    }

    pub(in crate::application) fn take_due_count(
        &mut self,
        time_s: f64,
        max_count: usize,
    ) -> usize {
        if !time_s.is_finite() || max_count == 0 || self.anchor.is_none() {
            return 0;
        }
        let Some(next_emit_time_s) = self.next_emit_time_s else {
            return 0;
        };
        let deadline_tolerance_s = self.interval_s * 1e-6;
        if time_s + deadline_tolerance_s < next_emit_time_s {
            return 0;
        }
        let elapsed_intervals =
            (time_s + deadline_tolerance_s - next_emit_time_s) / self.interval_s;
        let due_count = (elapsed_intervals.floor() as usize).saturating_add(1);
        let emit_count = due_count.min(max_count);
        self.next_emit_time_s = Some(if due_count > max_count {
            time_s + self.interval_s
        } else {
            next_emit_time_s + self.interval_s * due_count as f64
        });
        emit_count
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct StrokeSession {
    pub(in crate::application) descriptor: Arc<StrokeRenderDescriptor>,
    pub(in crate::application) space: StrokeSpace,
    pub(in crate::application) last_dab: Option<StrokeDab>,
    pub(in crate::application) last_surface_dab: Option<SurfaceDab>,
    pub(in crate::application) continuous: Option<ContinuousStrokeRuntime>,
    pub(in crate::application) stroke_damage: DamageMap,
    pub(in crate::application) pending_history_capture: Option<PixelHistoryCapture>,
    pub(in crate::application) has_drawn_dabs: bool,
    pub(in crate::application) input_filter: StrokeInputFilterRuntime,
    pub(in crate::application) viewport: Option<ViewportStrokeSessionData>,
}

impl StrokeSession {
    pub(in crate::application) fn target(&self) -> PaintSurfaceId {
        self.descriptor.target
    }

    pub(in crate::application) fn surfaces(&self) -> &PaintSurfaceSet {
        &self.descriptor.surfaces
    }

    pub(in crate::application) fn style(&self) -> &Arc<ResolvedStrokeStyle> {
        &self.descriptor.style
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::application) struct SurfaceMirrorXSessionData {
    pub(in crate::application) plane_x: f32,
    pub(in crate::application) view: crate::application::ViewportInputContext,
    pub(in crate::application) continuous: bool,
    pub(in crate::application) last_triangle: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub(in crate::application) struct ViewportStrokeSessionData {
    pub(in crate::application) locked_viewport_view_proj: Mat4,
    pub(in crate::application) locked_viewport_inv_view_proj: Mat4,
    pub(in crate::application) locked_camera_world: [f32; 3],
    pub(in crate::application) locked_viewport_size: [u32; 2],
    pub(in crate::application) mirror_x: Option<SurfaceMirrorXSessionData>,
}

impl Default for AppState {
    fn default() -> Self {
        #[cfg(not(test))]
        let tools = crate::core::tool::default_tools();
        #[cfg(test)]
        let tools = crate::core::tool_catalog::load_test_tools_from_fixtures()
            .expect("loading deterministic test tool fixtures");
        Self::from_configuration(UserSettings::default(), tools)
    }
}

impl Default for ViewState {
    fn default() -> Self {
        let defaults = default_editor_view_state();
        Self {
            camera: defaults.camera,
            shading: defaults.shading,
            gizmo_visible: defaults.gizmo_visible,
            background_color: defaults.background_color,
            viewport_wireframe_visible: defaults.viewport_wireframe_visible,
            viewport_wireframe: defaults.viewport_wireframe,
            scene_visibility: defaults.scene_visibility,
            surface_mirror_x_enabled: defaults.surface_mirror_x_enabled,
            surface_mirror_x_plane: defaults.surface_mirror_x_plane,
            surface_mirror_x_plane_visible: defaults.surface_mirror_x_plane_visible,
            uv_view_transform: defaults.uv_view_transform,
            uv_background_color: defaults.uv_background_color,
            uv_wireframe_visible: defaults.uv_wireframe_visible,
            uv_wireframe: defaults.uv_wireframe,
            uv_view_transform_initialized: false,
        }
    }
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            message: super::StatusMessage::default(),
        }
    }
}

impl ToolState {
    pub(crate) fn new(default_tools: DefaultTools) -> Self {
        let group_representative_tool_ids: Vec<_> = default_tools
            .shelf
            .groups
            .iter()
            .enumerate()
            .map(|(group_index, group)| {
                group
                    .entries
                    .first()
                    .map(|entry| entry.tool_id)
                    .unwrap_or(ToolId::EmptyGroup(group_index))
            })
            .collect();
        let active_tool_id = *group_representative_tool_ids
            .first()
            .expect("default tool shelf must contain at least one group");
        Self {
            tool_presets: default_tools.presets,
            default_brush_preset: default_tools.default_brush_preset,
            tool_catalog: default_tools.catalog,
            tool_shelf: default_tools.shelf,
            brush_engines: default_tools.brush_engines,
            brush_presets: default_tools.brush_presets,
            brush_textures: default_tools.brush_textures,
            tool_layout: default_tools.tool_layout,
            group_representative_tool_ids,
            active_tool_id,
            momentary_tool_overrides: Vec::new(),
            modal_tool: None,
            transient_tool_overrides: Vec::new(),
            decal_options: DecalToolOptions::default(),
            decal_image: None,
            view_projection_decal_start_requested: false,
            fill_options: FillToolOptions::default(),
            shape_options: ShapeToolOptions::default(),
            selection_options: SelectionToolOptions::default(),
            color_picker_options: ColorPickerToolOptions::default(),
            current_color: [0.95, 0.25, 0.25],
            tool_session: ToolSession::Idle,
            transform_idle_cache: RefCell::new(None),
        }
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.tool_session, ToolSession::Idle)
    }

    pub fn active_tool_id(&self) -> ToolId {
        self.active_tool_id
    }

    pub fn effective_tool_id(&self) -> ToolId {
        self.temporary_tool_id().unwrap_or(self.active_tool_id)
    }

    pub fn panel_tool_id(&self) -> ToolId {
        self.temporary_tool_id().unwrap_or(self.active_tool_id)
    }

    fn temporary_tool_id(&self) -> Option<ToolId> {
        self.modal_tool.map(ModalToolMode::tool_id).or_else(|| {
            self.momentary_tool_overrides
                .last()
                .map(|item| item.tool_id)
        })
    }

    pub fn tool_id_by_config_id(&self, config_id: &str) -> Option<ToolId> {
        self.tool_catalog
            .iter()
            .find(|tool| tool.config_id == config_id)
            .map(|tool| tool.id)
    }

    fn tool_config_id(&self, tool_id: ToolId) -> Option<&str> {
        self.tool_catalog
            .iter()
            .find(|tool| tool.id == tool_id)
            .map(|tool| tool.config_id.as_str())
    }

    fn tool_id_by_config_id_in_group(
        &self,
        config_id: &str,
        expected_group_index: usize,
    ) -> Option<ToolId> {
        let tool_id = self.tool_id_by_config_id(config_id)?;
        (self.tool_shelf.group_containing(tool_id) == Some(expected_group_index)).then_some(tool_id)
    }

    pub fn representative_tool_id_for_group(&self, group_id: &str) -> Option<ToolId> {
        let group_index = self
            .tool_shelf
            .groups
            .iter()
            .position(|group| group.id == group_id)?;
        self.representative_tool_id(group_index)
    }

    pub fn tool_catalog(&self) -> &[ToolDefinition] {
        &self.tool_catalog
    }

    pub fn tool_shelf(&self) -> &ToolShelf {
        &self.tool_shelf
    }

    pub fn representative_tool_id(&self, group_index: usize) -> Option<ToolId> {
        self.group_representative_tool_ids.get(group_index).copied()
    }

    pub(crate) fn restore_persistent_selection<'a>(
        &mut self,
        active_group_id: Option<&str>,
        saved_groups: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    ) {
        let saved_groups = saved_groups.into_iter().collect::<HashMap<_, _>>();
        let fallback_active_group_index = self.tool_shelf.group_containing(self.active_tool_id);
        let mut representatives = self.group_representative_tool_ids.clone();

        for (group_index, group) in self.tool_shelf.groups.iter().enumerate() {
            let Some(saved_entry_id) = saved_groups.get(group.id.as_str()) else {
                continue;
            };
            match saved_entry_id {
                Some(config_id) => {
                    if let Some(tool_id) =
                        self.tool_id_by_config_id_in_group(config_id, group_index)
                    {
                        representatives[group_index] = tool_id;
                    }
                }
                None if group.entries.is_empty() => {
                    representatives[group_index] = ToolId::EmptyGroup(group_index);
                }
                None => {}
            }
        }

        let active_group_index = active_group_id
            .and_then(|active_group_id| {
                self.tool_shelf
                    .groups
                    .iter()
                    .position(|group| group.id == active_group_id)
            })
            .or(fallback_active_group_index);
        if let Some(active_group_index) = active_group_index {
            self.active_tool_id = representatives[active_group_index];
        }
        self.group_representative_tool_ids = representatives;
        self.invalidate_transform_idle_cache();
    }

    pub fn active_tool_preset(&self) -> Option<&StrokeToolPreset> {
        let index = self.active_stroke_preset_index()?;
        self.tool_presets.get(index)
    }

    pub fn effective_tool_preset(&self) -> Option<&StrokeToolPreset> {
        let index = self.effective_stroke_preset_index()?;
        self.tool_presets.get(index)
    }

    #[cfg(test)]
    pub(crate) fn default_brush_preset(&self) -> &StrokeToolPreset {
        &self.default_brush_preset
    }

    pub fn stroke_tool_preset(&self, index: usize) -> Option<&StrokeToolPreset> {
        self.tool_presets.get(index)
    }

    pub(crate) fn brush_engines(&self) -> &BrushEngineRegistry {
        &self.brush_engines
    }

    pub(crate) fn brush_textures(&self) -> &TextureCatalog {
        &self.brush_textures
    }

    pub(crate) fn insert_brush_texture(
        &mut self,
        definition: crate::core::texture::TextureResourceDefinition,
    ) -> anyhow::Result<()> {
        self.brush_textures.insert(definition)
    }

    pub(crate) fn remove_brush_texture(
        &mut self,
        id: &str,
    ) -> Option<crate::core::texture::TextureResourceDefinition> {
        self.brush_textures.remove(id)
    }

    pub(crate) fn brush_texture_users(&self, id: &str) -> Vec<String> {
        self.tool_presets
            .iter()
            .filter(|preset| preset.references_resource(id))
            .map(|preset| preset.name.clone())
            .collect()
    }

    pub(crate) fn set_brush_texture_display_name(&mut self, id: &str, display_name: &str) -> bool {
        self.brush_textures.set_display_name(id, display_name)
    }

    pub(crate) fn brush_preset_definition(&self, id: &str) -> Option<&BrushPresetDefinition> {
        self.brush_presets
            .handle(id)
            .and_then(|handle| self.brush_presets.get(handle))
    }

    pub(crate) fn user_brush_preset_definitions(&self) -> Vec<BrushPresetDefinition> {
        self.brush_presets.user_definitions()
    }

    pub(crate) fn tool_layout_snapshot(&self) -> ToolLayoutFileV1 {
        self.tool_layout.clone()
    }

    pub(crate) fn tool_layout_groups(&self) -> &[ToolGroupDefinition] {
        &self.tool_layout.tools
    }

    pub(in crate::application) fn update_runtime_stroke_preset(
        &mut self,
        preset_index: usize,
        preset: StrokeToolPreset,
    ) -> anyhow::Result<()> {
        let engine_id = match &preset.stroke_op {
            crate::core::stroke_preset::StrokePresetOp::BrushEngine { engine_id, .. } => engine_id,
        };
        let engine = self
            .brush_engines
            .get(engine_id)
            .ok_or_else(|| anyhow::anyhow!("unknown brush engine {:?}", engine_id))?;
        let supported_spaces = spaces_from_engine(engine);
        let tool = self
            .tool_catalog
            .iter_mut()
            .find(|tool| {
                matches!(
                    tool.behavior,
                    ToolBehavior::Stroke { preset_index: index } if index == preset_index
                )
            })
            .ok_or_else(|| anyhow::anyhow!("unknown stroke preset index {preset_index}"))?;
        tool.supported_spaces = supported_spaces;
        *self
            .tool_presets
            .get_mut(preset_index)
            .ok_or_else(|| anyhow::anyhow!("unknown stroke preset index {preset_index}"))? = preset;
        let preset_id = tool.config_id.clone();
        self.invalidate_transform_idle_cache();
        self.sync_brush_definition(preset_index, &preset_id)
    }

    pub(in crate::application) fn set_runtime_brush_engine(
        &mut self,
        preset_index: usize,
        preset_id: &str,
        engine_id: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_interacting(),
            "cannot switch brush engine while a tool is interacting"
        );
        self.brush_tool_id(preset_index, preset_id)?;
        let preset = self
            .tool_presets
            .get(preset_index)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown stroke preset index {preset_index}"))?;
        let retargeted = self.retarget_brush_draft(preset_id, &preset, engine_id)?;
        self.update_runtime_stroke_preset(preset_index, retargeted)
    }

    fn sync_brush_definition(
        &mut self,
        preset_index: usize,
        preset_id: &str,
    ) -> anyhow::Result<()> {
        let tool_id = self.brush_tool_id(preset_index, preset_id)?;
        let preset = self
            .tool_presets
            .get(preset_index)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown stroke preset index {preset_index}"))?;
        let engine_id = match &preset.stroke_op {
            crate::core::stroke_preset::StrokePresetOp::BrushEngine { engine_id, .. } => engine_id,
        };
        let engine = self
            .brush_engines
            .get(engine_id)
            .ok_or_else(|| anyhow::anyhow!("unknown brush engine {:?}", engine_id))?;
        let mut presets = self.brush_presets.clone();
        let handle = presets
            .handle(preset_id)
            .ok_or_else(|| anyhow::anyhow!("unknown brush preset {:?}", preset_id))?;
        presets
            .get_mut(handle)
            .expect("brush preset handle must reference a definition")
            .sync_from_runtime(&preset, engine)?;
        self.brush_presets = presets;
        if let Some(tool) = self.tool_catalog.iter_mut().find(|tool| tool.id == tool_id) {
            tool.name = preset.name.clone();
        }
        for group in &mut self.tool_shelf.groups {
            if let Some(entry) = group
                .entries
                .iter_mut()
                .find(|entry| entry.tool_id == tool_id)
            {
                entry.name = preset.name.clone();
            }
        }
        Ok(())
    }

    pub(in crate::application) fn reset_brush_preset(
        &mut self,
        preset_index: usize,
        preset_id: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_interacting(),
            "cannot reset a brush while interacting"
        );
        self.brush_tool_id(preset_index, preset_id)?;
        let preset = self.brush_engine_defaults(preset_index, preset_id)?;
        self.update_runtime_stroke_preset(preset_index, preset)
    }

    pub(crate) fn brush_engine_defaults(
        &self,
        preset_index: usize,
        preset_id: &str,
    ) -> anyhow::Result<StrokeToolPreset> {
        self.brush_tool_id(preset_index, preset_id)?;
        let current = self
            .tool_presets
            .get(preset_index)
            .expect("validated preset");
        let crate::core::stroke_preset::StrokePresetOp::BrushEngine { engine_id, .. } =
            &current.stroke_op;
        let engine = self.brush_engines.get(engine_id).expect("validated engine");
        let mut definition = self
            .brush_preset_definition(preset_id)
            .expect("validated definition")
            .clone();
        definition.sync_from_runtime(current, engine)?;
        definition.reset_engine_defaults(engine)?;
        let mut defaults = definition.resolve(engine, &self.brush_textures)?;
        defaults.quick_params = current.quick_params.clone();
        Ok(defaults)
    }

    pub(in crate::application) fn create_brush_preset(
        &mut self,
        group_id: &str,
        source_preset_id: &str,
        new_preset_id: &str,
        preset: StrokeToolPreset,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_interacting(),
            "cannot create a brush while a tool is interacting"
        );
        let source = self
            .brush_preset_definition(source_preset_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown source brush preset {:?}", source_preset_id))?;
        let engine_id = match &preset.stroke_op {
            crate::core::stroke_preset::StrokePresetOp::BrushEngine { engine_id, .. } => engine_id,
        };
        let engine = self
            .brush_engines
            .get(engine_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown brush engine {:?}", engine_id))?;
        let mut definition = source.clone_as_user(new_preset_id, preset.name.clone());
        definition.sync_from_runtime(&preset, &engine)?;

        let mut presets = self.brush_presets.clone();
        presets.insert_user(definition)?;
        let mut layout = self.tool_layout.clone();
        let group = layout
            .tools
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or_else(|| anyhow::anyhow!("unknown tool group {:?}", group_id))?;
        let position = group.entries.iter().position(|entry|
            matches!(entry, ToolEntryDefinition::BrushPreset(id) if id == source_preset_id)
        ).map_or(group.entries.len(), |index| index + 1);
        group.entries.insert(
            position,
            ToolEntryDefinition::BrushPreset(new_preset_id.to_owned()),
        );
        self.apply_brush_configuration(presets, layout, Some(new_preset_id.to_owned()), None)
    }

    pub(in crate::application) fn delete_brush_preset(
        &mut self,
        preset_id: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_interacting(),
            "cannot delete a brush while a tool is interacting"
        );
        let target_tool_id = self
            .tool_catalog
            .iter()
            .find(|tool| tool.config_id == preset_id)
            .map(|tool| tool.id)
            .ok_or_else(|| anyhow::anyhow!("unknown brush preset tool {:?}", preset_id))?;
        let active_config_id = self.active_tool_config_id();
        let target_group_index = self
            .tool_shelf
            .group_containing(target_tool_id)
            .ok_or_else(|| {
                anyhow::anyhow!("brush preset tool {:?} is not in a group", preset_id)
            })?;
        let deleted_group_id = self.tool_shelf.groups[target_group_index].id.clone();
        let fallback = self.delete_fallback_config_id(target_group_index, target_tool_id);

        let mut layout = self.tool_layout.clone();
        for group in &mut layout.tools {
            group.entries.retain(
                |entry| !matches!(entry, ToolEntryDefinition::BrushPreset(id) if id == preset_id),
            );
        }
        let mut presets = self.brush_presets.clone();
        presets.delete_user_state(preset_id)?;
        let preferred = if active_config_id.as_deref() == Some(preset_id) {
            fallback.or_else(|| Some(empty_group_config_id(&deleted_group_id)))
        } else {
            None
        };
        self.apply_brush_configuration(presets, layout, preferred, Some(preset_id))
    }

    pub(in crate::application) fn move_tool_group(
        &mut self,
        from_index: usize,
        to_index: usize,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_interacting(),
            "cannot reorder tools while a tool is interacting"
        );
        let len = self.tool_layout.tools.len();
        anyhow::ensure!(from_index < len, "tool group source index out of bounds");
        anyhow::ensure!(to_index < len, "tool group destination index out of bounds");
        if from_index == to_index {
            return Ok(());
        }

        let mut layout = self.tool_layout.clone();
        let group = layout.tools.remove(from_index);
        layout.tools.insert(to_index, group);
        self.apply_brush_configuration(self.brush_presets.clone(), layout, None, None)
    }

    pub(in crate::application) fn move_tool_entry(
        &mut self,
        group_id: &str,
        from_index: usize,
        to_index: usize,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_interacting(),
            "cannot reorder tools while a tool is interacting"
        );
        let mut layout = self.tool_layout.clone();
        let group = layout
            .tools
            .iter_mut()
            .find(|group| group.id == group_id)
            .ok_or_else(|| anyhow::anyhow!("unknown tool group {:?}", group_id))?;
        if !reorder_visible_tool_entries(&mut group.entries, from_index, to_index)? {
            return Ok(());
        }
        self.apply_brush_configuration(self.brush_presets.clone(), layout, None, None)
    }

    pub(crate) fn retarget_brush_draft(
        &self,
        source_preset_id: &str,
        preset: &StrokeToolPreset,
        new_engine_id: &str,
    ) -> anyhow::Result<StrokeToolPreset> {
        let mut definition = self
            .brush_preset_definition(source_preset_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown source brush preset {:?}", source_preset_id))?;
        let current_engine_id = match &preset.stroke_op {
            crate::core::stroke_preset::StrokePresetOp::BrushEngine { engine_id, .. } => engine_id,
        };
        let current_engine = self
            .brush_engines
            .get(current_engine_id)
            .ok_or_else(|| anyhow::anyhow!("unknown brush engine {:?}", current_engine_id))?;
        definition.sync_from_runtime(preset, current_engine)?;
        let new_engine = self
            .brush_engines
            .get(new_engine_id)
            .ok_or_else(|| anyhow::anyhow!("unknown brush engine {:?}", new_engine_id))?;
        definition.retarget_engine(new_engine)?;
        definition.resolve(new_engine, &self.brush_textures)
    }

    fn brush_tool_id(&self, preset_index: usize, preset_id: &str) -> anyhow::Result<ToolId> {
        self.tool_catalog
            .iter()
            .find(|tool| {
                tool.config_id == preset_id
                    && matches!(
                        tool.behavior,
                        ToolBehavior::Stroke { preset_index: index } if index == preset_index
                    )
            })
            .map(|tool| tool.id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "brush preset {:?} does not match stroke preset index {preset_index}",
                    preset_id
                )
            })
    }

    fn active_tool_config_id(&self) -> Option<String> {
        self.tool_config_id(self.active_tool_id).map(str::to_owned)
    }

    fn delete_fallback_config_id(
        &self,
        group_index: usize,
        target_tool_id: ToolId,
    ) -> Option<String> {
        let group = self.tool_shelf.groups.get(group_index)?;
        let target_index = group
            .entries
            .iter()
            .position(|entry| entry.tool_id == target_tool_id)?;
        group
            .entries
            .get(target_index + 1)
            .or_else(|| {
                target_index
                    .checked_sub(1)
                    .and_then(|index| group.entries.get(index))
            })
            .and_then(|entry| {
                self.tool_catalog
                    .iter()
                    .find(|tool| tool.id == entry.tool_id)
                    .map(|tool| tool.config_id.clone())
            })
    }

    fn apply_brush_configuration(
        &mut self,
        brush_presets: BrushPresetCatalog,
        tool_layout: ToolLayoutFileV1,
        preferred_active_config_id: Option<String>,
        discard_runtime_preset_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.apply_brush_resource_configuration(
            self.brush_engines.clone(),
            brush_presets,
            tool_layout,
            preferred_active_config_id,
            discard_runtime_preset_id,
            true,
        )
    }

    fn apply_brush_resource_configuration(
        &mut self,
        brush_engines: BrushEngineRegistry,
        brush_presets: BrushPresetCatalog,
        tool_layout: ToolLayoutFileV1,
        preferred_active_config_id: Option<String>,
        discard_runtime_preset_id: Option<&str>,
        preserve_runtime_presets: bool,
    ) -> anyhow::Result<()> {
        let previous_active_config_id = self.active_tool_config_id();
        let previous_runtime_presets = self
            .tool_catalog
            .iter()
            .filter_map(|tool| {
                let ToolBehavior::Stroke { preset_index } = tool.behavior else {
                    return None;
                };
                self.tool_presets
                    .get(preset_index)
                    .cloned()
                    .map(|preset| (tool.config_id.clone(), preset))
            })
            .collect::<HashMap<_, _>>();
        let previous_representatives = self
            .tool_shelf
            .groups
            .iter()
            .enumerate()
            .filter_map(|(index, group)| {
                let tool_id = self.group_representative_tool_ids.get(index).copied()?;
                let config_id = self.tool_config_id(tool_id)?.to_owned();
                Some((group.id.clone(), config_id))
            })
            .collect::<HashMap<_, _>>();
        let mut resolved = resolve_default_tools(
            &brush_engines,
            &brush_presets,
            &self.brush_textures,
            &tool_layout,
        )?;
        for tool in &mut resolved.catalog {
            if !preserve_runtime_presets {
                break;
            }
            let ToolBehavior::Stroke { preset_index } = tool.behavior else {
                continue;
            };
            if discard_runtime_preset_id == Some(tool.config_id.as_str()) {
                continue;
            }
            let Some(preset) = previous_runtime_presets.get(&tool.config_id).cloned() else {
                continue;
            };
            let engine_id = match &preset.stroke_op {
                crate::core::stroke_preset::StrokePresetOp::BrushEngine { engine_id, .. } => {
                    engine_id
                }
            };
            let engine = brush_engines
                .get(engine_id)
                .ok_or_else(|| anyhow::anyhow!("unknown brush engine {:?}", engine_id))?;
            resolved.presets[preset_index] = preset;
            tool.supported_spaces = spaces_from_engine(engine);
        }
        self.tool_presets = resolved.presets;
        self.default_brush_preset = resolved.default_brush_preset;
        self.tool_catalog = resolved.catalog;
        self.tool_shelf = resolved.shelf;
        self.brush_engines = brush_engines;
        self.brush_presets = brush_presets;
        self.tool_layout = tool_layout;
        self.group_representative_tool_ids = self
            .tool_shelf
            .groups
            .iter()
            .enumerate()
            .map(|(group_index, group)| {
                previous_representatives
                    .get(&group.id)
                    .and_then(|config_id| {
                        self.tool_id_by_config_id_in_group(config_id, group_index)
                    })
                    .or_else(|| group.entries.first().map(|entry| entry.tool_id))
                    .unwrap_or(ToolId::EmptyGroup(group_index))
            })
            .collect();
        let active_config_id =
            preferred_active_config_id.or_else(|| previous_active_config_id.clone());
        let active_tool_id = active_config_id
            .as_deref()
            .and_then(|config_id| self.tool_id_by_config_id(config_id))
            .or_else(|| self.group_representative_tool_ids.first().copied())
            .expect("resolved tool shelf must contain a tool");
        if active_config_id == previous_active_config_id {
            self.active_tool_id = active_tool_id;
            if let Some(group_index) = self.tool_shelf.group_containing(active_tool_id)
                && let Some(representative) =
                    self.group_representative_tool_ids.get_mut(group_index)
            {
                *representative = active_tool_id;
            }
            self.invalidate_transform_idle_cache();
        } else {
            self.set_active_tool(active_tool_id);
        }
        Ok(())
    }

    pub(in crate::application) fn import_holopack_brush_resources(
        &mut self,
        engines: Vec<crate::core::brush_engine::BrushEngineDefinition>,
        presets: Vec<BrushPresetDefinition>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.is_interacting(),
            "cannot import a HoloPack while a tool is interacting"
        );
        let mut candidate_engines = self.brush_engines.clone();
        for engine in engines {
            candidate_engines.insert_or_replace_user(engine, &self.brush_textures)?;
        }
        let mut candidate_presets = self.brush_presets.clone();
        let mut layout = self.tool_layout.clone();
        for preset in presets {
            let is_new = candidate_presets.handle(preset.id()).is_none();
            let preset_id = preset.id().to_owned();
            candidate_presets.insert_or_replace_user(preset)?;
            if is_new {
                let group = layout
                    .tools
                    .iter_mut()
                    .find(|group| group.id == "tool.brush")
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "tool layout has no tool.brush group for imported Brush Preset {:?}",
                            preset_id
                        )
                    })?;
                group
                    .entries
                    .push(ToolEntryDefinition::BrushPreset(preset_id));
            }
        }
        self.apply_brush_resource_configuration(
            candidate_engines,
            candidate_presets,
            layout,
            None,
            None,
            false,
        )
    }

    pub(in crate::application) fn brush_engine_users(&self, id: &str) -> Vec<String> {
        let mut users = self
            .brush_presets
            .definitions()
            .filter(|preset| preset.engine_id() == id)
            .map(|preset| preset.display_name().to_owned())
            .collect::<Vec<_>>();
        users.extend(self.tool_presets.iter().filter_map(|preset| {
            let crate::core::stroke_preset::StrokePresetOp::BrushEngine { engine_id, .. } =
                &preset.stroke_op;
            (engine_id == id).then(|| preset.name.clone())
        }));
        users.sort();
        users.dedup();
        users
    }

    pub(in crate::application) fn delete_user_brush_engine(
        &mut self,
        id: &str,
    ) -> anyhow::Result<()> {
        let users = self.brush_engine_users(id);
        anyhow::ensure!(
            users.is_empty(),
            "Brush Engine {id:?} is used by: {}",
            users.join(", ")
        );
        let mut engines = self.brush_engines.clone();
        engines.remove_user(id)?;
        self.apply_brush_resource_configuration(
            engines,
            self.brush_presets.clone(),
            self.tool_layout.clone(),
            None,
            None,
            false,
        )
    }

    pub fn active_tool_preset_mut(&mut self) -> Option<&mut StrokeToolPreset> {
        let index = self.active_stroke_preset_index()?;
        self.tool_presets.get_mut(index)
    }

    pub fn set_active_tool_preset_index(&mut self, index: usize) {
        if let Some(tool) = self
            .tool_catalog
            .iter()
            .find(|tool| {
                matches!(tool.behavior, ToolBehavior::Stroke { preset_index } if preset_index == index)
            })
        {
            self.set_active_tool(tool.id);
        }
    }

    pub fn active_tool_definition(&self) -> Option<&ToolDefinition> {
        self.tool_catalog
            .iter()
            .find(|tool| tool.id == self.active_tool_id)
    }

    pub fn active_stroke_preset_index(&self) -> Option<usize> {
        match self.active_tool_definition().map(|tool| &tool.behavior) {
            Some(ToolBehavior::Stroke { preset_index }) => Some(*preset_index),
            _ => None,
        }
    }

    pub fn effective_tool_definition(&self) -> Option<&ToolDefinition> {
        let tool_id = self.effective_tool_id();
        self.tool_catalog.iter().find(|tool| tool.id == tool_id)
    }

    pub fn effective_stroke_preset_index(&self) -> Option<usize> {
        match self.effective_tool_definition().map(|tool| &tool.behavior) {
            Some(ToolBehavior::Stroke { preset_index }) => Some(*preset_index),
            _ => None,
        }
    }

    pub(in crate::application) fn begin_momentary_tool(
        &mut self,
        token: InputHoldToken,
        tool_id: ToolId,
    ) {
        if !self.is_idle()
            || self.modal_tool.is_some()
            || self
                .momentary_tool_overrides
                .iter()
                .any(|item| item.token == token)
            || !self.tool_catalog.iter().any(|tool| tool.id == tool_id)
        {
            return;
        }
        self.momentary_tool_overrides.push(MomentaryToolOverride {
            token,
            tool_id,
            pending_completion: None,
        });
        self.invalidate_transform_idle_cache();
    }

    pub(in crate::application) fn complete_momentary_tool(
        &mut self,
        token: InputHoldToken,
        tool_id: ToolId,
        select_tool: bool,
    ) {
        let Some(index) = self
            .momentary_tool_overrides
            .iter()
            .position(|item| item.token == token)
        else {
            return;
        };
        if self.momentary_tool_overrides[index].tool_id != tool_id {
            return;
        }
        if self.is_pointer_gesture_active() {
            self.momentary_tool_overrides[index].pending_completion = Some(select_tool);
            return;
        }
        let item = self.momentary_tool_overrides.remove(index);
        if select_tool {
            self.set_active_tool(item.tool_id);
        }
        self.invalidate_transform_idle_cache();
    }

    pub(in crate::application) fn finish_pending_momentary_tools(&mut self) {
        if self.is_pointer_gesture_active() {
            return;
        }
        let mut selected = None;
        self.momentary_tool_overrides.retain(|item| {
            if let Some(select_tool) = item.pending_completion {
                if select_tool {
                    selected = Some(item.tool_id);
                }
                false
            } else {
                true
            }
        });
        if let Some(tool_id) = selected {
            self.set_active_tool(tool_id);
        }
        self.invalidate_transform_idle_cache();
    }

    pub(in crate::application) fn begin_transform_mode(&mut self) {
        if self.is_idle() && self.modal_tool.is_none() && self.momentary_tool_overrides.is_empty() {
            self.modal_tool = Some(ModalToolMode::Transform);
            self.invalidate_transform_idle_cache();
        }
    }

    pub(in crate::application) fn clear_modal_tool(&mut self) {
        self.modal_tool = None;
        self.invalidate_transform_idle_cache();
    }

    pub fn has_active_modal_tool(&self) -> bool {
        self.modal_tool.is_some()
    }

    pub(in crate::application) fn begin_transient_tool_override(
        &mut self,
        token: InputHoldToken,
        id: String,
    ) {
        if id.trim().is_empty()
            || self
                .transient_tool_overrides
                .iter()
                .any(|item| item.token == token)
        {
            return;
        }
        self.transient_tool_overrides
            .push(ActiveTransientToolOverride { token, id });
    }

    pub(in crate::application) fn end_transient_tool_override(&mut self, token: InputHoldToken) {
        self.transient_tool_overrides
            .retain(|item| item.token != token);
    }

    pub(in crate::application) fn transient_tool_override_ids(&self) -> impl Iterator<Item = &str> {
        self.transient_tool_overrides
            .iter()
            .map(|item| item.id.as_str())
    }

    pub fn set_active_tool(&mut self, tool_id: ToolId) {
        let Some(group_index) = self.tool_shelf.group_containing(tool_id) else {
            return;
        };
        if !self.tool_catalog.iter().any(|tool| tool.id == tool_id) {
            return;
        }
        self.group_representative_tool_ids[group_index] = tool_id;
        self.active_tool_id = tool_id;
        self.tool_session = ToolSession::Idle;
        self.invalidate_transform_idle_cache();
    }

    pub fn is_stroking(&self) -> bool {
        matches!(self.tool_session, ToolSession::Stroke(_))
    }

    pub fn is_interacting(&self) -> bool {
        !self.is_idle() || self.modal_tool.is_some() || !self.momentary_tool_overrides.is_empty()
    }

    pub fn is_pointer_gesture_active(&self) -> bool {
        match &self.tool_session {
            ToolSession::Idle => false,
            ToolSession::Stroke(_)
            | ToolSession::Fill(_)
            | ToolSession::ShapeDrag(_)
            | ToolSession::SelectionDrag(_) => true,
            ToolSession::Decal(session) => session.gesture.is_some(),
            ToolSession::ViewProjectionDecal(session) => session.gesture.is_some(),
            ToolSession::Transform(session) => session.gesture.is_some(),
            ToolSession::EmbeddedImageTransform(session) => session.gesture.is_some(),
        }
    }

    pub(in crate::application) fn clear_session(&mut self) {
        self.tool_session = ToolSession::Idle;
    }

    pub(in crate::application) fn cancel_session(&mut self) -> CancelledToolSession {
        match std::mem::replace(&mut self.tool_session, ToolSession::Idle) {
            ToolSession::Idle => CancelledToolSession::None,
            ToolSession::Stroke(_) => CancelledToolSession::Stroke,
            ToolSession::Fill(_) => CancelledToolSession::Fill,
            ToolSession::Transform(_)
            | ToolSession::EmbeddedImageTransform(_)
            | ToolSession::Decal(_)
            | ToolSession::ViewProjectionDecal(_)
            | ToolSession::ShapeDrag(_)
            | ToolSession::SelectionDrag(_) => CancelledToolSession::Other,
        }
    }

    pub(in crate::application) fn set_session(&mut self, session: ToolSession) {
        self.tool_session = session;
    }

    pub(in crate::application) fn cached_transform_idle_bounds(
        &self,
        target: PaintSurfaceId,
        surface_revision: u64,
        selection_revision: u64,
        active_selection: &ActiveSelection,
    ) -> Option<Option<RectU32>> {
        let cache = self.transform_idle_cache.borrow();
        let cache = cache.as_ref()?;
        (cache.target == target
            && cache.surface_revision == surface_revision
            && cache.selection_revision == selection_revision
            && cache.active_selection == *active_selection)
            .then_some(cache.source_bounds)
    }

    pub(in crate::application) fn cache_transform_idle_bounds(
        &self,
        target: PaintSurfaceId,
        surface_revision: u64,
        selection_revision: u64,
        active_selection: ActiveSelection,
        source_bounds: Option<RectU32>,
    ) {
        *self.transform_idle_cache.borrow_mut() = Some(TransformIdleCache {
            target,
            surface_revision,
            selection_revision,
            active_selection,
            source_bounds,
        });
    }

    pub(in crate::application) fn invalidate_transform_idle_cache(&self) {
        self.transform_idle_cache.borrow_mut().take();
    }

    pub(in crate::application) fn fill_session(&self) -> Option<&FillStrokeSession> {
        match &self.tool_session {
            ToolSession::Fill(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn fill_session_mut(&mut self) -> Option<&mut FillStrokeSession> {
        match &mut self.tool_session {
            ToolSession::Fill(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn take_fill_session(&mut self) -> Option<FillStrokeSession> {
        match std::mem::replace(&mut self.tool_session, ToolSession::Idle) {
            ToolSession::Fill(session) => Some(session),
            other => {
                self.tool_session = other;
                None
            }
        }
    }

    pub(in crate::application) fn decal_session(&self) -> Option<&DecalSession> {
        match &self.tool_session {
            ToolSession::Decal(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn decal_session_mut(&mut self) -> Option<&mut DecalSession> {
        match &mut self.tool_session {
            ToolSession::Decal(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn take_decal_session(&mut self) -> Option<DecalSession> {
        match std::mem::replace(&mut self.tool_session, ToolSession::Idle) {
            ToolSession::Decal(session) => Some(session),
            other => {
                self.tool_session = other;
                None
            }
        }
    }

    pub(in crate::application) fn view_projection_decal_session(
        &self,
    ) -> Option<&ViewProjectionDecalSession> {
        match &self.tool_session {
            ToolSession::ViewProjectionDecal(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn view_projection_decal_session_mut(
        &mut self,
    ) -> Option<&mut ViewProjectionDecalSession> {
        match &mut self.tool_session {
            ToolSession::ViewProjectionDecal(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn take_view_projection_decal_session(
        &mut self,
    ) -> Option<ViewProjectionDecalSession> {
        match std::mem::replace(&mut self.tool_session, ToolSession::Idle) {
            ToolSession::ViewProjectionDecal(session) => Some(session),
            other => {
                self.tool_session = other;
                None
            }
        }
    }

    pub(in crate::application) fn transform_session(&self) -> Option<&TransformSession> {
        match &self.tool_session {
            ToolSession::Transform(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn transform_session_mut(
        &mut self,
    ) -> Option<&mut TransformSession> {
        match &mut self.tool_session {
            ToolSession::Transform(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn take_transform_session(&mut self) -> Option<TransformSession> {
        match std::mem::replace(&mut self.tool_session, ToolSession::Idle) {
            ToolSession::Transform(session) => Some(session),
            other => {
                self.tool_session = other;
                None
            }
        }
    }

    pub(in crate::application) fn embedded_image_transform_session(
        &self,
    ) -> Option<&EmbeddedImageTransformSession> {
        match &self.tool_session {
            ToolSession::EmbeddedImageTransform(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn embedded_image_transform_session_mut(
        &mut self,
    ) -> Option<&mut EmbeddedImageTransformSession> {
        match &mut self.tool_session {
            ToolSession::EmbeddedImageTransform(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn take_embedded_image_transform_session(
        &mut self,
    ) -> Option<EmbeddedImageTransformSession> {
        match std::mem::replace(&mut self.tool_session, ToolSession::Idle) {
            ToolSession::EmbeddedImageTransform(session) => Some(session),
            other => {
                self.tool_session = other;
                None
            }
        }
    }

    pub(in crate::application) fn stroke_session(
        &self,
        space: StrokeSpace,
    ) -> Option<&StrokeSession> {
        match &self.tool_session {
            ToolSession::Stroke(session) if session.space == space => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn stroke_session_mut(
        &mut self,
        space: StrokeSpace,
    ) -> Option<&mut StrokeSession> {
        match &mut self.tool_session {
            ToolSession::Stroke(session) if session.space == space => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn active_stroke_session(&self) -> Option<&StrokeSession> {
        match &self.tool_session {
            ToolSession::Stroke(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn active_stroke_session_mut(
        &mut self,
    ) -> Option<&mut StrokeSession> {
        match &mut self.tool_session {
            ToolSession::Stroke(session) => Some(session),
            _ => None,
        }
    }

    pub(in crate::application) fn finish_stroke_session(
        &mut self,
        space: StrokeSpace,
    ) -> Option<PaintSurfaceSet> {
        let surfaces = self.stroke_session(space)?.surfaces().clone();
        self.clear_session();
        Some(surfaces)
    }

    pub(in crate::application) fn finish_stroke_session_with_history(
        &mut self,
        space: StrokeSpace,
    ) -> Option<(PaintSurfaceSet, Option<PixelHistoryCapture>)> {
        let session = self.stroke_session(space)?;
        let surfaces = session.surfaces().clone();
        let history_capture = session.pending_history_capture.clone();
        self.clear_session();
        Some((surfaces, history_capture))
    }

    pub(in crate::application) fn extend_stroke_session_surfaces(
        &mut self,
        space: StrokeSpace,
        surfaces: &PaintSurfaceSet,
    ) {
        let Some(session) = self.stroke_session_mut(space) else {
            return;
        };
        let mut merged = session.surfaces().as_slice().to_vec();
        for surface in surfaces.iter() {
            if !merged.contains(&surface) {
                merged.push(surface);
            }
        }
        session.descriptor = Arc::new(StrokeRenderDescriptor {
            surfaces: PaintSurfaceSet::from_vec(merged),
            ..session.descriptor.as_ref().clone()
        });
    }

    pub(in crate::application) fn begin_shape_drag(
        &mut self,
        tool_id: ToolId,
        position: Vec2,
        view_size: [u32; 2],
    ) {
        if self.is_idle() {
            self.tool_session = ToolSession::ShapeDrag(ShapeDragSession {
                tool_id,
                gesture: SpatialGesture::rectangle(GestureSpaceContext::Uv { view_size }, position),
            });
        }
    }

    pub(in crate::application) fn begin_surface_shape_drag(
        &mut self,
        tool_id: ToolId,
        position_px: Vec2,
        view: crate::application::ViewportInputContext,
    ) {
        if self.is_idle() {
            self.tool_session = ToolSession::ShapeDrag(ShapeDragSession {
                tool_id,
                gesture: SpatialGesture::rectangle(
                    GestureSpaceContext::Surface { view },
                    position_px,
                ),
            });
        }
    }

    pub(in crate::application) fn begin_lasso_shape_drag(
        &mut self,
        tool_id: ToolId,
        position: Vec2,
        view_size: [u32; 2],
    ) {
        if self.is_idle() {
            self.tool_session = ToolSession::ShapeDrag(ShapeDragSession {
                tool_id,
                gesture: SpatialGesture::polyline(GestureSpaceContext::Uv { view_size }, position),
            });
        }
    }

    pub(in crate::application) fn begin_surface_lasso_shape_drag(
        &mut self,
        tool_id: ToolId,
        position_px: Vec2,
        view: crate::application::ViewportInputContext,
    ) {
        if self.is_idle() {
            self.tool_session = ToolSession::ShapeDrag(ShapeDragSession {
                tool_id,
                gesture: SpatialGesture::polyline(
                    GestureSpaceContext::Surface { view },
                    position_px,
                ),
            });
        }
    }

    pub(in crate::application) fn update_shape_drag(
        &mut self,
        tool_id: ToolId,
        space: StrokeSpace,
        position: Vec2,
    ) -> bool {
        let ToolSession::ShapeDrag(session) = &mut self.tool_session else {
            return false;
        };
        if session.tool_id != tool_id || session.gesture.space() != space {
            return false;
        }
        session.gesture.update(position);
        true
    }

    pub(in crate::application) fn finish_shape_drag(
        &mut self,
        tool_id: ToolId,
        space: StrokeSpace,
        position: Vec2,
    ) -> Option<FinishedRectDrag> {
        let ToolSession::ShapeDrag(session) = &self.tool_session else {
            return None;
        };
        if session.tool_id != tool_id || session.gesture.space() != space {
            return None;
        }
        let GestureGeometry::Rectangle { start, .. } = session.gesture.geometry else {
            return None;
        };
        let result = match session.gesture.space {
            GestureSpaceContext::Uv { .. } => FinishedRectDrag::Uv {
                start,
                end: position,
            },
            GestureSpaceContext::Surface { view } => FinishedRectDrag::Surface {
                start_px: start,
                end_px: position,
                view,
            },
        };
        self.clear_session();
        Some(result)
    }

    pub(in crate::application) fn finish_lasso_shape_drag(
        &mut self,
        tool_id: ToolId,
        space: StrokeSpace,
        position: Vec2,
    ) -> Option<FinishedLassoDrag> {
        let ToolSession::ShapeDrag(session) = &self.tool_session else {
            return None;
        };
        if session.tool_id != tool_id || session.gesture.space() != space {
            return None;
        }
        let gesture = session.gesture.clone();
        let points = finalized_polyline_points(&gesture, position);
        self.clear_session();
        let points = points?;
        Some(match gesture.space {
            GestureSpaceContext::Uv { view_size } => FinishedLassoDrag::Uv {
                points_uv: points,
                view_size,
            },
            GestureSpaceContext::Surface { view } => FinishedLassoDrag::Surface {
                points_px: points,
                view,
            },
        })
    }

    pub(in crate::application) fn begin_selection_drag(
        &mut self,
        shape: SelectionShapeKind,
        operation: SelectionCompositeMode,
        position: Vec2,
        view_size: [u32; 2],
    ) {
        if self.is_idle() {
            let gesture = match shape {
                SelectionShapeKind::Rectangle => {
                    SpatialGesture::rectangle(GestureSpaceContext::Uv { view_size }, position)
                }
                SelectionShapeKind::Lasso => {
                    SpatialGesture::polyline(GestureSpaceContext::Uv { view_size }, position)
                }
            };
            self.tool_session = ToolSession::SelectionDrag(SelectionDragSession {
                shape,
                operation,
                gesture,
            });
        }
    }

    pub(in crate::application) fn begin_surface_selection_drag(
        &mut self,
        shape: SelectionShapeKind,
        operation: SelectionCompositeMode,
        position_px: Vec2,
        view: crate::application::ViewportInputContext,
    ) {
        if self.is_idle() {
            let gesture = match shape {
                SelectionShapeKind::Rectangle => {
                    SpatialGesture::rectangle(GestureSpaceContext::Surface { view }, position_px)
                }
                SelectionShapeKind::Lasso => {
                    SpatialGesture::polyline(GestureSpaceContext::Surface { view }, position_px)
                }
            };
            self.tool_session = ToolSession::SelectionDrag(SelectionDragSession {
                shape,
                operation,
                gesture,
            });
        }
    }

    pub(in crate::application) fn update_selection_drag(
        &mut self,
        space: StrokeSpace,
        position: Vec2,
    ) -> bool {
        let ToolSession::SelectionDrag(session) = &mut self.tool_session else {
            return false;
        };
        if session.gesture.space() != space {
            return false;
        }
        session.gesture.update(position);
        true
    }

    pub(in crate::application) fn finish_selection_drag(
        &mut self,
        space: StrokeSpace,
        position: Vec2,
    ) -> Option<(SelectionCompositeMode, FinishedRectDrag)> {
        let ToolSession::SelectionDrag(session) = &self.tool_session else {
            return None;
        };
        if session.gesture.space() != space {
            return None;
        }
        let GestureGeometry::Rectangle { start, .. } = session.gesture.geometry else {
            return None;
        };
        let drag = match session.gesture.space {
            GestureSpaceContext::Uv { .. } => FinishedRectDrag::Uv {
                start,
                end: position,
            },
            GestureSpaceContext::Surface { view } => FinishedRectDrag::Surface {
                start_px: start,
                end_px: position,
                view,
            },
        };
        let result = (session.operation, drag);
        self.clear_session();
        Some(result)
    }

    pub(in crate::application) fn finish_lasso_selection_drag(
        &mut self,
        space: StrokeSpace,
        position: Vec2,
    ) -> Option<(SelectionCompositeMode, FinishedLassoDrag)> {
        let ToolSession::SelectionDrag(session) = &self.tool_session else {
            return None;
        };
        if session.shape != SelectionShapeKind::Lasso || session.gesture.space() != space {
            return None;
        }
        let operation = session.operation;
        let gesture = session.gesture.clone();
        let points = finalized_polyline_points(&gesture, position);
        self.clear_session();
        let points = points?;
        let drag = match gesture.space {
            GestureSpaceContext::Uv { view_size } => FinishedLassoDrag::Uv {
                points_uv: points,
                view_size,
            },
            GestureSpaceContext::Surface { view } => FinishedLassoDrag::Surface {
                points_px: points,
                view,
            },
        };
        Some((operation, drag))
    }

    pub(in crate::application) fn active_transform_preview(&self) -> Option<UvTransformPreview> {
        self.transform_session()
            .map(TransformSession::preview)
            .or_else(|| {
                self.embedded_image_transform_session()
                    .map(EmbeddedImageTransformSession::preview)
            })
    }

    pub fn uv_drag_preview(&self) -> Option<(Vec2, Vec2)> {
        match &self.tool_session {
            ToolSession::ShapeDrag(session) => uv_rectangle(&session.gesture),
            ToolSession::SelectionDrag(session) => uv_rectangle(&session.gesture),
            _ => None,
        }
    }

    pub fn surface_rect_drag_preview(&self) -> Option<SurfaceRectDragPreview> {
        match &self.tool_session {
            ToolSession::ShapeDrag(session) => surface_rectangle(&session.gesture),
            ToolSession::SelectionDrag(session) => surface_rectangle(&session.gesture),
            _ => None,
        }
    }

    pub fn uv_lasso_drag_preview(&self) -> Option<Vec<Vec2>> {
        match &self.tool_session {
            ToolSession::ShapeDrag(session) => uv_polyline(&session.gesture),
            ToolSession::SelectionDrag(session) => uv_polyline(&session.gesture),
            _ => None,
        }
    }

    pub fn surface_lasso_drag_preview(&self) -> Option<SurfaceLassoDragPreview> {
        match &self.tool_session {
            ToolSession::ShapeDrag(session) => surface_polyline(&session.gesture),
            ToolSession::SelectionDrag(session) => surface_polyline(&session.gesture),
            _ => None,
        }
    }
}

fn uv_rectangle(gesture: &SpatialGesture) -> Option<(Vec2, Vec2)> {
    let GestureSpaceContext::Uv { .. } = gesture.space else {
        return None;
    };
    let GestureGeometry::Rectangle { start, current } = gesture.geometry else {
        return None;
    };
    Some((start, current))
}

fn surface_rectangle(gesture: &SpatialGesture) -> Option<SurfaceRectDragPreview> {
    let GestureSpaceContext::Surface { view } = gesture.space else {
        return None;
    };
    let GestureGeometry::Rectangle { start, current } = gesture.geometry else {
        return None;
    };
    Some(SurfaceRectDragPreview {
        start_px: start,
        current_px: current,
        viewport_size: view.size,
    })
}

fn uv_polyline(gesture: &SpatialGesture) -> Option<Vec<Vec2>> {
    let GestureSpaceContext::Uv { .. } = gesture.space else {
        return None;
    };
    preview_polyline_points(gesture)
}

fn surface_polyline(gesture: &SpatialGesture) -> Option<SurfaceLassoDragPreview> {
    let GestureSpaceContext::Surface { view } = gesture.space else {
        return None;
    };
    Some(SurfaceLassoDragPreview {
        points_px: preview_polyline_points(gesture)?,
        viewport_size: view.size,
    })
}

fn preview_polyline_points(gesture: &SpatialGesture) -> Option<Vec<Vec2>> {
    let GestureGeometry::Polyline { points, current } = &gesture.geometry else {
        return None;
    };
    let mut preview = points.clone();
    if preview
        .last()
        .is_none_or(|last| last.distance_squared(*current) > f32::EPSILON)
    {
        preview.push(*current);
    }
    Some(preview)
}

fn finalized_polyline_points(gesture: &SpatialGesture, final_position: Vec2) -> Option<Vec<Vec2>> {
    let GestureGeometry::Polyline { points, .. } = &gesture.geometry else {
        return None;
    };
    let mut points = points.clone();
    if points
        .last()
        .is_none_or(|last| last.distance_squared(final_position) > f32::EPSILON)
    {
        points.push(final_position);
    }
    points.dedup_by(|a, b| (*a).distance_squared(*b) <= f32::EPSILON);
    if points.len() < 3 {
        return None;
    }

    let pixel_points: Vec<Vec2> = points
        .iter()
        .copied()
        .map(|point| match gesture.space {
            GestureSpaceContext::Uv { view_size } => {
                point * Vec2::new(view_size[0].max(1) as f32, view_size[1].max(1) as f32)
            }
            GestureSpaceContext::Surface { .. } => point,
        })
        .collect();
    let mut epsilon = 0.75;
    let mut keep = rdp_indices(&pixel_points, epsilon);
    while keep.len() > 1024 {
        epsilon *= 1.5;
        keep = rdp_indices(&pixel_points, epsilon);
    }
    let simplified: Vec<Vec2> = keep.into_iter().map(|index| points[index]).collect();
    (simplified.len() >= 3).then_some(simplified)
}

fn rdp_indices(points: &[Vec2], epsilon: f32) -> Vec<usize> {
    if points.len() <= 2 {
        return (0..points.len()).collect();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    rdp_mark(points, 0, points.len() - 1, epsilon * epsilon, &mut keep);
    keep.into_iter()
        .enumerate()
        .filter_map(|(index, keep)| keep.then_some(index))
        .collect()
}

fn rdp_mark(points: &[Vec2], first: usize, last: usize, epsilon_squared: f32, keep: &mut [bool]) {
    if last <= first + 1 {
        return;
    }
    let start = points[first];
    let end = points[last];
    let segment = end - start;
    let segment_length_squared = segment.length_squared();
    let mut farthest = None;
    let mut farthest_distance_squared = 0.0;
    for (offset, point) in points[(first + 1)..last].iter().copied().enumerate() {
        let index = first + 1 + offset;
        let distance_squared = if segment_length_squared <= f32::EPSILON {
            point.distance_squared(start)
        } else {
            let t = ((point - start).dot(segment) / segment_length_squared).clamp(0.0, 1.0);
            point.distance_squared(start + segment * t)
        };
        if distance_squared > farthest_distance_squared {
            farthest_distance_squared = distance_squared;
            farthest = Some(index);
        }
    }
    if farthest_distance_squared <= epsilon_squared {
        return;
    }
    let index = farthest.expect("non-adjacent polyline segment has an interior point");
    keep[index] = true;
    rdp_mark(points, first, index, epsilon_squared, keep);
    rdp_mark(points, index, last, epsilon_squared, keep);
}

impl Default for EditorDocumentState {
    fn default() -> Self {
        use slotmap::Key;

        Self {
            focused_material_index: 0.into(),
            active_layer_id: crate::core::surface::LayerId::null(),
            active_part: crate::core::document::ActiveLayerPart::Content,
        }
    }
}

impl DocumentSession {
    pub(in crate::application) fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }

    pub(in crate::application) fn document_mut(&mut self) -> Option<&mut Document> {
        self.document.as_mut()
    }

    pub(in crate::application) fn document_and_editor_document_mut(
        &mut self,
    ) -> Option<(&mut Document, &mut EditorDocumentState)> {
        self.document
            .as_mut()
            .map(|document| (document, &mut self.editor))
    }

    pub(in crate::application) fn resolved_editor_document(
        &self,
        document: &Document,
    ) -> EditorDocumentState {
        self.editor.resolved(document)
    }

    pub(in crate::application) fn replace(
        &mut self,
        document: Document,
        editor: EditorDocumentState,
    ) {
        self.document = Some(document);
        self.editor = editor;
        self.bump_generation();
    }

    pub(in crate::application) fn clear(&mut self) {
        self.document = None;
        self.editor = EditorDocumentState::default();
        self.bump_generation();
    }

    pub(in crate::application) fn load_asset(&mut self, asset: ImportedAsset) {
        self.document = Some(Document::from_imported(asset));
        self.editor = self
            .document
            .as_ref()
            .map(EditorDocumentState::from_document)
            .unwrap_or_default();
        self.bump_generation();
    }

    pub(in crate::application) fn generation(&self) -> u64 {
        self.generation
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub(in crate::application) fn mark_reloaded(&mut self) {
        self.bump_generation();
    }

    pub(in crate::application) fn active_layer_id(&self) -> LayerId {
        self.editor.active_layer_id
    }

    pub(in crate::application) fn active_layer_target(&self) -> ActiveLayerTarget {
        self.document
            .as_ref()
            .map(|document| self.editor.resolved(document).active_target(document))
            .unwrap_or(ActiveLayerTarget::Structure)
    }

    fn select_content(&mut self, layer_id: LayerId) {
        self.editor.active_layer_id = layer_id;
        self.editor.active_part = crate::core::document::ActiveLayerPart::Content;
    }

    fn select_mask(&mut self, layer_id: LayerId) {
        self.editor.active_layer_id = layer_id;
        self.editor.active_part = crate::core::document::ActiveLayerPart::LayerMask;
    }

    pub(in crate::application) fn select_raster_layer_if_paintable(
        &mut self,
        layer_id: LayerId,
    ) -> bool {
        let Some(document) = self.document() else {
            return false;
        };
        if !document.layer_tree.is_paintable(layer_id) {
            return false;
        }
        self.select_content(layer_id);
        true
    }

    pub(in crate::application) fn select_solid_fill_layer_if_present(
        &mut self,
        layer_id: LayerId,
    ) -> bool {
        let Some(document) = self.document() else {
            return false;
        };
        if !document.layer_tree.is_solid_fill(layer_id) {
            return false;
        }
        self.select_content(layer_id);
        true
    }

    pub(in crate::application) fn select_adjustment_layer_if_present(
        &mut self,
        layer_id: LayerId,
    ) -> bool {
        let Some(document) = self.document() else {
            return false;
        };
        if !document.layer_tree.is_adjustment(layer_id) {
            return false;
        }
        self.select_content(layer_id);
        true
    }

    pub(in crate::application) fn select_layer_mask_if_present(
        &mut self,
        layer_id: LayerId,
    ) -> bool {
        let Some(document) = self.document() else {
            return false;
        };
        if !document.layer_tree.has_layer_mask(layer_id) {
            return false;
        }
        self.select_mask(layer_id);
        true
    }

    pub(in crate::application) fn select_structure_layer_if_present(
        &mut self,
        layer_id: LayerId,
    ) -> bool {
        let Some(document) = self.document() else {
            return false;
        };
        if !document.layer_tree.is_group(layer_id)
            && !document.layer_tree.is_embedded_image(layer_id)
        {
            return false;
        }
        self.select_content(layer_id);
        true
    }

    pub(in crate::application) fn set_focused_material(&mut self, material_index: usize) -> bool {
        let Some(document) = self.document() else {
            return false;
        };
        if material_index >= document.materials.len() {
            return false;
        }
        self.editor.focused_material_index = material_index.into();
        true
    }

    /// [NOTE: Design Architecture]
    /// `focused_material_index` (and the derived `active_paint_surface()`) is a pure UI state
    /// indicating which material is currently displayed in the UV View.
    /// DO NOT use this as a condition to filter paint targets or skip rendering in the Viewport (3D space).
    /// 3D viewport painting should inherently be applied across all materials in the scene.
    pub(in crate::application) fn focused_material_index(&self) -> usize {
        self.document()
            .map(|_| self.editor.focused_material_index.as_usize())
            .unwrap_or(0)
    }

    pub(in crate::application) fn active_paint_surface(&self) -> Option<PaintSurfaceId> {
        self.resolve_paint_target(PaintTargetScope::FocusedMaterial)
            .map(|target| target.target())
    }

    pub(in crate::application) fn active_paint_surfaces(&self) -> Vec<PaintSurfaceId> {
        self.resolve_paint_target(PaintTargetScope::AllMaterials)
            .map(|target| target.surfaces_vec())
            .unwrap_or_default()
    }

    pub(in crate::application) fn active_paint_surfaces_for_mesh(
        &self,
        mesh_id: MeshId,
    ) -> Vec<PaintSurfaceId> {
        self.resolve_paint_target(PaintTargetScope::Mesh(mesh_id))
            .map(|target| target.surfaces_vec())
            .unwrap_or_default()
    }
}

fn reorder_visible_tool_entries(
    entries: &mut [ToolEntryDefinition],
    from_index: usize,
    to_index: usize,
) -> anyhow::Result<bool> {
    let slots = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            (!matches!(entry, ToolEntryDefinition::Separator)).then_some(index)
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        from_index < slots.len(),
        "tool entry source index out of bounds"
    );
    anyhow::ensure!(
        to_index < slots.len(),
        "tool entry destination index out of bounds"
    );
    if from_index == to_index {
        return Ok(false);
    }

    let mut visible_entries = slots
        .iter()
        .map(|&index| entries[index].clone())
        .collect::<Vec<_>>();
    let entry = visible_entries.remove(from_index);
    visible_entries.insert(to_index, entry);
    for (slot, entry) in slots.into_iter().zip(visible_entries) {
        entries[slot] = entry;
    }
    Ok(true)
}

impl AppState {
    pub const MAX_HISTORY_ENTRIES: usize = 64;
    pub const MAX_HISTORY_BYTES: usize = 256 * 1024 * 1024;

    fn from_configuration(settings: UserSettings, default_tools: DefaultTools) -> Self {
        Self {
            history: HistoryStore::default(),
            document: DocumentSession::default(),
            tool: ToolState::new(default_tools),
            view: ViewState::default(),
            settings,
            status: StatusState::default(),
            adjustment_filter_session: None,
        }
    }

    pub(crate) fn with_status_and_configuration(
        status: impl Into<String>,
        settings: UserSettings,
        default_tools: DefaultTools,
    ) -> Self {
        let mut state = Self::from_configuration(settings, default_tools);
        state.status.message = super::StatusMessage::raw(status);
        state
    }

    pub fn with_status(status: impl Into<String>) -> Self {
        let mut state = Self::default();
        state.status.message = super::StatusMessage::raw(status);
        state
    }

    pub fn document(&self) -> Option<&Document> {
        self.document.document()
    }

    pub fn document_generation(&self) -> u64 {
        self.document.generation()
    }

    pub fn has_adjustment_filter_session(&self) -> bool {
        self.adjustment_filter_session.is_some()
    }

    pub fn adjustment_filter_session_matches(
        &self,
        layer_id: LayerId,
        kind: AdjustmentKind,
    ) -> bool {
        self.adjustment_filter_session
            .as_ref()
            .is_some_and(|session| {
                session.document_generation == self.document_generation()
                    && session.layer_id == layer_id
                    && session.kind == kind
            })
    }

    pub fn is_document_edit_interacting(&self) -> bool {
        self.is_tool_interacting() || self.has_adjustment_filter_session()
    }

    pub fn active_selection(&self) -> Option<&crate::core::selection::ActiveSelection> {
        self.document().map(|document| &document.active_selection)
    }

    pub fn has_active_selection(&self) -> bool {
        self.active_selection()
            .is_some_and(|selection| selection.is_visible_active())
    }

    pub(in crate::application) fn document_mut(&mut self) -> Option<&mut Document> {
        self.document.document_mut()
    }

    pub(in crate::application) fn document_and_editor_document_mut(
        &mut self,
    ) -> Option<(&mut Document, &mut EditorDocumentState)> {
        self.document.document_and_editor_document_mut()
    }

    pub(in crate::application) fn resolved_editor_document(
        &self,
        document: &Document,
    ) -> EditorDocumentState {
        self.document.resolved_editor_document(document)
    }

    pub(in crate::application) fn view_mut(&mut self) -> &mut ViewState {
        &mut self.view
    }

    pub(in crate::application) fn settings_mut(&mut self) -> &mut UserSettings {
        &mut self.settings
    }

    pub(crate) fn user_settings(&self) -> &UserSettings {
        &self.settings
    }

    pub(in crate::application) fn tool_mut(&mut self) -> &mut ToolState {
        &mut self.tool
    }

    pub fn status(&self) -> &super::StatusMessage {
        &self.status.message
    }

    pub fn camera(&self) -> &OrbitCamera {
        &self.view.camera
    }

    pub fn viewport_gizmo_visible(&self) -> bool {
        self.view.gizmo_visible
    }

    pub fn viewport_shading(&self) -> ViewportShading {
        self.view.shading
    }

    pub fn viewport_wireframe_visible(&self) -> bool {
        self.view.viewport_wireframe_visible
    }

    pub fn viewport_wireframe_style(&self) -> WireframeStyle {
        self.view.viewport_wireframe
    }

    pub fn viewport_background_color(&self) -> [f32; 3] {
        self.view.background_color
    }

    pub fn viewport_scene_visibility(&self) -> &ViewportSceneVisibility {
        &self.view.scene_visibility
    }

    pub fn viewport_mesh_visible(&self, mesh_id: MeshId) -> bool {
        self.view.scene_visibility.mesh_visible(mesh_id)
    }

    pub fn viewport_material_visible(&self, material_index: usize) -> bool {
        self.view
            .scene_visibility
            .material_visible(material_index.into())
    }

    pub fn uv_wireframe_visible(&self) -> bool {
        self.view.uv_wireframe_visible
    }

    pub fn uv_wireframe_style(&self) -> WireframeStyle {
        self.view.uv_wireframe
    }

    pub fn uv_view_background_color(&self) -> [f32; 3] {
        self.view.uv_background_color
    }

    pub fn uv_view_transform(&self) -> crate::core::uv_view::UvViewTransform {
        self.view.uv_view_transform
    }

    pub fn uv_view_transform_initialized(&self) -> bool {
        self.view.uv_view_transform_initialized
    }

    pub fn is_tool_idle(&self) -> bool {
        self.tool.is_idle()
    }

    pub fn can_edit_viewport_camera_settings(&self) -> bool {
        self.tool.is_idle()
            || self
                .tool
                .view_projection_decal_session()
                .is_some_and(|session| session.gesture.is_none())
    }

    pub fn uv_transform_preview(&self) -> Option<UvTransformPreview> {
        if !self
            .effective_tool_definition()
            .is_some_and(|tool| matches!(&tool.behavior, ToolBehavior::Transform))
        {
            return None;
        }
        self.tool
            .active_transform_preview()
            .or_else(|| super::transform_controller::idle_transform_preview(self))
    }

    pub fn uv_drag_preview(&self) -> Option<(Vec2, Vec2)> {
        self.tool.uv_drag_preview()
    }

    pub fn surface_rect_drag_preview(&self) -> Option<SurfaceRectDragPreview> {
        self.tool.surface_rect_drag_preview()
    }

    pub fn uv_lasso_drag_preview(&self) -> Option<Vec<Vec2>> {
        self.tool.uv_lasso_drag_preview()
    }

    pub fn surface_lasso_drag_preview(&self) -> Option<SurfaceLassoDragPreview> {
        self.tool.surface_lasso_drag_preview()
    }

    pub fn decal_options(&self) -> &DecalToolOptions {
        &self.tool.decal_options
    }

    pub fn decal_image(&self) -> Option<&Arc<DecalImageAsset>> {
        self.tool.decal_image.as_ref()
    }

    pub fn active_decal_transform(&self) -> Option<DecalTransform> {
        self.tool.decal_session().map(|session| session.transform)
    }

    pub fn active_decal_scene_visibility(&self) -> Option<&ViewportSceneVisibility> {
        self.tool
            .decal_session()
            .map(|session| &session.scene_visibility)
            .or_else(|| {
                self.tool
                    .view_projection_decal_session()
                    .map(|session| &session.scene_visibility)
            })
    }

    pub fn active_decal_handle_display_transform(&self) -> Option<DecalTransform> {
        let session = self.tool.decal_session()?;
        self.decal_handle_display_transform(session.transform, session.source_hit)
    }

    pub(crate) fn decal_handle_display_transform(
        &self,
        transform: DecalTransform,
        source_hit: SurfaceHit,
    ) -> Option<DecalTransform> {
        let mesh = &self.document()?.mesh;
        let mut face_normal = mesh.triangle_geometric_normal(source_hit.triangle_index)?;
        if face_normal.dot(transform.normal_world) < 0.0 {
            face_normal = -face_normal;
        }
        let visual_epsilon = (mesh.scene_diagonal() * 1.0e-5).max(1.0e-6);
        transform.handle_display_transform(source_hit.world_pos, face_normal, visual_epsilon)
    }

    pub fn has_active_decal_session(&self) -> bool {
        self.tool.decal_session().is_some() || self.tool.view_projection_decal_session().is_some()
    }

    pub fn view_projection_decal_start_requested(&self) -> bool {
        self.tool.view_projection_decal_start_requested
    }

    pub fn active_view_projection_decal_transform(&self) -> Option<ViewProjectionDecalTransform> {
        self.tool
            .view_projection_decal_session()
            .map(|session| session.transform)
    }

    pub fn active_view_projection_decal_geometry(
        &self,
    ) -> Option<crate::core::decal::DecalHandleGeometry> {
        self.active_view_projection_decal_transform()?
            .handle_geometry()
    }

    pub fn active_decal_handle(&self) -> Option<TransformHandle> {
        self.tool
            .decal_session()
            .and_then(|session| session.gesture.map(|gesture| gesture.handle))
            .or_else(|| {
                self.tool
                    .view_projection_decal_session()
                    .and_then(|session| session.gesture.map(|gesture| gesture.handle))
            })
    }

    pub fn fill_options(&self) -> &FillToolOptions {
        &self.tool.fill_options
    }

    pub fn shape_options(&self) -> &ShapeToolOptions {
        &self.tool.shape_options
    }

    pub fn selection_options(&self) -> &SelectionToolOptions {
        &self.tool.selection_options
    }

    pub fn color_picker_options(&self) -> ColorPickerToolOptions {
        self.tool.color_picker_options
    }

    pub fn surface_mirror_options(&self) -> SurfaceMirrorOptions {
        SurfaceMirrorOptions {
            x_enabled: self.view.surface_mirror_x_enabled,
            x_plane: self.view.surface_mirror_x_plane,
            show_x_plane: self.view.surface_mirror_x_plane_visible,
        }
    }

    pub(in crate::application) fn active_surface_mirror_plane_x(&self) -> Option<f32> {
        let ToolSession::Stroke(session) = &self.tool.tool_session else {
            return None;
        };
        if session.space != StrokeSpace::Surface {
            return None;
        }
        session
            .viewport
            .as_ref()
            .and_then(|viewport| viewport.mirror_x)
            .map(|mirror| mirror.plane_x)
    }

    pub fn current_color(&self) -> [f32; 3] {
        self.tool.current_color
    }

    pub(in crate::application) fn set_status_message(&mut self, status: super::StatusMessage) {
        self.status.message = status;
    }

    pub(in crate::application) fn set_status_key(&mut self, key: &'static str) {
        self.set_status_message(super::StatusMessage::localized(key));
    }

    pub fn active_tool_id(&self) -> ToolId {
        self.tool.active_tool_id()
    }

    pub fn effective_tool_id(&self) -> ToolId {
        if self.active_layer_target() == ActiveLayerTarget::EmbeddedImage {
            ToolId::Transform
        } else {
            self.tool.effective_tool_id()
        }
    }

    pub fn panel_tool_id(&self) -> ToolId {
        if self.active_layer_target() == ActiveLayerTarget::EmbeddedImage {
            ToolId::Transform
        } else {
            self.tool.panel_tool_id()
        }
    }

    pub fn tool_id_by_config_id(&self, config_id: &str) -> Option<ToolId> {
        self.tool.tool_id_by_config_id(config_id)
    }

    pub fn representative_tool_id_for_group(&self, group_id: &str) -> Option<ToolId> {
        self.tool.representative_tool_id_for_group(group_id)
    }

    pub fn tool_catalog(&self) -> &[ToolDefinition] {
        self.tool.tool_catalog()
    }

    pub fn tool_shelf(&self) -> &ToolShelf {
        self.tool.tool_shelf()
    }

    pub fn representative_tool_id(&self, group_index: usize) -> Option<ToolId> {
        self.tool.representative_tool_id(group_index)
    }

    pub(crate) fn restore_persistent_tool_selection<'a>(
        &mut self,
        active_group_id: Option<&str>,
        saved_groups: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    ) {
        self.tool
            .restore_persistent_selection(active_group_id, saved_groups);
    }

    pub fn active_layer_id(&self) -> LayerId {
        self.document.active_layer_id()
    }

    pub fn active_layer_target(&self) -> ActiveLayerTarget {
        self.document.active_layer_target()
    }

    pub fn project_editor_state(
        &self,
        collapsed_layer_group_ids: Vec<LayerId>,
    ) -> Option<ProjectEditorState> {
        let document = self.document()?;
        let editor = self.resolved_editor_document(document);
        Some(ProjectEditorState {
            document_focus: DocumentFocusState {
                active_layer_id: editor.active_layer_id,
                active_layer_part: editor.active_part,
                focused_material_index: editor.focused_material_index.as_usize(),
            },
            view: EditorViewState {
                camera: self.view.camera.clone(),
                shading: self.view.shading,
                gizmo_visible: self.view.gizmo_visible,
                background_color: self.view.background_color,
                viewport_wireframe_visible: self.view.viewport_wireframe_visible,
                viewport_wireframe: self.view.viewport_wireframe,
                scene_visibility: self.view.scene_visibility.clone(),
                surface_mirror_x_enabled: self.view.surface_mirror_x_enabled,
                surface_mirror_x_plane: self.view.surface_mirror_x_plane,
                surface_mirror_x_plane_visible: self.view.surface_mirror_x_plane_visible,
                uv_view_transform: self.view.uv_view_transform,
                uv_background_color: self.view.uv_background_color,
                uv_wireframe_visible: self.view.uv_wireframe_visible,
                uv_wireframe: self.view.uv_wireframe,
            },
            layer_tree: LayerTreeEditorState {
                collapsed_layer_group_ids,
            },
        })
    }

    pub(in crate::application) fn replace_document_with_editor(
        &mut self,
        document: Document,
        editor_document: EditorDocumentState,
    ) {
        self.document.replace(document, editor_document);
        self.view = ViewState::default();
        self.tool.clear_session();
        self.clear_history();
    }

    pub(in crate::application) fn clear_document_state(&mut self) {
        self.document.clear();
        self.view.scene_visibility.clear();
        self.tool.clear_session();
        self.clear_history();
    }

    pub(in crate::application) fn finish_mesh_reload(&mut self) {
        self.document.mark_reloaded();
        self.view.scene_visibility.clear_meshes();
        self.tool.clear_session();
        self.tool.invalidate_transform_idle_cache();
        self.adjustment_filter_session = None;
        self.clear_history();
    }

    pub fn load_asset(&mut self, asset: ImportedAsset) {
        self.document.load_asset(asset);
        self.view.scene_visibility.clear();
        self.tool.clear_session();
        self.clear_history();
    }

    pub fn set_focused_material(&mut self, material_index: usize) -> bool {
        self.document.set_focused_material(material_index)
    }

    /// [NOTE: Design Architecture]
    /// `focused_material_index` (and the derived `active_paint_surface()`) is a pure UI state
    /// indicating which material is currently displayed in the UV View.
    /// DO NOT use this as a condition to filter paint targets or skip rendering in the Viewport (3D space).
    /// 3D viewport painting should inherently be applied across all materials in the scene.
    pub fn focused_material_index(&self) -> usize {
        self.document.focused_material_index()
    }

    pub fn active_paint_surface(&self) -> Option<PaintSurfaceId> {
        self.document.active_paint_surface()
    }

    pub fn active_paint_surfaces(&self) -> Vec<PaintSurfaceId> {
        self.document.active_paint_surfaces()
    }

    pub fn active_paint_surfaces_for_mesh(&self, mesh_id: MeshId) -> Vec<PaintSurfaceId> {
        self.document.active_paint_surfaces_for_mesh(mesh_id)
    }

    pub fn active_tool_preset(&self) -> Option<&StrokeToolPreset> {
        self.tool.active_tool_preset()
    }

    pub fn effective_tool_preset(&self) -> Option<&StrokeToolPreset> {
        self.tool.effective_tool_preset()
    }

    #[cfg(test)]
    pub(crate) fn default_brush_preset(&self) -> &StrokeToolPreset {
        self.tool.default_brush_preset()
    }

    pub(in crate::application) fn effective_brush_radius_world(&self) -> Option<f32> {
        let scene_diagonal = self
            .document()
            .map(|document| document.mesh.scene_diagonal())
            .unwrap_or(0.0);
        Some(
            self.effective_tool_preset()?
                .stroke_op
                .radius_world(scene_diagonal),
        )
    }

    pub fn stroke_tool_preset(&self, index: usize) -> Option<&StrokeToolPreset> {
        self.tool.stroke_tool_preset(index)
    }

    pub fn brush_engines(&self) -> &BrushEngineRegistry {
        self.tool.brush_engines()
    }

    pub(crate) fn brush_presets(&self) -> &BrushPresetCatalog {
        &self.tool.brush_presets
    }

    pub(crate) fn brush_engine_users(&self, id: &str) -> Vec<String> {
        self.tool.brush_engine_users(id)
    }

    pub(crate) fn delete_user_brush_engine(&mut self, id: &str) -> anyhow::Result<()> {
        self.tool.delete_user_brush_engine(id)?;
        self.settings.brush_engines = self.tool.brush_engines.user_definitions();
        Ok(())
    }

    pub(crate) fn import_holopack_brush_resources(
        &mut self,
        engines: Vec<crate::core::brush_engine::BrushEngineDefinition>,
        presets: Vec<BrushPresetDefinition>,
    ) -> anyhow::Result<()> {
        self.tool
            .import_holopack_brush_resources(engines, presets)?;
        self.settings.brush_engines = self.tool.brush_engines.user_definitions();
        Ok(())
    }

    pub(crate) fn user_brush_resource_snapshot(&self) -> UserBrushResourceSnapshot {
        UserBrushResourceSnapshot {
            tool: self.tool.clone(),
            settings: self.settings.clone(),
        }
    }

    pub(crate) fn restore_user_brush_resource_snapshot(
        &mut self,
        snapshot: UserBrushResourceSnapshot,
    ) {
        self.tool = snapshot.tool;
        self.settings = snapshot.settings;
    }

    pub fn brush_textures(&self) -> &TextureCatalog {
        self.tool.brush_textures()
    }

    pub(crate) fn insert_brush_texture(
        &mut self,
        definition: crate::core::texture::TextureResourceDefinition,
    ) -> anyhow::Result<()> {
        self.tool.insert_brush_texture(definition)
    }

    pub(crate) fn remove_brush_texture(
        &mut self,
        id: &str,
    ) -> Option<crate::core::texture::TextureResourceDefinition> {
        self.tool.remove_brush_texture(id)
    }

    pub(crate) fn brush_texture_users(&self, id: &str) -> Vec<String> {
        self.tool.brush_texture_users(id)
    }

    pub(crate) fn set_brush_texture_display_name(&mut self, id: &str, display_name: &str) -> bool {
        self.tool.set_brush_texture_display_name(id, display_name)
    }

    pub(crate) fn user_brush_preset_definitions(&self) -> Vec<BrushPresetDefinition> {
        self.tool.user_brush_preset_definitions()
    }

    pub(crate) fn tool_layout_snapshot(&self) -> ToolLayoutFileV1 {
        self.tool.tool_layout_snapshot()
    }

    pub(crate) fn tool_layout_groups(&self) -> &[ToolGroupDefinition] {
        self.tool.tool_layout_groups()
    }

    pub(crate) fn brush_engine_defaults(
        &self,
        preset_index: usize,
        preset_id: &str,
    ) -> anyhow::Result<StrokeToolPreset> {
        self.tool.brush_engine_defaults(preset_index, preset_id)
    }

    pub fn active_tool_preset_mut(&mut self) -> Option<&mut StrokeToolPreset> {
        self.tool.active_tool_preset_mut()
    }

    pub fn set_active_tool_preset_index(&mut self, index: usize) {
        self.tool.set_active_tool_preset_index(index);
    }

    pub fn active_tool_definition(&self) -> Option<&ToolDefinition> {
        self.tool.active_tool_definition()
    }

    pub fn effective_tool_definition(&self) -> Option<&ToolDefinition> {
        let tool_id = self.effective_tool_id();
        self.tool_catalog().iter().find(|tool| tool.id == tool_id)
    }

    pub fn panel_tool_definition(&self) -> Option<&ToolDefinition> {
        let tool_id = self.panel_tool_id();
        self.tool_catalog().iter().find(|tool| tool.id == tool_id)
    }

    pub fn active_stroke_preset_index(&self) -> Option<usize> {
        self.tool.active_stroke_preset_index()
    }

    pub fn set_active_tool(&mut self, tool_id: ToolId) {
        self.tool.set_active_tool(tool_id);
    }

    pub fn is_stroking(&self) -> bool {
        self.tool.is_stroking()
    }

    pub fn active_stroke_next_emit_time_s(&self) -> Option<f64> {
        self.tool
            .active_stroke_session()
            .and_then(|session| session.continuous.as_ref())
            .and_then(|continuous| continuous.next_emit_time_s)
    }

    pub fn is_tool_interacting(&self) -> bool {
        self.tool.is_interacting()
    }

    pub fn is_tool_pointer_gesture_active(&self) -> bool {
        self.tool.is_pointer_gesture_active()
    }

    pub fn has_active_transform_session(&self) -> bool {
        self.tool.transform_session().is_some()
            || self.tool.embedded_image_transform_session().is_some()
    }

    pub fn has_active_raster_transform_session(&self) -> bool {
        self.tool.transform_session().is_some()
    }

    pub fn transform_numeric_values(
        &self,
    ) -> Option<crate::core::transform::TransformNumericValues> {
        super::transform_controller::transform_numeric_values(self)
    }

    pub fn active_transform_preserves_aspect(&self) -> bool {
        self.tool.transform_session().is_some_and(|session| {
            session.scale_constraint == TransformScaleConstraint::PreserveAspect
        })
    }

    pub fn has_active_modal_tool(&self) -> bool {
        self.tool.has_active_modal_tool()
    }

    pub(in crate::application) fn transient_tool_override_ids(&self) -> impl Iterator<Item = &str> {
        self.tool.transient_tool_override_ids()
    }

    pub fn stroking_space(&self) -> Option<StrokeSpace> {
        match &self.tool.tool_session {
            ToolSession::Idle => None,
            ToolSession::Stroke(session) => Some(session.space),
            ToolSession::Fill(session) => Some(session.input_space.space()),
            ToolSession::ShapeDrag(session) => Some(session.gesture.space()),
            ToolSession::SelectionDrag(session) => Some(session.gesture.space()),
            ToolSession::Decal(_) => Some(StrokeSpace::Surface),
            ToolSession::ViewProjectionDecal(_) => Some(StrokeSpace::Surface),
            ToolSession::Transform(_) => Some(StrokeSpace::Uv),
            ToolSession::EmbeddedImageTransform(_) => Some(StrokeSpace::Uv),
        }
    }

    pub(in crate::application) fn pop_undo_history(&mut self) -> Option<HistoryTransaction> {
        self.history.pop_undo()
    }

    pub(in crate::application) fn pop_redo_history(&mut self) -> Option<HistoryTransaction> {
        self.history.pop_redo()
    }

    pub(in crate::application) fn push_undo_history_raw(&mut self, entry: HistoryTransaction) {
        self.history.push_undo_raw(entry);
    }

    pub(in crate::application) fn push_redo_history_raw(&mut self, entry: HistoryTransaction) {
        self.history.push_redo_raw(entry);
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn push_history(
        &mut self,
        entry: HistoryTransaction,
        max_entries: usize,
        max_bytes: usize,
    ) {
        self.history.push(entry, max_entries, max_bytes);
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }
}

impl HistoryStore {
    pub(in crate::application) fn pop_undo(&mut self) -> Option<HistoryTransaction> {
        self.undo_stack.pop()
    }

    pub(in crate::application) fn pop_redo(&mut self) -> Option<HistoryTransaction> {
        self.redo_stack.pop()
    }

    pub(in crate::application) fn push_undo_raw(&mut self, entry: HistoryTransaction) {
        self.undo_stack.push(entry);
    }

    pub(in crate::application) fn push_redo_raw(&mut self, entry: HistoryTransaction) {
        self.redo_stack.push(entry);
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    pub fn push(&mut self, entry: HistoryTransaction, max_entries: usize, max_bytes: usize) {
        if entry.is_noop() {
            return;
        }
        if let Some(last) = self.undo_stack.last_mut() {
            if last.try_merge(&entry) {
                if last.is_noop() {
                    self.undo_stack.pop();
                }
                self.redo_stack.clear();
                self.trim(max_entries, max_bytes);
                return;
            }
        }
        self.undo_stack.push(entry);
        self.redo_stack.clear();
        self.trim(max_entries, max_bytes);
    }

    fn trim(&mut self, max_entries: usize, max_bytes: usize) {
        if self.undo_stack.len() > max_entries {
            let overflow = self.undo_stack.len() - max_entries;
            self.undo_stack.drain(0..overflow);
        }
        while history_bytes(&self.undo_stack) > max_bytes && self.undo_stack.len() > 1 {
            self.undo_stack.remove(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

impl EditorDocumentState {
    pub(in crate::application) fn from_document(document: &Document) -> Self {
        Self {
            focused_material_index: 0.into(),
            active_layer_id: document
                .layer_tree
                .default_raster_layer()
                .unwrap_or_else(|| document.layer_tree.root()),
            active_part: crate::core::document::ActiveLayerPart::Content,
        }
    }

    pub(in crate::application) fn resolved(&self, document: &Document) -> Self {
        use slotmap::Key;

        let mut resolved = *self;
        if resolved.active_layer_id.is_null()
            || !document.layer_tree.contains(resolved.active_layer_id)
        {
            resolved.active_layer_id = document
                .layer_tree
                .default_raster_layer()
                .unwrap_or_else(|| document.layer_tree.root());
            resolved.active_part = crate::core::document::ActiveLayerPart::Content;
        }
        if resolved.active_part == crate::core::document::ActiveLayerPart::LayerMask
            && !document.layer_tree.has_layer_mask(resolved.active_layer_id)
        {
            resolved.active_part = crate::core::document::ActiveLayerPart::Content;
        }
        if resolved.focused_material_index.as_usize() >= document.materials.len() {
            resolved.focused_material_index = 0.into();
        }
        resolved
    }

    pub(in crate::application) fn active_target(&self, document: &Document) -> ActiveLayerTarget {
        if self.active_part == crate::core::document::ActiveLayerPart::LayerMask
            && document.layer_tree.has_layer_mask(self.active_layer_id)
        {
            ActiveLayerTarget::LayerMask
        } else {
            document.content_target(self.active_layer_id)
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec3};

    use crate::core::{
        adjustment::AdjustmentKind,
        document::{
            ActiveLayerPart, ActiveLayerTarget, Document, ImportedAsset, LayerInsertion,
            MaterialSpec, MeshData, MeshId, MeshObject, SubMesh,
        },
        surface::{LayerMaterialMask, PaintSurfaceRole},
        tool::ToolId,
        tool_catalog::{load_test_tools_from_fixtures, resolve_default_tools},
        tool_layout::{ToolEntryDefinition, ToolGroupDefinition, ToolLayoutFileV1},
    };

    use super::{
        AppState, EditorDocumentState, ModalToolMode, PaintTargetScope, ToolState,
        reorder_visible_tool_entries,
    };
    use crate::application::PaintEditBlockReason;

    fn state_with_default_layer() -> (AppState, crate::core::surface::LayerId) {
        state_with_materials(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])])
    }

    fn state_with_materials(
        mesh: MeshData,
        materials: Vec<MaterialSpec>,
    ) -> (AppState, crate::core::surface::LayerId) {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(mesh, materials));
        state.document.editor = EditorDocumentState::from_document(state.document().unwrap());
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        (state, layer_id)
    }

    fn two_material_mesh(mesh_id: MeshId) -> MeshData {
        MeshData::new(
            vec![
                Vec3::ZERO,
                Vec3::X,
                Vec3::Y,
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
                Vec3::new(2.0, 1.0, 0.0),
            ],
            vec![Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ZERO, Vec2::X, Vec2::Y],
            vec![Vec3::Z; 6],
            vec![[0, 1, 2], [3, 4, 5]],
            vec![
                SubMesh {
                    mesh_id,
                    start_index: 0,
                    index_count: 3,
                    material_index: 0,
                    material_name: "A".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id,
                    start_index: 3,
                    index_count: 3,
                    material_index: 1,
                    material_name: "B".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![MeshObject {
                id: mesh_id,
                name: "Mesh".to_owned(),
            }],
            vec![mesh_id, mesh_id],
        )
        .expect("two-material test mesh")
    }

    #[test]
    fn brush_texture_users_find_runtime_resource_references() {
        let state = AppState::default();
        assert!(
            !state
                .brush_texture_users("texture.brush_tip.brush")
                .is_empty()
        );
        assert!(state.brush_texture_users("texture.missing").is_empty());
    }

    #[test]
    fn visible_tool_entry_reorder_preserves_separator_slots() {
        use ToolEntryDefinition::{BuiltinTool, Separator};
        let mut single_separator = vec![
            BuiltinTool("A".to_owned()),
            Separator,
            BuiltinTool("B".to_owned()),
            BuiltinTool("C".to_owned()),
        ];
        reorder_visible_tool_entries(&mut single_separator, 1, 0).expect("move B to front");
        assert_eq!(
            single_separator,
            vec![
                BuiltinTool("B".to_owned()),
                Separator,
                BuiltinTool("A".to_owned()),
                BuiltinTool("C".to_owned()),
            ]
        );

        let mut entries = vec![
            BuiltinTool("A".to_owned()),
            Separator,
            BuiltinTool("B".to_owned()),
            Separator,
            BuiltinTool("C".to_owned()),
        ];

        reorder_visible_tool_entries(&mut entries, 2, 0).expect("move C to front");

        assert_eq!(
            entries,
            vec![
                BuiltinTool("C".to_owned()),
                Separator,
                BuiltinTool("A".to_owned()),
                Separator,
                BuiltinTool("B".to_owned()),
            ]
        );
    }

    #[test]
    fn visible_tool_entry_reorder_validates_indices_without_mutating() {
        use ToolEntryDefinition::{BuiltinTool, Separator};
        let entries = vec![BuiltinTool("A".to_owned()), Separator];
        let mut reordered = entries.clone();

        assert!(reorder_visible_tool_entries(&mut reordered, 1, 0).is_err());
        assert_eq!(reordered, entries);
        assert!(reorder_visible_tool_entries(&mut reordered, 0, 1).is_err());
        assert_eq!(reordered, entries);
    }

    #[test]
    fn document_replacement_and_clear_reset_viewport_visibility() {
        let mesh_id = MeshId(7);
        let mesh = two_material_mesh(mesh_id);
        let materials = vec![
            MaterialSpec::new("A", [8, 8]),
            MaterialSpec::new("B", [8, 8]),
        ];
        let (mut state, _) = state_with_materials(mesh.clone(), materials.clone());
        state.view.scene_visibility.set_mesh_visible(mesh_id, false);
        state
            .view
            .scene_visibility
            .set_material_visible(1.into(), false);

        let replacement = Document::new(mesh.clone(), materials.clone());
        let editor = EditorDocumentState::from_document(&replacement);
        state.replace_document_with_editor(replacement, editor);
        assert!(state.viewport_mesh_visible(mesh_id));
        assert!(state.viewport_material_visible(1));

        state.view.scene_visibility.set_mesh_visible(mesh_id, false);
        state.clear_document_state();
        assert!(state.viewport_mesh_visible(mesh_id));

        state
            .view
            .scene_visibility
            .set_material_visible(0.into(), false);
        state.load_asset(ImportedAsset { mesh, materials });
        assert!(state.viewport_material_visible(0));
    }

    #[test]
    fn tool_groups_initially_use_their_first_tool_as_representative() {
        let state = AppState::default();

        for (group_index, group) in state.tool_shelf().groups.iter().enumerate() {
            assert_eq!(
                state.representative_tool_id(group_index),
                group.entries.first().map(|entry| entry.tool_id)
            );
        }
        assert_eq!(
            state.active_tool_id(),
            state.tool_shelf().groups[0].entries[0].tool_id
        );
    }

    #[test]
    fn tool_state_can_initialize_with_an_empty_first_group() {
        let defaults = load_test_tools_from_fixtures().unwrap();
        let layout = ToolLayoutFileV1 {
            schema_version: crate::core::tool_layout::TOOL_LAYOUT_SCHEMA_VERSION,
            id: "test.empty_first".to_owned(),
            display_name: "Empty First".to_owned(),
            tools: vec![ToolGroupDefinition {
                id: "tool.empty".to_owned(),
                display_name: "Empty".to_owned(),
                icon: None,
                entries: Vec::new(),
            }],
        };
        let resolved = resolve_default_tools(
            &defaults.brush_engines,
            &defaults.brush_presets,
            &defaults.brush_textures,
            &layout,
        )
        .expect("empty layout group should resolve");

        let state = ToolState::new(resolved);
        assert_eq!(state.active_tool_id(), ToolId::EmptyGroup(0));
        assert_eq!(state.representative_tool_id(0), Some(ToolId::EmptyGroup(0)));
        assert!(state.active_tool_preset().is_none());
        assert!(state.tool_presets.is_empty());
    }

    #[test]
    fn activating_a_tool_updates_only_its_group_representative() {
        let mut state = AppState::default();
        let smudge_tool = state
            .tool_catalog()
            .iter()
            .find(|tool| tool.config_id == "builtin.brush.smudge.soft")
            .expect("smudge brush preset")
            .id;
        let filter_brush_group_index = state
            .tool_shelf()
            .group_containing(smudge_tool)
            .expect("filter brush tool group");
        let selection_group_index = state
            .tool_shelf()
            .group_containing(ToolId::RectangleSelection)
            .expect("selection tool group");
        let selection_tool = state.tool_shelf().groups[selection_group_index].entries[1].tool_id;

        state.set_active_tool(smudge_tool);
        assert_eq!(
            state.representative_tool_id(filter_brush_group_index),
            Some(smudge_tool)
        );

        state.set_active_tool(selection_tool);
        assert_eq!(state.active_tool_id(), selection_tool);
        assert_eq!(
            state.representative_tool_id(filter_brush_group_index),
            Some(smudge_tool)
        );
        assert_eq!(
            state.representative_tool_id(selection_group_index),
            Some(selection_tool)
        );
    }

    #[test]
    fn editor_session_capture_ignores_temporary_tool_state() {
        let mut state = AppState::default();
        let selected = state
            .tool_id_by_config_id("builtin.brush.paint.directional_flat")
            .expect("directional brush");
        let temporary = state
            .tool_id_by_config_id("builtin.tool.fill.mesh")
            .expect("mesh fill");
        state.set_active_tool(selected);
        state
            .tool
            .begin_momentary_tool(crate::application::InputHoldToken(1), temporary);
        state.tool.modal_tool = Some(ModalToolMode::Transform);
        state.tool.begin_transient_tool_override(
            crate::application::InputHoldToken(2),
            "test.transient".to_owned(),
        );

        let captured = crate::editor_session::EditorSessionFileV1::capture(&state)
            .expect("capture editor session");

        assert_eq!(
            captured.tool_selection.active_group_id.as_deref(),
            Some("tool.brush")
        );
        assert_eq!(
            captured
                .tool_selection
                .groups
                .iter()
                .find(|group| group.group_id == "tool.brush")
                .and_then(|group| group.selected_entry_id.as_deref()),
            Some("builtin.brush.paint.directional_flat")
        );
    }

    #[test]
    fn edit_paint_target_accepts_visible_raster_layer() {
        let (state, _layer_id) = state_with_default_layer();

        assert_eq!(state.paint_edit_permission(None).blocked_reason(), None);
        assert!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .is_some()
        );
    }

    #[test]
    fn edit_paint_target_filters_all_material_scopes_by_effective_mask() {
        let mesh_id = MeshId(7);
        let (mut state, layer_id) = state_with_materials(
            two_material_mesh(mesh_id),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        );
        let material_b = state.document().unwrap().material_id(1.into()).unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([material_b].into_iter().collect()),
            );

        state.document.set_focused_material(0);
        assert!(
            state
                .document
                .resolve_paint_target(PaintTargetScope::FocusedMaterial)
                .is_some()
        );
        assert!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .is_none()
        );

        state.document.set_focused_material(1);
        assert_eq!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .unwrap()
                .surfaces_vec()
                .iter()
                .map(|surface| surface.material_index().as_usize())
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::Material(0.into()))
                .into_allowed()
                .is_none()
        );
        assert_eq!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::Materials(vec![
                    1.into(),
                    0.into(),
                    1.into(),
                    99.into(),
                ]))
                .into_allowed()
                .unwrap()
                .surfaces_vec()
                .iter()
                .map(|surface| surface.material_index().as_usize())
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
                .into_allowed()
                .unwrap()
                .surfaces_vec()
                .iter()
                .map(|surface| surface.material_index().as_usize())
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::Mesh(mesh_id))
                .into_allowed()
                .unwrap()
                .surfaces_vec()
                .iter()
                .map(|surface| surface.material_index().as_usize())
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn uv_paint_edit_reports_excluded_focused_material_without_blocking_surface_space() {
        let (mut state, layer_id) = state_with_materials(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        );
        let material_b = state.document().unwrap().material_id(1.into()).unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([material_b].into_iter().collect()),
            );

        assert_eq!(state.paint_edit_permission(None).blocked_reason(), None);
        assert_eq!(
            state
                .paint_edit_permission(Some(state.focused_material_index()))
                .blocked_reason(),
            Some(PaintEditBlockReason::MaterialExcluded)
        );
        assert_eq!(state.paint_edit_permission(None).blocked_reason(), None);

        assert!(state.set_focused_material(1));
        assert_eq!(
            state
                .paint_edit_permission(Some(state.focused_material_index()))
                .blocked_reason(),
            None
        );
    }

    #[test]
    fn edit_paint_target_filters_layer_mask_surfaces_without_hiding_reference_targets() {
        let (mut state, layer_id) = state_with_materials(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        );
        let material_a = state.document().unwrap().material_id(0.into()).unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_layer_mask(layer_id);
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([material_a].into_iter().collect()),
            );
        state.document.select_layer_mask_if_present(layer_id);

        let reference_target = state
            .document
            .resolve_paint_target(PaintTargetScope::AllMaterials)
            .unwrap();
        assert_eq!(reference_target.surfaces_vec().len(), 2);
        assert!(
            reference_target
                .surfaces_vec()
                .iter()
                .all(|surface| surface.role == PaintSurfaceRole::LayerMask)
        );

        let edit_target = state
            .document
            .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
            .into_allowed()
            .unwrap();
        assert_eq!(edit_target.surfaces_vec().len(), 1);
        assert_eq!(edit_target.target().material_index().as_usize(), 0);
        assert_eq!(edit_target.target().role, PaintSurfaceRole::LayerMask);
    }

    #[test]
    fn edit_paint_target_rejects_hidden_raster_layer() {
        let (mut state, layer_id) = state_with_default_layer();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_visible(layer_id, false);

        assert_eq!(
            state.paint_edit_permission(None).blocked_reason(),
            Some(PaintEditBlockReason::ActiveLayerHidden)
        );
        assert!(
            state
                .document
                .resolve_paint_target(PaintTargetScope::FocusedMaterial)
                .is_some()
        );
        assert!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .is_none()
        );
    }

    #[test]
    fn edit_paint_target_rejects_mask_when_owner_layer_hidden() {
        let (mut state, layer_id) = state_with_default_layer();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_layer_mask(layer_id);
        state.document.select_layer_mask_if_present(layer_id);

        let visible_target = state
            .document
            .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
            .into_allowed()
            .expect("visible layer mask should be editable");
        assert_eq!(visible_target.target().role, PaintSurfaceRole::LayerMask);

        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_visible(layer_id, false);

        assert_eq!(
            state.paint_edit_permission(None).blocked_reason(),
            Some(PaintEditBlockReason::ActiveLayerHidden)
        );
        assert!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .is_none()
        );
    }

    #[test]
    fn edit_paint_target_rejects_group_selection() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        state.document.select_structure_layer_if_present(group);

        assert_eq!(
            state.paint_edit_permission(None).blocked_reason(),
            Some(PaintEditBlockReason::ActiveLayerIsGroup)
        );
        assert!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
                .into_allowed()
                .is_none()
        );
    }

    #[test]
    fn structure_selection_rejects_raster_layers() {
        let (mut state, raster) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(raster, "Group")
            .unwrap();
        assert!(state.document.select_structure_layer_if_present(group));

        assert!(!state.document.select_structure_layer_if_present(raster));
        assert_eq!(state.document.active_layer_id(), group);
        assert_eq!(
            state.document.active_layer_target(),
            ActiveLayerTarget::Structure
        );
    }

    #[test]
    fn structure_selection_accepts_embedded_image_layers() {
        let (mut state, raster) = state_with_default_layer();
        let image = state
            .document_mut()
            .unwrap()
            .add_embedded_image_layer(
                LayerInsertion::Above(raster),
                "Image",
                "image.png".to_owned(),
                [1, 1],
                vec![255, 255, 255, 255].into(),
                0usize.into(),
            )
            .unwrap()
            .unwrap()
            .layer_id;

        assert!(state.document.select_structure_layer_if_present(image));
        assert_eq!(state.document.active_layer_id(), image);
        assert_eq!(
            state.document.active_layer_target(),
            ActiveLayerTarget::EmbeddedImage
        );
    }

    #[test]
    fn edit_paint_target_accepts_visible_group_mask() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_layer_mask(group);
        state.document.select_layer_mask_if_present(group);

        let visible_target = state
            .document
            .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
            .into_allowed()
            .expect("visible group mask should be editable");
        assert_eq!(visible_target.target().role, PaintSurfaceRole::LayerMask);
    }

    #[test]
    fn resolved_group_mask_selection_falls_back_to_structure_after_mask_delete() {
        let (mut state, layer_id) = state_with_default_layer();
        let group = state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_group_above(layer_id, "Group")
            .unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .add_layer_mask(group);
        state.document.select_layer_mask_if_present(group);
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .delete_layer_mask(group);

        let resolved = state.document.editor.resolved(state.document().unwrap());
        assert_eq!(resolved.active_layer_id, group);
        assert_eq!(
            resolved.active_target(state.document().unwrap()),
            ActiveLayerTarget::Structure
        );
        assert_eq!(resolved.active_part, ActiveLayerPart::Content);
    }

    #[test]
    fn content_target_is_derived_from_the_selected_layer() {
        let (mut state, raster) = state_with_default_layer();
        let (solid_fill, adjustment, group) = {
            let tree = &mut state.document_mut().unwrap().layer_tree;
            let solid_fill = tree
                .add_solid_fill_layer_above(raster, "Fill", [0.25, 0.5, 0.75])
                .unwrap();
            let adjustment = tree
                .add_adjustment_layer_above(
                    solid_fill,
                    "Adjustment",
                    AdjustmentKind::BrightnessContrast.default_adjustment(),
                )
                .unwrap();
            let group = tree.add_group_above(adjustment, "Group").unwrap();
            (solid_fill, adjustment, group)
        };

        for (layer_id, expected) in [
            (raster, ActiveLayerTarget::Raster),
            (solid_fill, ActiveLayerTarget::SolidFill),
            (adjustment, ActiveLayerTarget::Adjustment),
            (group, ActiveLayerTarget::Structure),
        ] {
            state.document.editor.active_layer_id = layer_id;
            state.document.editor.active_part = ActiveLayerPart::Content;

            assert_eq!(state.document.active_layer_target(), expected);
            assert_eq!(state.document.editor.active_part, ActiveLayerPart::Content);
        }
    }
}

fn history_bytes(entries: &[HistoryTransaction]) -> usize {
    entries
        .iter()
        .map(HistoryTransaction::byte_len)
        .fold(0usize, usize::saturating_add)
}

#[cfg(test)]
mod spatial_gesture_tests {
    use super::*;

    #[test]
    fn user_brush_engine_import_updates_settings_and_delete_removes_it() {
        let mut state = AppState::default();
        let mut engine = state.brush_engines().get("paint").unwrap().clone();
        engine.id = "user.engine.imported_test".to_owned();
        state
            .import_holopack_brush_resources(vec![engine], Vec::new())
            .unwrap();
        assert!(
            state
                .user_settings()
                .brush_engines
                .iter()
                .any(|engine| engine.id == "user.engine.imported_test")
        );
        state
            .delete_user_brush_engine("user.engine.imported_test")
            .unwrap();
        assert!(
            state
                .brush_engines()
                .get("user.engine.imported_test")
                .is_none()
        );
        assert!(state.user_settings().brush_engines.is_empty());
    }

    #[test]
    fn user_brush_engine_delete_is_blocked_by_imported_preset_reference() {
        let mut state = AppState::default();
        let mut engine = state.brush_engines().get("paint").unwrap().clone();
        engine.id = "user.engine.referenced_test".to_owned();
        let preset_source = crate::embedded_resources::text("brushes/presets/basic_brush.ron")
            .unwrap()
            .replace(
                "builtin.brush.paint.basic_brush",
                "user.brush.reference_test",
            )
            .replace(
                "engine_id: \"paint\"",
                "engine_id: \"user.engine.referenced_test\"",
            );
        let preset =
            BrushPresetDefinition::from_ron_bytes(preset_source.as_bytes(), "reference_test.ron")
                .unwrap();
        state
            .import_holopack_brush_resources(vec![engine], vec![preset])
            .unwrap();
        let error = state
            .delete_user_brush_engine("user.engine.referenced_test")
            .unwrap_err();
        assert!(error.to_string().contains("used by"));
    }

    #[test]
    fn polyline_keeps_uv_view_context_and_appends_points() {
        let mut gesture = SpatialGesture::polyline(
            GestureSpaceContext::Uv {
                view_size: [640, 480],
            },
            Vec2::new(1.0, 2.0),
        );
        gesture.update(Vec2::new(3.0, 4.0));
        assert_eq!(gesture.space(), StrokeSpace::Uv);
        let GestureGeometry::Polyline { points, .. } = &gesture.geometry else {
            panic!("expected polyline gesture");
        };
        assert_eq!(points, &[Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0)]);
        assert!(matches!(
            gesture.space,
            GestureSpaceContext::Uv {
                view_size: [640, 480]
            }
        ));
    }

    #[test]
    fn polyline_samples_only_after_two_pixels() {
        let mut gesture = SpatialGesture::polyline(
            GestureSpaceContext::Uv {
                view_size: [100, 100],
            },
            Vec2::ZERO,
        );
        gesture.update(Vec2::new(0.019, 0.0));
        let GestureGeometry::Polyline { points, .. } = &gesture.geometry else {
            panic!("expected polyline gesture");
        };
        assert_eq!(points, &[Vec2::ZERO]);

        gesture.update(Vec2::new(0.02, 0.0));
        let GestureGeometry::Polyline { points, .. } = &gesture.geometry else {
            panic!("expected polyline gesture");
        };
        assert_eq!(points, &[Vec2::ZERO, Vec2::new(0.02, 0.0)]);
    }

    #[test]
    fn polyline_caps_sampled_points_at_4096() {
        let mut gesture =
            SpatialGesture::polyline(GestureSpaceContext::Uv { view_size: [1, 1] }, Vec2::ZERO);
        for index in 1..=5000 {
            gesture.update(Vec2::new(index as f32 * 2.0, 0.0));
        }
        let GestureGeometry::Polyline { points, current } = &gesture.geometry else {
            panic!("expected polyline gesture");
        };
        assert_eq!(points.len(), 4096);
        assert_eq!(points.last().copied(), Some(Vec2::new(8190.0, 0.0)));
        assert_eq!(*current, Vec2::new(10000.0, 0.0));
    }
}

pub(in crate::application) fn begin_stroke_uv(
    first_dab: StrokeDab,
    descriptor: Arc<StrokeRenderDescriptor>,
    input_filter: StrokeInputFilterRuntime,
    continuous: Option<ContinuousStrokeRuntime>,
) -> ToolSession {
    ToolSession::Stroke(StrokeSession {
        descriptor,
        space: StrokeSpace::Uv,
        last_dab: Some(first_dab),
        last_surface_dab: None,
        continuous,
        stroke_damage: DamageMap::default(),
        pending_history_capture: None,
        has_drawn_dabs: false,
        input_filter,
        viewport: None,
    })
}

pub(in crate::application) fn begin_stroke_surface(
    first_dab: SurfaceDab,
    descriptor: Arc<StrokeRenderDescriptor>,
    input_filter: StrokeInputFilterRuntime,
    viewport: ViewportStrokeSessionData,
    continuous: Option<ContinuousStrokeRuntime>,
) -> ToolSession {
    ToolSession::Stroke(StrokeSession {
        descriptor,
        space: StrokeSpace::Surface,
        last_dab: None,
        last_surface_dab: Some(first_dab),
        continuous,
        stroke_damage: DamageMap::default(),
        pending_history_capture: None,
        has_drawn_dabs: false,
        input_filter,
        viewport: Some(viewport),
    })
}

#[cfg(test)]
mod continuous_stroke_runtime_tests {
    use glam::Vec2;

    use super::{ContinuousStrokeAnchor, ContinuousStrokeRuntime};
    use crate::core::stroke::StrokeDab;

    fn anchor(position: f32) -> ContinuousStrokeAnchor {
        ContinuousStrokeAnchor::Uv(StrokeDab::new(Vec2::splat(position), 1.0))
    }

    #[test]
    fn pause_and_resume_discard_elapsed_deadlines() {
        let mut runtime = ContinuousStrokeRuntime::new(anchor(0.0), 0.0, 10.0);
        runtime.update_anchor(None, 0.05, false);
        assert_eq!(runtime.next_emit_time_s, None);
        assert_eq!(runtime.take_due_count(10.0, 16), 0);

        runtime.update_anchor(Some(anchor(1.0)), 10.0, false);
        assert!((runtime.next_emit_time_s.unwrap() - 10.1).abs() < 1e-9);
        assert_eq!(runtime.take_due_count(10.05, 16), 0);
        assert_eq!(runtime.take_due_count(10.1, 16), 1);
    }

    #[test]
    fn due_count_is_independent_of_tick_frequency() {
        let mut slow_ticks = ContinuousStrokeRuntime::new(anchor(0.0), 0.0, 20.0);
        let mut fast_ticks = slow_ticks.clone();
        let mut slow_count = 0;
        let mut fast_count = 0;

        for time_s in [0.25, 0.5, 0.75, 1.0] {
            slow_count += slow_ticks.take_due_count(time_s, 16);
        }
        for step in 1..=100 {
            fast_count += fast_ticks.take_due_count(step as f64 / 100.0, 16);
        }

        assert_eq!(slow_count, 20);
        assert_eq!(fast_count, slow_count);
    }
}

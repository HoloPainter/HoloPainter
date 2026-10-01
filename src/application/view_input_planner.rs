use anyhow::Result;
use glam::{Mat4, Vec2, Vec3};

use crate::{
    application::{
        InputModifiers, PaintEditDecision, PointerSample, ReducerOutput, ToolInputEvent,
        UvViewInputContext, ViewPointerEvent, ViewPointerPhase, ViewportInputContext,
    },
    core::{
        camera::CameraProjection,
        decal::DecalProjection,
        document::{RaycastScratch, SurfaceHit},
        math::ray_from_viewport_px,
        stroke::{StrokeSpace, SurfaceDab},
        tool::{DecalKind, SurfaceHitRequirement, ToolBehavior},
    },
    renderer::{
        BrushOverlayRequest, DecalOverlayRequest, MirrorPlaneOverlayRequest,
        SurfaceBrushOverlayRequest, UvBrushOverlayRequest,
    },
};

use super::{
    state::{AppState, PaintTargetScope},
    stroke_controller::StrokeSampleScratch,
    tool_reducer,
};

pub(crate) fn view_pointer_into_with_scratch(
    state: &mut AppState,
    event: ViewPointerEvent,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    match event {
        ViewPointerEvent::Cancel { reason } => tool_reducer::tool_input_into_with_scratch(
            state,
            ToolInputEvent::Cancel { reason },
            scratch,
        ),
        ViewPointerEvent::Viewport3d { meta, view } => handle_viewport_pointer(
            state,
            meta.phase,
            meta.position_px,
            meta.pressure,
            meta.time_s,
            meta.modifiers,
            view,
            scratch,
        ),
        ViewPointerEvent::Uv { meta, view } => handle_uv_pointer(
            state,
            meta.phase,
            meta.position_px,
            view,
            meta.pressure,
            meta.time_s,
            meta.modifiers,
            scratch,
        ),
    }
}

#[derive(Debug, Clone)]
pub struct ViewportHoverAnalysis {
    pub surface_hit: Option<SurfaceHit>,
    pub paint_edit: PaintEditDecision<()>,
    pub brush_overlay_request: Option<BrushOverlayRequest>,
}

pub fn analyze_viewport_hover(
    state: &AppState,
    position_px: Vec2,
    pressure: f32,
    view: ViewportInputContext,
    scratch: &mut RaycastScratch,
) -> ViewportHoverAnalysis {
    let surface_hit = state.document().and_then(|document| {
        let (ray_origin, ray_dir) =
            ray_from_viewport_px(position_px, view.size, view.inv_view_proj)?;
        document.raycast_visible_with_scratch(
            ray_origin,
            ray_dir,
            state.viewport_scene_visibility(),
            scratch,
        )
    });
    let paint_edit = viewport_paint_edit_decision(state, surface_hit);
    let brush_overlay_request = match (paint_edit, surface_hit) {
        (PaintEditDecision::Allowed(()), Some(hit)) if state.is_tool_idle() => state
            .effective_tool_definition()
            .filter(|tool| tool.is_stroke() && tool.supports_space(StrokeSpace::Surface))
            .and_then(|_| {
                let document = state.document()?;
                let tool_preset = state.effective_tool_preset()?;
                let corrected_pressure = tool_preset.corrected_pressure(pressure);
                let stroke_op = tool_preset
                    .stroke_op
                    .resolve(document.mesh.scene_diagonal());
                let dab = SurfaceDab::with_scales(
                    position_px,
                    hit.world_pos,
                    hit.world_normal,
                    hit.material_index.as_usize(),
                    corrected_pressure,
                    tool_preset.stroke_op.radius_scale(corrected_pressure),
                );
                let view_direction = match state.camera().projection {
                    CameraProjection::Perspective => {
                        (Vec3::from_array(view.camera_world) - dab.world_pos).normalize_or_zero()
                    }
                    CameraProjection::Orthographic => -state.camera().forward(),
                };
                BrushOverlayRequest::Surface(SurfaceBrushOverlayRequest {
                    viewport_size: view.size,
                    viewport_view_proj_gl: view.view_proj,
                    view_direction_world: view_direction.to_array(),
                    stroke_op,
                    dab,
                })
                .into()
            }),
        _ => None,
    };
    ViewportHoverAnalysis {
        surface_hit,
        paint_edit,
        brush_overlay_request,
    }
}

fn viewport_paint_edit_decision(
    state: &AppState,
    surface_hit: Option<SurfaceHit>,
) -> PaintEditDecision<()> {
    let Some(tool) = state.effective_tool_definition() else {
        return PaintEditDecision::Unavailable;
    };
    match state.paint_edit_permission(None) {
        PaintEditDecision::Allowed(()) => {}
        PaintEditDecision::Blocked(reason) => return PaintEditDecision::Blocked(reason),
        PaintEditDecision::Unavailable => return PaintEditDecision::Unavailable,
    }
    match &tool.behavior {
        ToolBehavior::Stroke { .. } | ToolBehavior::Fill { .. } => surface_hit
            .map_or(PaintEditDecision::Unavailable, |hit| {
                state.paint_edit_permission(Some(hit.material_index.as_usize()))
            }),
        ToolBehavior::Decal {
            kind: DecalKind::Surface,
        } if state.active_decal_transform().is_none() => surface_hit
            .map_or(PaintEditDecision::Unavailable, |hit| {
                state.paint_edit_permission(Some(hit.material_index.as_usize()))
            }),
        ToolBehavior::Shape { .. } => match state
            .document
            .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
        {
            PaintEditDecision::Allowed(_) => PaintEditDecision::Allowed(()),
            PaintEditDecision::Blocked(reason) => PaintEditDecision::Blocked(reason),
            PaintEditDecision::Unavailable => PaintEditDecision::Unavailable,
        },
        ToolBehavior::Decal { .. } | ToolBehavior::Selection { .. } | ToolBehavior::Transform => {
            state.paint_edit_permission(None)
        }
        ToolBehavior::ColorPicker => PaintEditDecision::Allowed(()),
        ToolBehavior::NoOp => PaintEditDecision::Unavailable,
    }
}

pub fn plan_decal_overlay_request(state: &AppState) -> Option<DecalOverlayRequest> {
    let transform = state.active_decal_transform()?;
    let image = state.decal_image()?.clone();
    let scene_visibility = state.active_decal_scene_visibility()?.clone();
    let opacity = state.decal_options().opacity.clamp(0.0, 1.0);
    (opacity > 0.0).then_some(DecalOverlayRequest {
        image,
        projection: DecalProjection::Surface(transform),
        opacity,
        scene_visibility,
    })
}

pub fn plan_decal_overlay_request_for_viewport(
    state: &AppState,
    viewport_view_proj: Mat4,
    viewport_size: [u32; 2],
) -> Option<DecalOverlayRequest> {
    if state.active_decal_transform().is_some() {
        return plan_decal_overlay_request(state);
    }
    let transform = state
        .active_view_projection_decal_transform()?
        .resized(viewport_size)?;
    let image = state.decal_image()?.clone();
    let scene_visibility = state.active_decal_scene_visibility()?.clone();
    let opacity = state.decal_options().opacity.clamp(0.0, 1.0);
    let projector_view_proj = transform.projector_view_projection(viewport_view_proj)?;
    let depth_size = [
        transform.size_px.x.round().max(1.0) as u32,
        transform.size_px.y.round().max(1.0) as u32,
    ];
    (opacity > 0.0).then_some(DecalOverlayRequest {
        image,
        projection: DecalProjection::ViewProjection {
            projector_view_proj,
            viewport_view_proj,
            depth_size,
        },
        opacity,
        scene_visibility,
    })
}

pub fn plan_surface_mirror_plane_overlay_request(
    state: &AppState,
    view_proj: Mat4,
) -> Option<MirrorPlaneOverlayRequest> {
    let options = state.surface_mirror_options();
    if !options.x_enabled || !options.show_x_plane {
        return None;
    }
    let tool = state.effective_tool_definition()?;
    if !tool.supports_surface_mirror() {
        return None;
    }

    let document = state.document()?;
    let (bounds_min, bounds_max) = document.mesh.scene_bounds()?;
    let scene_diagonal = document.mesh.scene_diagonal();
    if !scene_diagonal.is_finite() || scene_diagonal <= 0.0 {
        return None;
    }

    let plane_x = state
        .active_surface_mirror_plane_x()
        .unwrap_or(options.x_plane);
    let center = (bounds_min + bounds_max) * 0.5;
    let size = bounds_max - bounds_min;
    let padding = scene_diagonal * 0.05;
    let minimum_half_extent = scene_diagonal * 0.25;
    let half_y = (size.y * 0.5 + padding).max(minimum_half_extent);
    let half_z = (size.z * 0.5 + padding).max(minimum_half_extent);
    let request = MirrorPlaneOverlayRequest {
        plane_x,
        center_yz: [center.y, center.z],
        half_extent_yz: [half_y, half_z],
    };
    if !mirror_plane_request_is_finite(request)
        || !mirror_plane_clip_positions_are_finite(view_proj, request)
    {
        return None;
    }
    Some(request)
}

fn mirror_plane_request_is_finite(request: MirrorPlaneOverlayRequest) -> bool {
    request.plane_x.is_finite()
        && request.center_yz.into_iter().all(f32::is_finite)
        && request.half_extent_yz.into_iter().all(f32::is_finite)
        && request
            .half_extent_yz
            .into_iter()
            .all(|extent| extent > 0.0)
}

fn mirror_plane_clip_positions_are_finite(
    view_proj: Mat4,
    request: MirrorPlaneOverlayRequest,
) -> bool {
    for y_sign in [-1.0, 1.0] {
        for z_sign in [-1.0, 1.0] {
            let world = Vec3::new(
                request.plane_x,
                request.center_yz[0] + request.half_extent_yz[0] * y_sign,
                request.center_yz[1] + request.half_extent_yz[1] * z_sign,
            );
            if !(view_proj * world.extend(1.0)).is_finite() {
                return false;
            }
        }
    }
    true
}

pub fn plan_uv_brush_overlay_request(
    state: &AppState,
    position_px: Vec2,
    pressure: f32,
    view: UvViewInputContext,
) -> Option<UvBrushOverlayRequest> {
    if state
        .document
        .resolve_paint_edit_target(PaintTargetScope::FocusedMaterial)
        .into_allowed()
        .is_none()
        || !state.is_tool_idle()
    {
        return None;
    }
    let tool = state.effective_tool_definition()?;
    if !tool.is_stroke() || !tool.supports_space(StrokeSpace::Uv) {
        return None;
    }
    if !view
        .transform
        .contains_view_px(position_px, view.size, view.canvas_size)
    {
        return None;
    }
    let document = state.document()?;
    let tool_preset = state.effective_tool_preset()?;
    let corrected_pressure = tool_preset.corrected_pressure(pressure);
    let radius_world = tool_preset
        .stroke_op
        .radius_world(document.mesh.scene_diagonal());
    let radius_px = document
        .mesh
        .world_to_uv_texel_radius(radius_world, view.canvas_size)
        * view.transform.zoom
        * tool_preset.stroke_op.radius_scale(corrected_pressure);
    Some(UvBrushOverlayRequest {
        center_px: [position_px.x, position_px.y],
        view_size: view.size,
        radius_px: radius_px.max(1.0),
    })
}

fn handle_viewport_pointer(
    state: &mut AppState,
    phase: ViewPointerPhase,
    position_px: Vec2,
    pressure: f32,
    time_s: f64,
    modifiers: InputModifiers,
    view: ViewportInputContext,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let Some(tool) = state.effective_tool_definition() else {
        return Ok(ReducerOutput::default());
    };
    if !tool.supports_space(StrokeSpace::Surface) {
        return Ok(ReducerOutput::default());
    }
    let start_analysis =
        matches!(phase, ViewPointerPhase::Click | ViewPointerPhase::Down).then(|| {
            analyze_viewport_hover(
                state,
                position_px,
                pressure,
                view,
                scratch.raycast_scratch_mut(),
            )
        });
    if active_tool_edits_paint_target_in_space(state, StrokeSpace::Surface)
        && start_analysis
            .as_ref()
            .is_some_and(|hover| !hover.paint_edit.is_allowed())
    {
        return Ok(ReducerOutput::default());
    }

    match phase {
        ViewPointerPhase::Click => {
            let Some(sample) = (PointerSampleFactory { state })
                .pointer_sample_for_3d_view_with_hit(
                    position_px,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                    start_analysis.and_then(|hover| hover.surface_hit),
                )
            else {
                return Ok(ReducerOutput::default());
            };
            dispatch_tool_sequence(
                state,
                [
                    ToolInputEvent::PointerDown(sample.clone()),
                    ToolInputEvent::PointerUp(sample),
                ],
                scratch,
            )
        }
        ViewPointerPhase::Down => {
            let Some(sample) = (PointerSampleFactory { state })
                .pointer_sample_for_3d_view_with_hit(
                    position_px,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                    start_analysis.and_then(|hover| hover.surface_hit),
                )
            else {
                return Ok(ReducerOutput::default());
            };
            tool_reducer::tool_input_into_with_scratch(
                state,
                ToolInputEvent::PointerDown(sample),
                scratch,
            )
        }
        ViewPointerPhase::Move => {
            if state
                .stroking_space()
                .is_some_and(|space| space != StrokeSpace::Surface)
            {
                return Ok(ReducerOutput::default());
            }
            let sample = if state.tool.fill_session().is_some() {
                PointerSample::surface_without_hit_with_pressure_at(
                    position_px,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                )
            } else {
                let surface_hit = (PointerSampleFactory { state }).surface_hit(
                    position_px,
                    view,
                    scratch.raycast_scratch_mut(),
                );
                let Some(sample) = (PointerSampleFactory { state })
                    .pointer_sample_for_3d_view_with_hit(
                        position_px,
                        pressure,
                        time_s,
                        modifiers,
                        view,
                        surface_hit,
                    )
                else {
                    return Ok(ReducerOutput::default());
                };
                sample
            };
            tool_reducer::tool_input_into_with_scratch(
                state,
                ToolInputEvent::PointerMove(sample),
                scratch,
            )
        }
        ViewPointerPhase::Up => {
            let Some(stroking_space) = state.stroking_space() else {
                return Ok(ReducerOutput::default());
            };
            let sample = match stroking_space {
                StrokeSpace::Surface => PointerSample::surface_without_hit_with_pressure_at(
                    position_px,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                ),
                StrokeSpace::Uv => return Ok(ReducerOutput::default()),
            };
            tool_reducer::tool_input_into_with_scratch(
                state,
                ToolInputEvent::PointerUp(sample),
                scratch,
            )
        }
    }
}

fn handle_uv_pointer(
    state: &mut AppState,
    phase: ViewPointerPhase,
    position_px: Vec2,
    view: UvViewInputContext,
    pressure: f32,
    time_s: f64,
    modifiers: InputModifiers,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let Some(tool) = state.effective_tool_definition() else {
        return Ok(ReducerOutput::default());
    };
    if !tool.supports_space(StrokeSpace::Uv) {
        return Ok(ReducerOutput::default());
    }
    if should_block_paint_edit_start(state, StrokeSpace::Uv, phase) {
        return Ok(ReducerOutput::default());
    }

    let uv = view
        .transform
        .view_px_to_uv(position_px, view.size, view.canvas_size);
    let inside_canvas = (0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y);
    if !inside_canvas
        && matches!(phase, ViewPointerPhase::Click | ViewPointerPhase::Down)
        && !active_transform_accepts_workspace_input(state)
    {
        return Ok(ReducerOutput::default());
    }
    let sample =
        PointerSample::uv_with_context_at(uv, position_px, view, pressure, time_s, modifiers);
    match phase {
        ViewPointerPhase::Click => dispatch_tool_sequence(
            state,
            [
                ToolInputEvent::PointerDown(sample.clone()),
                ToolInputEvent::PointerUp(sample),
            ],
            scratch,
        ),
        ViewPointerPhase::Down => tool_reducer::tool_input_into_with_scratch(
            state,
            ToolInputEvent::PointerDown(sample),
            scratch,
        ),
        ViewPointerPhase::Move => tool_reducer::tool_input_into_with_scratch(
            state,
            ToolInputEvent::PointerMove(sample),
            scratch,
        ),
        ViewPointerPhase::Up => tool_reducer::tool_input_into_with_scratch(
            state,
            ToolInputEvent::PointerUp(sample),
            scratch,
        ),
    }
}

fn dispatch_tool_sequence<const N: usize>(
    state: &mut AppState,
    events: [ToolInputEvent; N],
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let mut combined = ReducerOutput::default();
    for event in events {
        let outcome = tool_reducer::tool_input_into_with_scratch(state, event, scratch)?;
        combined.append_outcome(outcome);
    }
    Ok(combined)
}

fn active_transform_accepts_workspace_input(state: &AppState) -> bool {
    state.has_active_transform_session()
        || state
            .effective_tool_definition()
            .is_some_and(|tool| matches!(&tool.behavior, ToolBehavior::Transform))
}

fn active_tool_edits_paint_target_in_space(state: &AppState, space: StrokeSpace) -> bool {
    state
        .effective_tool_definition()
        .is_some_and(|tool| tool.edits_paint_target_in_space(space))
}

fn should_block_paint_edit_start(
    state: &AppState,
    space: StrokeSpace,
    phase: ViewPointerPhase,
) -> bool {
    if !matches!(phase, ViewPointerPhase::Click | ViewPointerPhase::Down) {
        return false;
    }
    if space == StrokeSpace::Uv
        && state
            .effective_tool_definition()
            .is_some_and(|tool| matches!(&tool.behavior, ToolBehavior::Transform))
        && state.active_layer_target() == crate::core::document::ActiveLayerTarget::EmbeddedImage
        && state.document().is_some_and(|document| {
            !document
                .layer_tree
                .is_effectively_locked(state.active_layer_id())
        })
    {
        return false;
    }
    let anchor_material_index = (space == StrokeSpace::Uv).then(|| state.focused_material_index());
    state
        .paint_edit_permission(anchor_material_index)
        .blocked_reason()
        .is_some()
        && state
            .effective_tool_definition()
            .is_some_and(|tool| tool.edits_paint_target_in_space(space))
}

struct PointerSampleFactory<'a> {
    state: &'a AppState,
}

impl<'a> PointerSampleFactory<'a> {
    fn surface_hit(
        &self,
        viewport_pos: Vec2,
        view: ViewportInputContext,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        let document = self.state.document()?;
        let (ray_origin, ray_dir) =
            ray_from_viewport_px(viewport_pos, view.size, view.inv_view_proj)?;
        document.raycast_visible_with_scratch(
            ray_origin,
            ray_dir,
            self.state.viewport_scene_visibility(),
            scratch,
        )
    }

    fn pointer_sample_for_3d_view_with_hit(
        &self,
        viewport_pos: Vec2,
        pressure: f32,
        time_s: f64,
        modifiers: InputModifiers,
        view: ViewportInputContext,
        surface_hit: Option<SurfaceHit>,
    ) -> Option<PointerSample> {
        let tool = self.state.effective_tool_definition()?;
        let requirement = if self.state.has_active_decal_session()
            && tool.surface_hit_requirement() == SurfaceHitRequirement::Required
        {
            SurfaceHitRequirement::Optional
        } else {
            tool.surface_hit_requirement()
        };
        match requirement {
            SurfaceHitRequirement::Required => surface_hit.map(|hit| {
                PointerSample::surface_with_pressure_at(
                    viewport_pos,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                    hit,
                )
            }),
            SurfaceHitRequirement::Optional => Some(match surface_hit {
                Some(hit) => PointerSample::surface_with_pressure_at(
                    viewport_pos,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                    hit,
                ),
                None => PointerSample::surface_without_hit_with_pressure_at(
                    viewport_pos,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                ),
            }),
            SurfaceHitRequirement::None => {
                Some(PointerSample::surface_without_hit_with_pressure_at(
                    viewport_pos,
                    pressure,
                    time_s,
                    modifiers,
                    view,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::{Mat4, Vec2, Vec3};

    use crate::{
        application::{
            Command, InputModifiers, PaintEditDecision, UvViewInputContext, ViewPointerPhase,
            ViewportInputContext, reduce,
            state::{
                AppState, DecalSession, EditorDocumentState, PaintTargetScope, ToolSession,
                ViewProjectionDecalSession,
            },
        },
        core::{
            decal::{
                DecalImageAsset, DecalImageId, DecalProjection, DecalTransform,
                ViewProjectionDecalTransform,
            },
            document::{
                Document, MaterialSpec, MeshData, MeshId, MeshObject, RaycastScratch, SubMesh,
                SurfaceHit,
            },
            surface::LayerMaterialMask,
            tool::ToolId,
            uv_view::UvViewTransform,
        },
    };

    use super::{
        PointerSampleFactory, active_transform_accepts_workspace_input, analyze_viewport_hover,
        handle_viewport_pointer, plan_decal_overlay_request,
        plan_surface_mirror_plane_overlay_request, plan_uv_brush_overlay_request,
    };
    use crate::application::stroke_controller::StrokeSampleScratch;

    #[test]
    fn transform_accepts_pointer_start_in_workspace() {
        let mut state = AppState::default();
        assert!(!active_transform_accepts_workspace_input(&state));

        state.set_active_tool(ToolId::Transform);
        assert!(active_transform_accepts_workspace_input(&state));

        let mut modal_state = AppState::default();
        modal_state.tool.begin_transform_mode();
        assert!(active_transform_accepts_workspace_input(&modal_state));
    }

    #[test]
    fn empty_group_no_op_has_no_overlay_or_paint_input() {
        let mut state = state_with_mesh();
        let preset_id = "builtin.brush.paint.round_soft";
        let brush = state.tool_id_by_config_id(preset_id).expect("brush tool");
        state.set_active_tool(brush);
        for preset_id in [
            "builtin.brush.paint.directional_flat",
            "builtin.brush.paint.round_soft",
        ] {
            let _ = reduce(
                &mut state,
                Command::DeleteBrushPreset {
                    preset_id: preset_id.to_owned(),
                },
            );
        }

        let view = test_view();
        let hover = analyze_viewport_hover(
            &state,
            Vec2::new(50.0, 50.0),
            1.0,
            view,
            &mut RaycastScratch::default(),
        );
        assert_eq!(hover.paint_edit, PaintEditDecision::Unavailable);
        assert!(hover.brush_overlay_request.is_none());
        assert!(
            plan_uv_brush_overlay_request(
                &state,
                Vec2::new(50.0, 50.0),
                1.0,
                UvViewInputContext {
                    size: [100, 100],
                    canvas_size: [100, 100],
                    transform: UvViewTransform::default(),
                },
            )
            .is_none()
        );

        let output = handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Down,
            Vec2::new(50.0, 50.0),
            1.0,
            0.0,
            InputModifiers::default(),
            view,
            &mut StrokeSampleScratch::default(),
        )
        .unwrap();
        assert!(!output.has_renderer_work());
        assert!(state.is_tool_idle());
    }

    #[test]
    fn decal_overlay_requires_an_active_session_and_positive_opacity() {
        let mut state = state_with_mesh();
        assert!(plan_decal_overlay_request(&state).is_none());

        let transform = DecalTransform {
            center_world: Vec3::ZERO,
            axis_x_world: Vec3::X,
            axis_y_world: Vec3::Y,
            normal_world: Vec3::Z,
            size_world: Vec2::new(2.0, 1.0),
            projection_depth_world: 4.0,
        };
        state.tool.set_session(ToolSession::Decal(DecalSession {
            transform,
            source_hit: SurfaceHit {
                world_pos: Vec3::ZERO,
                world_normal: Vec3::Z,
                uv: Vec2::splat(0.5),
                uv_edge_distance: 1.0,
                uv_paint_boundary_distance: 1.0,
                triangle_index: 0,
                material_index: 0.into(),
                mesh_id: MeshId(0),
                t: 1.0,
            },
            gesture: None,
            scene_visibility: Default::default(),
        }));
        assert!(plan_decal_overlay_request(&state).is_none());

        let image = Arc::new(DecalImageAsset {
            id: DecalImageId(7),
            file_name: "decal.png".to_owned(),
            size: [1, 1],
            rgba8: Arc::from([255, 255, 255, 255]),
        });
        state.tool.decal_image = Some(image.clone());

        let request = plan_decal_overlay_request(&state).expect("active decal should render");
        assert_eq!(request.image.id, image.id);
        assert_eq!(request.projection, DecalProjection::Surface(transform));
        assert_eq!(request.opacity, 1.0);

        state.tool.decal_options.opacity = 0.0;
        assert!(plan_decal_overlay_request(&state).is_none());
    }

    #[test]
    fn viewport_hover_blocks_excluded_material_and_keeps_allowed_mesh_surfaces() {
        let mut state = state_with_mesh_and_materials(vec![
            MaterialSpec::new("Excluded", [8, 8]),
            MaterialSpec::new("Allowed", [8, 8]),
        ]);
        let document = state.document().expect("test document");
        let layer_id = document
            .layer_tree
            .default_raster_layer()
            .expect("default raster layer");
        let allowed_material = document.material_id(1.into()).expect("allowed material id");
        state
            .document_mut()
            .expect("test document")
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([allowed_material].into_iter().collect()),
            );
        let view = ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, -1.0],
        };

        let hover = analyze_viewport_hover(
            &state,
            Vec2::new(50.0, 50.0),
            1.0,
            view,
            &mut RaycastScratch::default(),
        );
        assert_eq!(
            hover.paint_edit,
            PaintEditDecision::Blocked(crate::application::PaintEditBlockReason::MaterialExcluded)
        );
        assert!(hover.brush_overlay_request.is_none());
        assert_eq!(state.paint_edit_permission(None).blocked_reason(), None,);
        assert_eq!(
            state
                .document
                .resolve_paint_edit_target(PaintTargetScope::AllMaterials)
                .into_allowed()
                .expect("the allowed material remains editable")
                .surfaces_vec()
                .iter()
                .map(|surface| surface.material_index().as_usize())
                .collect::<Vec<_>>(),
            vec![1],
        );
    }

    #[test]
    fn excluded_material_blocks_pointer_down_but_does_not_cancel_an_active_stroke() {
        let mut state = state_with_mesh_and_materials(vec![
            MaterialSpec::new("Hit", [8, 8]),
            MaterialSpec::new("Other", [8, 8]),
        ]);
        let layer_id = state
            .document()
            .and_then(|document| document.layer_tree.default_raster_layer())
            .expect("default raster layer");
        let hit_material = state.document().unwrap().material_id(0.into()).unwrap();
        let other_material = state.document().unwrap().material_id(1.into()).unwrap();
        let view = ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, -1.0],
        };
        let position = Vec2::new(50.0, 50.0);
        let mut scratch = StrokeSampleScratch::default();

        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([other_material].into_iter().collect()),
            );
        handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Down,
            position,
            1.0,
            0.0,
            InputModifiers::default(),
            view,
            &mut scratch,
        )
        .unwrap();
        assert!(state.is_tool_idle());

        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([hit_material].into_iter().collect()),
            );
        handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Down,
            position,
            1.0,
            0.1,
            InputModifiers::default(),
            view,
            &mut scratch,
        )
        .unwrap();
        assert!(state.is_tool_pointer_gesture_active());

        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([other_material].into_iter().collect()),
            );
        handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Move,
            position,
            1.0,
            0.2,
            InputModifiers::default(),
            view,
            &mut scratch,
        )
        .unwrap();
        assert!(state.is_tool_pointer_gesture_active());
    }

    #[test]
    fn mesh_fill_requires_an_allowed_anchor_material() {
        let mut state = state_with_mesh_and_materials(vec![
            MaterialSpec::new("Hit", [8, 8]),
            MaterialSpec::new("Other", [8, 8]),
        ]);
        state.set_active_tool(ToolId::FillMesh);
        let layer_id = state
            .document()
            .and_then(|document| document.layer_tree.default_raster_layer())
            .unwrap();
        let hit_material = state.document().unwrap().material_id(0.into()).unwrap();
        let other_material = state.document().unwrap().material_id(1.into()).unwrap();
        let view = ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, -1.0],
        };
        let mut scratch = StrokeSampleScratch::default();

        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([other_material].into_iter().collect()),
            );
        let blocked = handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Click,
            Vec2::new(50.0, 50.0),
            1.0,
            0.0,
            InputModifiers::default(),
            view,
            &mut scratch,
        )
        .unwrap();
        assert!(!blocked.has_renderer_work());

        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([hit_material].into_iter().collect()),
            );
        let allowed = handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Click,
            Vec2::new(50.0, 50.0),
            1.0,
            0.1,
            InputModifiers::default(),
            view,
            &mut scratch,
        )
        .unwrap();
        assert!(allowed.has_renderer_work());
    }

    #[test]
    fn surface_shape_does_not_use_the_hovered_material_as_an_anchor() {
        let mut state = state_with_mesh_and_materials(vec![
            MaterialSpec::new("Hit", [8, 8]),
            MaterialSpec::new("Allowed", [8, 8]),
        ]);
        state.set_active_tool(ToolId::RectanglePaint);
        exclude_hit_material(&mut state);
        let view = test_view();
        let position = Vec2::new(50.0, 50.0);

        let hover =
            analyze_viewport_hover(&state, position, 1.0, view, &mut RaycastScratch::default());
        assert_eq!(hover.paint_edit, PaintEditDecision::Allowed(()));

        handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Down,
            position,
            1.0,
            0.0,
            InputModifiers::default(),
            view,
            &mut StrokeSampleScratch::default(),
        )
        .unwrap();
        assert!(state.is_tool_pointer_gesture_active());
    }

    #[test]
    fn surface_shapes_are_blocked_when_every_material_is_excluded() {
        for tool_id in [
            ToolId::RectanglePaint,
            ToolId::RectangleErase,
            ToolId::LassoPaint,
            ToolId::LassoErase,
        ] {
            let mut state = state_with_mesh();
            state.set_active_tool(tool_id);
            exclude_all_materials(&mut state);
            let view = test_view();
            let position = Vec2::new(50.0, 50.0);

            let hover =
                analyze_viewport_hover(&state, position, 1.0, view, &mut RaycastScratch::default());
            assert_eq!(
                hover.paint_edit,
                PaintEditDecision::Blocked(
                    crate::application::PaintEditBlockReason::MaterialExcluded
                ),
                "{tool_id:?} should expose the same block used by pointer start"
            );

            handle_viewport_pointer(
                &mut state,
                ViewPointerPhase::Down,
                position,
                1.0,
                0.0,
                InputModifiers::default(),
                view,
                &mut StrokeSampleScratch::default(),
            )
            .unwrap();
            assert!(
                state.is_tool_idle(),
                "{tool_id:?} should not start without an editable surface"
            );
        }
    }

    #[test]
    fn new_surface_decal_uses_material_anchor_but_active_handle_does_not() {
        let mut state = state_with_mesh_and_materials(vec![
            MaterialSpec::new("Hit", [8, 8]),
            MaterialSpec::new("Allowed", [8, 8]),
        ]);
        state.set_active_tool(ToolId::SurfaceDecal);
        state.tool.decal_image = Some(test_decal_image());
        exclude_hit_material(&mut state);
        let view = test_view();
        let position = Vec2::new(50.0, 50.0);
        let initial_hover =
            analyze_viewport_hover(&state, position, 1.0, view, &mut RaycastScratch::default());
        assert!(matches!(
            initial_hover.paint_edit,
            PaintEditDecision::Blocked(crate::application::PaintEditBlockReason::MaterialExcluded)
        ));

        let hit = initial_hover.surface_hit.unwrap();
        state.tool.set_session(ToolSession::Decal(DecalSession {
            transform: DecalTransform {
                center_world: Vec3::ZERO,
                axis_x_world: Vec3::X,
                axis_y_world: Vec3::Y,
                normal_world: Vec3::Z,
                size_world: Vec2::splat(0.5),
                projection_depth_world: 2.0,
            },
            source_hit: SurfaceHit {
                world_pos: Vec3::ZERO,
                ..hit
            },
            gesture: None,
            scene_visibility: Default::default(),
        }));

        let active_hover =
            analyze_viewport_hover(&state, position, 1.0, view, &mut RaycastScratch::default());
        assert_eq!(active_hover.paint_edit, PaintEditDecision::Allowed(()));
        handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Down,
            position,
            1.0,
            0.0,
            InputModifiers::default(),
            view,
            &mut StrokeSampleScratch::default(),
        )
        .unwrap();
        assert!(
            state
                .tool
                .decal_session()
                .is_some_and(|session| session.gesture.is_some())
        );
    }

    #[test]
    fn view_projection_decal_handle_ignores_material_under_screen_handle() {
        let mut state = state_with_mesh_and_materials(vec![
            MaterialSpec::new("Hit", [8, 8]),
            MaterialSpec::new("Allowed", [8, 8]),
        ]);
        state.set_active_tool(ToolId::ViewProjectionDecal);
        exclude_hit_material(&mut state);
        let view = test_view();
        let position = Vec2::new(50.0, 50.0);
        state.tool.set_session(ToolSession::ViewProjectionDecal(
            ViewProjectionDecalSession {
                transform: ViewProjectionDecalTransform::initial([10, 10], view.size).unwrap(),
                gesture: None,
                scene_visibility: Default::default(),
            },
        ));

        let hover =
            analyze_viewport_hover(&state, position, 1.0, view, &mut RaycastScratch::default());
        assert_eq!(hover.paint_edit, PaintEditDecision::Allowed(()));
        handle_viewport_pointer(
            &mut state,
            ViewPointerPhase::Down,
            position,
            1.0,
            0.0,
            InputModifiers::default(),
            view,
            &mut StrokeSampleScratch::default(),
        )
        .unwrap();
        assert!(
            state
                .tool
                .view_projection_decal_session()
                .is_some_and(|session| session.gesture.is_some())
        );
    }

    #[test]
    fn viewport_pointer_sampling_ignores_hidden_geometry() {
        let mut state = state_with_mesh();
        let view = ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, -1.0],
        };
        let position = Vec2::new(50.0, 50.0);

        assert!(
            (PointerSampleFactory { state: &state })
                .surface_hit(position, view, &mut RaycastScratch::default())
                .is_some()
        );

        state
            .view
            .scene_visibility
            .set_mesh_visible(MeshId(0), false);

        assert!(
            (PointerSampleFactory { state: &state })
                .surface_hit(position, view, &mut RaycastScratch::default())
                .is_none()
        );
        assert_eq!(
            analyze_viewport_hover(&state, position, 1.0, view, &mut RaycastScratch::default(),)
                .paint_edit,
            PaintEditDecision::Unavailable
        );
    }

    #[test]
    fn mirror_plane_overlay_requires_both_mirror_and_visibility() {
        let mut state = state_with_mesh();
        assert!(plan_surface_mirror_plane_overlay_request(&state, Mat4::IDENTITY).is_none());

        state.view.surface_mirror_x_enabled = true;
        let request = plan_surface_mirror_plane_overlay_request(&state, Mat4::IDENTITY)
            .expect("enabled surface mirror should produce an overlay request");
        assert_eq!(request.plane_x, 0.0);

        state.view.surface_mirror_x_plane_visible = false;
        assert!(plan_surface_mirror_plane_overlay_request(&state, Mat4::IDENTITY).is_none());
    }

    #[test]
    fn mirror_plane_overlay_supports_surface_shape_paint_and_erase_tools() {
        let mut state = state_with_mesh();
        state.view.surface_mirror_x_enabled = true;

        for tool_id in [
            ToolId::RectanglePaint,
            ToolId::RectangleErase,
            ToolId::LassoPaint,
            ToolId::LassoErase,
        ] {
            state.set_active_tool(tool_id);
            assert!(
                plan_surface_mirror_plane_overlay_request(&state, Mat4::IDENTITY).is_some(),
                "{tool_id:?} should display the surface mirror plane"
            );
        }

        for tool_id in [ToolId::RectangleSelection, ToolId::LassoSelection] {
            state.set_active_tool(tool_id);
            assert!(
                plan_surface_mirror_plane_overlay_request(&state, Mat4::IDENTITY).is_none(),
                "{tool_id:?} should not display the surface mirror plane"
            );
        }
    }

    #[test]
    fn mirror_plane_overlay_uses_scene_bounds_and_configured_plane() {
        let mut state = state_with_mesh();
        state.view.surface_mirror_x_enabled = true;
        state.view.surface_mirror_x_plane = 1.25;

        let request = plan_surface_mirror_plane_overlay_request(&state, Mat4::IDENTITY)
            .expect("valid mirror settings should produce an overlay request");
        let diagonal = Vec3::new(3.0, 4.0, 12.0).length();
        assert_eq!(request.plane_x, 1.25);
        assert_eq!(request.center_yz, [0.0, 3.0]);
        assert!((request.half_extent_yz[0] - diagonal * 0.25).abs() < 1e-6);
        assert!((request.half_extent_yz[1] - (6.0 + diagonal * 0.05)).abs() < 1e-6);
    }

    fn test_view() -> ViewportInputContext {
        ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, -1.0],
        }
    }

    fn test_decal_image() -> Arc<DecalImageAsset> {
        Arc::new(DecalImageAsset {
            id: DecalImageId(99),
            file_name: "decal.png".to_owned(),
            size: [1, 1],
            rgba8: Arc::from([255, 255, 255, 255]),
        })
    }

    fn exclude_hit_material(state: &mut AppState) {
        let document = state.document().unwrap();
        let layer_id = document.layer_tree.default_raster_layer().unwrap();
        let allowed_material = document.material_id(1.into()).unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(
                layer_id,
                LayerMaterialMask::Specified([allowed_material].into_iter().collect()),
            );
    }

    fn exclude_all_materials(state: &mut AppState) {
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        state
            .document_mut()
            .unwrap()
            .layer_tree
            .set_layer_material_mask(layer_id, LayerMaterialMask::Specified(Default::default()));
    }

    fn state_with_mesh() -> AppState {
        state_with_mesh_and_materials(vec![MaterialSpec::new("Material", [8, 8])])
    }

    fn state_with_mesh_and_materials(materials: Vec<MaterialSpec>) -> AppState {
        let positions = vec![
            Vec3::new(-1.0, -2.0, -3.0),
            Vec3::new(2.0, -2.0, -3.0),
            Vec3::new(-1.0, 2.0, 9.0),
        ];
        let mesh = MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO; positions.len()],
            vec![Vec3::Z; positions.len()],
            vec![[0, 1, 2]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0)],
        )
        .unwrap();
        let document = Document::new(mesh, materials);
        let editor = EditorDocumentState::from_document(&document);
        let mut state = AppState::default();
        state.document.document = Some(document);
        state.document.editor = editor;
        state
    }
}

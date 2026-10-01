use crate::application::{
    ReducerOutput,
    command::{Command, CommandBlocker, CommandDomain},
};
use anyhow::Result;

use super::{
    camera_controller, decal_controller, document_reducer, history_reducer, image_cut_controller,
    image_import_reducer, layer_mask_paste_controller, layer_reducer, selection_controller,
    state::AppState, stroke_controller::StrokeSampleScratch, surface_filter_controller,
    tool_reducer, view_input_planner, view_reducer,
};

pub(crate) fn reduce_to_outcome(
    state: &mut AppState,
    command: Command,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    if state.has_adjustment_filter_session()
        && !command
            .policy()
            .allowed_during(CommandBlocker::AdjustmentFilterSession)
    {
        state.set_status_key("status-filter-preview-blocking-edit");
        return Ok(ReducerOutput::default());
    }

    match command.policy().domain() {
        CommandDomain::Import => reduce_import(state, command),
        CommandDomain::DocumentReset
        | CommandDomain::Document
        | CommandDomain::RendererSettings
        | CommandDomain::Material => reduce_document(state, command),
        CommandDomain::Settings => reduce_settings(state, command),
        CommandDomain::View | CommandDomain::Status => reduce_view(state, command),
        CommandDomain::Tool | CommandDomain::InteractionCleanup => {
            reduce_tool(state, command, scratch)
        }
        CommandDomain::Transform => reduce_transform(state, command),
        CommandDomain::Paint => reduce_paint(state, command),
        CommandDomain::Selection => reduce_selection(state, command),
        CommandDomain::Filter | CommandDomain::AdjustmentSessionControl => {
            reduce_filter(state, command)
        }
        CommandDomain::History => reduce_history(state, command),
        CommandDomain::Layer => reduce_layer(state, command),
    }
}

fn reduce_settings(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    match command {
        Command::SetTheme(theme) => {
            state.settings_mut().appearance.theme = theme;
            Ok(ReducerOutput::default())
        }
        Command::SetLanguage(language) => {
            state.settings_mut().appearance.language = language;
            Ok(ReducerOutput::default())
        }
        Command::SetTabletBackend(backend) => {
            state.settings_mut().input.tablet.backend = backend;
            Ok(ReducerOutput::default())
        }
        Command::SetTabletPressureCurve(curve) => {
            curve
                .validate()
                .map_err(|error| anyhow::anyhow!("invalid tablet pressure curve: {error}"))?;
            state.settings_mut().input.tablet.pressure_curve = curve;
            Ok(ReducerOutput::default())
        }
        _ => unreachable!("command was routed to the wrong domain reducer"),
    }
}

fn reduce_import(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    match command {
        Command::BeginEmbeddedImageImport { image } => {
            return image_import_reducer::begin_embedded(state, image);
        }
        Command::BeginImportedImageTransform { image } => {
            return image_import_reducer::begin(state, image);
        }
        Command::PasteImageAsLayer { image } => {
            return image_import_reducer::paste(state, image);
        }
        _ => unreachable!("command was routed to the wrong domain reducer"),
    }
}

fn reduce_document(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    let produced = match command {
        Command::AssetLoaded { asset, materials } => {
            let output = document_reducer::asset_loaded(state, asset, materials)?;
            view_reducer::reset_uv_view_transform(state.view_mut());
            output
        }
        Command::ProjectLoaded(project) => document_reducer::project_loaded(state, project)?,
        Command::ReloadMesh { request } => {
            let output = document_reducer::reload_mesh(state, request)?;
            view_reducer::reset_uv_view_transform(state.view_mut());
            output
        }
        Command::SetFocusedMaterial(index) => document_reducer::set_focused_material(state, index)?,
        Command::SetMaterialRenderSettings {
            material_index,
            settings,
        } => document_reducer::set_material_render_settings(state, material_index, settings),
        Command::ResizeMaterialTexture {
            material_index,
            texture_size,
        } => document_reducer::resize_material_texture(state, material_index, texture_size)?,
        Command::SetMaterialExportImageFileNames { names } => {
            document_reducer::set_material_export_image_file_names(state, names)?
        }
        Command::SetMaterialExportPsdFileNames { names } => {
            document_reducer::set_material_export_psd_file_names(state, names)?
        }
        Command::ClearDocument => document_reducer::clear_document(state),
        _ => unreachable!("command was routed to the wrong domain reducer"),
    };
    Ok(produced)
}

fn reduce_view(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    let produced = match command {
        Command::SetViewportGizmoVisible(visible) => {
            view_reducer::set_viewport_gizmo_visible(state.view_mut(), visible)
        }
        Command::SetViewportShading(shading) => {
            view_reducer::set_viewport_shading(state.view_mut(), shading)
        }
        Command::SetViewportWireframeVisible(visible) => {
            view_reducer::set_viewport_wireframe_visible(state.view_mut(), visible)
        }
        Command::SetViewportWireframeStyle(style) => {
            view_reducer::set_viewport_wireframe_style(state.view_mut(), style)
        }
        Command::SetViewportBackgroundColor(color) => {
            view_reducer::set_viewport_background_color(state.view_mut(), color)
        }
        Command::SetViewportMeshVisible { mesh_id, visible } => {
            view_reducer::set_viewport_mesh_visible(state, mesh_id, visible)
        }
        Command::SetViewportMaterialVisible {
            material_index,
            visible,
        } => view_reducer::set_viewport_material_visible(state, material_index, visible),
        Command::SetUvWireframeVisible(visible) => {
            view_reducer::set_uv_wireframe_visible(state.view_mut(), visible)
        }
        Command::SetUvWireframeStyle(style) => {
            view_reducer::set_uv_wireframe_style(state.view_mut(), style)
        }
        Command::SetUvViewBackgroundColor(color) => {
            view_reducer::set_uv_view_background_color(state.view_mut(), color)
        }
        Command::SetStatusMessage(status) => {
            state.set_status_message(status);
            ReducerOutput::default()
        }
        Command::SetCamera(camera) => view_reducer::set_camera(state.view_mut(), camera),
        Command::ResetViewportCamera => {
            let camera = camera_controller::reset_camera_for_document(state);
            view_reducer::set_camera(state.view_mut(), camera)
        }
        Command::FrameAllViewport => {
            if let Some(camera) = camera_controller::fit_camera_to_document(state) {
                view_reducer::set_camera(state.view_mut(), camera)
            } else {
                ReducerOutput::default()
            }
        }
        Command::SetUvViewTransform(transform) => {
            view_reducer::set_uv_view_transform(state.view_mut(), transform)
        }
        Command::ResetUvViewTransform => {
            view_reducer::reset_uv_view_transform(state.view_mut());
            ReducerOutput::default()
        }
        _ => unreachable!("command was routed to the wrong domain reducer"),
    };
    Ok(produced)
}

fn reduce_tool(
    state: &mut AppState,
    command: Command,
    scratch: &mut StrokeSampleScratch,
) -> Result<ReducerOutput> {
    let produced = match command {
        Command::SetCurrentColor(color) => view_reducer::set_current_color(state.tool_mut(), color),
        Command::SetDecalImage(image) => tool_reducer::set_decal_image(state, image),
        Command::ClearDecalImage => tool_reducer::clear_decal_image(state),
        Command::UpdateDecalToolOptions(options) => {
            tool_reducer::update_decal_tool_options(state, options)
        }
        Command::BeginViewProjectionDecal { viewport_size } => {
            decal_controller::begin_view_projection_decal(state, viewport_size)
        }
        Command::RequestViewProjectionDecal => {
            decal_controller::request_view_projection_decal(state)
        }
        Command::SyncViewProjectionDecalViewport { viewport_size } => {
            decal_controller::sync_view_projection_decal_viewport(state, viewport_size)
        }
        Command::ApplyActiveDecal => {
            return tool_reducer::apply_active_decal(state);
        }
        Command::CancelActiveDecal => tool_reducer::cancel_active_decal(state),
        Command::SetActiveTool(tool_id) => {
            return tool_reducer::set_active_tool(state, tool_id, scratch);
        }
        Command::BeginMomentaryTool { token, tool_id } => {
            tool_reducer::begin_momentary_tool(state, token, tool_id)
        }
        Command::CompleteMomentaryTool {
            token,
            tool_id,
            select_tool,
        } => tool_reducer::complete_momentary_tool(state, token, tool_id, select_tool),
        Command::BeginTransientToolOverride { token, id } => {
            tool_reducer::begin_transient_tool_override(state.tool_mut(), token, id)
        }
        Command::EndTransientToolOverride { token } => {
            tool_reducer::end_transient_tool_override(state.tool_mut(), token)
        }
        Command::SetActiveToolPresetIndex(index) => {
            tool_reducer::set_active_tool_preset_index(state.tool_mut(), index)
        }
        Command::UpdateActiveToolPreset(tool_preset) => {
            return tool_reducer::update_active_tool_preset(state.tool_mut(), tool_preset);
        }
        Command::UpdateRuntimeStrokePreset {
            preset_index,
            preset,
        } => {
            return tool_reducer::update_runtime_stroke_preset(
                state.tool_mut(),
                preset_index,
                preset,
            );
        }
        Command::SetRuntimeBrushEngine {
            preset_index,
            preset_id,
            engine_id,
        } => {
            return tool_reducer::set_runtime_brush_engine(
                state.tool_mut(),
                preset_index,
                &preset_id,
                &engine_id,
            );
        }
        Command::ResetBrushPreset {
            preset_index,
            preset_id,
        } => {
            return tool_reducer::reset_brush_preset(state.tool_mut(), preset_index, &preset_id);
        }
        Command::CreateBrushPreset {
            group_id,
            source_preset_id,
            new_preset_id,
            preset,
        } => {
            return tool_reducer::create_brush_preset(
                state.tool_mut(),
                &group_id,
                &source_preset_id,
                &new_preset_id,
                preset,
            );
        }
        Command::DeleteBrushPreset { preset_id } => {
            return tool_reducer::delete_brush_preset(state.tool_mut(), &preset_id);
        }
        Command::MoveToolGroup {
            from_index,
            to_index,
        } => {
            return tool_reducer::move_tool_group(state.tool_mut(), from_index, to_index);
        }
        Command::MoveToolEntry {
            group_id,
            from_index,
            to_index,
        } => {
            return tool_reducer::move_tool_entry(
                state.tool_mut(),
                &group_id,
                from_index,
                to_index,
            );
        }
        Command::UpdateFillToolOptions(options) => {
            tool_reducer::update_fill_tool_options(state.tool_mut(), options)
        }
        Command::UpdateShapeToolOptions(options) => {
            tool_reducer::update_shape_tool_options(state.tool_mut(), options)
        }
        Command::UpdateSelectionToolOptions(options) => {
            tool_reducer::update_selection_tool_options(state.tool_mut(), options)
        }
        Command::UpdateColorPickerToolOptions(options) => {
            tool_reducer::update_color_picker_tool_options(state.tool_mut(), options)
        }
        Command::SetSurfaceMirrorXEnabled(enabled) => {
            tool_reducer::set_surface_mirror_x_enabled(state, enabled)
        }
        Command::SetSurfaceMirrorXPlane(plane_x) => {
            tool_reducer::set_surface_mirror_x_plane(state, plane_x)
        }
        Command::SetSurfaceMirrorXPlaneVisible(visible) => {
            tool_reducer::set_surface_mirror_x_plane_visible(state.view_mut(), visible)
        }
        Command::ViewPointer(event) => {
            return view_input_planner::view_pointer_into_with_scratch(state, event, scratch);
        }
        Command::ToolInput(event) => {
            return tool_reducer::tool_input_into_with_scratch(state, event, scratch);
        }
        Command::AdvanceActiveStroke { time_s } => {
            return tool_reducer::advance_active_stroke_into_with_scratch(state, time_s, scratch);
        }
        _ => unreachable!("command was routed to the wrong domain reducer"),
    };
    Ok(produced)
}

fn reduce_transform(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    let produced = match command {
        Command::BeginTransformMode => tool_reducer::begin_transform_mode(state),
        Command::ApplyActiveTransform => {
            return tool_reducer::apply_active_transform(state);
        }
        Command::CancelActiveTransform => tool_reducer::cancel_active_transform(state),
        Command::BeginTransformNumericEdit => {
            return super::transform_controller::begin_transform_numeric_edit(state);
        }
        Command::UpdateTransformNumeric(edit) => {
            return super::transform_controller::update_transform_numeric(state, edit);
        }
        Command::CommitTransformNumericEdit => {
            return super::transform_controller::commit_transform_numeric_edit(state);
        }
        _ => unreachable!("command was routed to the wrong domain reducer"),
    };
    Ok(produced)
}

fn reduce_paint(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    match command {
        Command::FillMaterial {
            material_index,
            opacity,
        } => return tool_reducer::fill_material_into(state, material_index, opacity),
        Command::FillMesh { mesh_id, opacity } => {
            return tool_reducer::fill_mesh_into(state, mesh_id, opacity);
        }
        Command::RectanglePaintUv {
            min_uv,
            max_uv,
            opacity,
        } => return tool_reducer::rectangle_paint_uv_into(state, min_uv, max_uv, opacity),
        Command::RectangleEraseUv {
            min_uv,
            max_uv,
            opacity,
        } => return tool_reducer::rectangle_erase_uv_into(state, min_uv, max_uv, opacity),
        _ => unreachable!("command was routed to the wrong domain reducer"),
    }
}

fn reduce_selection(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    match command {
        Command::CutSurfacePixels { target } => {
            return image_cut_controller::cut_surface_pixels(state, target);
        }
        Command::DeleteSelectedPixels => {
            return image_cut_controller::delete_selected_pixels(state);
        }
        Command::RectangleSelectUv {
            min_uv,
            max_uv,
            operation,
        } => {
            return tool_reducer::rectangle_select_uv_into(state, min_uv, max_uv, operation);
        }
        Command::SelectAll => return Ok(selection_controller::select_all(state)),
        Command::InvertSelection => return Ok(selection_controller::invert_selection(state)),
        Command::DeselectSelection => return tool_reducer::deselect_selection_into(state),
        Command::SelectFromLayerTransparency { layer_id } => {
            return selection_controller::select_from_layer_transparency(state, layer_id);
        }
        _ => unreachable!("command was routed to the wrong domain reducer"),
    }
}

fn reduce_filter(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    match command {
        Command::ApplySurfaceBlur { radius_px } => {
            return surface_filter_controller::apply_surface_blur(state, radius_px);
        }
        Command::ApplySpatialBlur {
            radius_px,
            cross_meshes,
            orientation,
            maximum_normal_angle_degrees,
        } => {
            return surface_filter_controller::apply_spatial_blur(
                state,
                radius_px,
                cross_meshes,
                orientation,
                maximum_normal_angle_degrees,
            );
        }
        Command::ApplyAdjustmentFilter {
            layer_id,
            adjustment,
        } => {
            return surface_filter_controller::apply_adjustment_filter(state, layer_id, adjustment);
        }
        Command::BeginAdjustmentFilterSession { layer_id, kind } => {
            return surface_filter_controller::begin_adjustment_filter_session(
                state, layer_id, kind,
            );
        }
        Command::PreviewAdjustmentFilter {
            layer_id,
            adjustment,
        } => {
            return surface_filter_controller::preview_adjustment_filter(
                state, layer_id, adjustment,
            );
        }
        Command::CommitAdjustmentFilterSession {
            layer_id,
            adjustment,
        } => {
            return surface_filter_controller::commit_adjustment_filter_session(
                state, layer_id, adjustment,
            );
        }
        Command::CancelAdjustmentFilterSession { layer_id } => {
            return surface_filter_controller::cancel_adjustment_filter_session(state, layer_id);
        }
        _ => unreachable!("command was routed to the wrong domain reducer"),
    }
}

fn reduce_history(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    let produced = match command {
        Command::Undo => history_reducer::undo(state)?,
        Command::Redo => history_reducer::redo(state)?,
        _ => unreachable!("command was routed to the wrong domain reducer"),
    };
    Ok(produced)
}

fn reduce_layer(state: &mut AppState, command: Command) -> Result<ReducerOutput> {
    let produced = match command {
        Command::PasteLayerMask {
            layer_id,
            material_index,
            payload,
        } => {
            return layer_mask_paste_controller::paste_layer_mask(
                state,
                layer_id,
                material_index,
                payload,
            );
        }
        Command::AddLayer => layer_reducer::add_layer(state)?,
        Command::AddSolidFillLayer { color } => layer_reducer::add_solid_fill_layer(state, color)?,
        Command::AddAdjustmentLayer { kind } => layer_reducer::add_adjustment_layer(state, kind)?,
        Command::AddGroup => layer_reducer::add_group(state)?,
        Command::DeleteLayer { layer_id } => layer_reducer::delete_layer(state, layer_id)?,
        Command::DeleteLayers { layer_ids } => layer_reducer::delete_layers(state, layer_ids)?,
        Command::DuplicateLayer { layer_id } => layer_reducer::duplicate_layer(state, layer_id)?,
        Command::DuplicateLayers {
            layer_ids,
            primary_layer_id,
        } => layer_reducer::duplicate_layers(state, layer_ids, primary_layer_id)?,
        Command::MergeLayers { layer_ids } => layer_reducer::merge_layers(state, layer_ids)?,
        Command::RasterizeLayer { layer_id } => layer_reducer::rasterize_layer(state, layer_id)?,
        Command::ApplyLayerMask { layer_id } => layer_reducer::apply_layer_mask(state, layer_id)?,
        Command::AddLayerMask { layer_id, mode } => {
            layer_reducer::add_layer_mask(state, layer_id, mode)?
        }
        Command::AddLayerMaskFromSelection { layer_id } => {
            layer_reducer::add_layer_mask_from_selection(state, layer_id)?
        }
        Command::DeleteLayerMask { layer_id } => layer_reducer::delete_layer_mask(state, layer_id)?,
        Command::MoveLayerMask {
            source_layer_id,
            target_layer_id,
        } => layer_reducer::move_layer_mask(state, source_layer_id, target_layer_id)?,
        Command::SelectLayer { layer_id } => layer_reducer::select_layer(state, layer_id)?,
        Command::SelectSolidFillLayer { layer_id } => {
            layer_reducer::select_solid_fill_layer(state, layer_id)?
        }
        Command::SelectAdjustmentLayer { layer_id } => {
            layer_reducer::select_adjustment_layer(state, layer_id)?
        }
        Command::SelectLayerMask { layer_id } => layer_reducer::select_layer_mask(state, layer_id)?,
        Command::SetLayerMaskEnabled { layer_id, enabled } => {
            layer_reducer::set_layer_mask_enabled(state, layer_id, enabled)?
        }
        Command::SelectLayerForStructure { layer_id } => {
            layer_reducer::select_layer_for_structure(state, layer_id)?
        }
        Command::RenameLayer { layer_id, name } => {
            layer_reducer::rename_layer(state, layer_id, name)?
        }
        Command::SetLayerVisible { layer_id, visible } => {
            layer_reducer::set_layer_visible(state, layer_id, visible)?
        }
        Command::SetLayerLocked { layer_id, locked } => {
            layer_reducer::set_layer_locked(state, layer_id, locked)?
        }
        Command::SetLayersLocked { layer_ids, locked } => {
            layer_reducer::set_layers_locked(state, layer_ids, locked)?
        }
        Command::SetLayerMaterialMask {
            layer_id,
            material_mask,
        } => layer_reducer::set_layer_material_mask(state, layer_id, material_mask)?,
        Command::SetEmbeddedImageTransform {
            layer_id,
            transform,
        } => layer_reducer::set_embedded_image_transform(state, layer_id, transform)?,
        Command::SetLayerOpacity { layer_id, opacity } => {
            layer_reducer::set_layer_opacity(state, layer_id, opacity)?
        }
        Command::SetSolidFillColor {
            layer_id,
            color,
            edit_session,
        } => layer_reducer::set_solid_fill_color(state, layer_id, color, edit_session)?,
        Command::SetAdjustment {
            layer_id,
            adjustment,
            edit_session,
        } => layer_reducer::set_adjustment(state, layer_id, adjustment, edit_session)?,
        Command::SetLayersOpacity { layer_ids, opacity } => {
            layer_reducer::set_layers_opacity(state, layer_ids, opacity)?
        }
        Command::SetLayerBlendMode {
            layer_id,
            blend_mode,
        } => layer_reducer::set_layer_blend_mode(state, layer_id, blend_mode)?,
        Command::SetLayersBlendMode {
            layer_ids,
            blend_mode,
        } => layer_reducer::set_layers_blend_mode(state, layer_ids, blend_mode)?,
        Command::SetLayerGroupCompositeMode { layer_id, mode } => {
            layer_reducer::set_layer_group_composite_mode(state, layer_id, mode)?
        }
        Command::SetLayersGroupCompositeMode { layer_ids, mode } => {
            layer_reducer::set_layers_group_composite_mode(state, layer_ids, mode)?
        }
        Command::MoveLayer {
            layer_id,
            new_parent,
            new_index,
        } => layer_reducer::move_layer(state, layer_id, new_parent, new_index)?,
        Command::MoveLayers {
            layer_ids,
            new_parent,
            new_index,
        } => layer_reducer::move_layers(state, layer_ids, new_parent, new_index)?,
        _ => unreachable!("command was routed to the wrong domain reducer"),
    };
    Ok(produced)
}

#[cfg(test)]
pub fn reduce(state: &mut AppState, command: Command) -> crate::renderer::RendererFramePlan {
    let mut scratch = StrokeSampleScratch::default();
    match reduce_to_outcome(state, command, &mut scratch) {
        Ok(outcome) => {
            let commit_request = outcome
                .pending_transaction
                .as_ref()
                .and_then(|pending| pending.renderer_commit_request());
            outcome
                .into_renderer_plan()
                .into_frame_plan(Vec::new(), commit_request)
        }
        Err(err) => {
            state.set_status_message(
                crate::application::StatusMessage::localized("status-operation-failed")
                    .arg("error", format!("{err:#}")),
            );
            crate::renderer::RendererFramePlan::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        application::{
            ApplicationRuntime, Command, HistoryEntry, HistoryTransaction, MeshReloadRequest,
            PixelEdit, PixelSnapshotData, RectU32, SelectionTileEdit,
        },
        core::{
            camera::{CameraProjection, OrbitCamera},
            document::{
                Document, LayerInsertion, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh,
            },
            image::LayerInitialPixels,
            selection::{ActiveSelection, MaterialSelectionMask, SelectionMaskId},
            surface::PaintSurfaceId,
            uv_view::UvViewTransform,
            viewport_shading::ViewportShading,
            wireframe::WireframeStyle,
        },
        renderer::GpuDocumentCommand,
    };

    use glam::{Vec2, Vec3};

    use super::{AppState, reduce};

    #[test]
    fn theme_setting_updates_only_the_user_theme() {
        let mut state = AppState::default();
        let input_before = state.user_settings().input.clone();

        assert!(
            reduce(
                &mut state,
                Command::SetTheme(crate::settings::AppTheme::Light)
            )
            .is_empty()
        );

        assert_eq!(
            state.user_settings().appearance.theme,
            crate::settings::AppTheme::Light
        );
        assert_eq!(state.user_settings().input, input_before);
    }

    #[test]
    fn language_setting_updates_only_user_settings_without_project_changes() {
        let mut state = AppState::default();
        let theme_before = state.user_settings().appearance.theme;

        assert!(
            reduce(
                &mut state,
                Command::SetLanguage(crate::settings::LanguagePreference::Japanese)
            )
            .is_empty()
        );

        assert_eq!(
            state.user_settings().appearance.language,
            crate::settings::LanguagePreference::Japanese
        );
        assert_eq!(state.user_settings().appearance.theme, theme_before);
    }

    #[test]
    fn viewport_camera_reset_and_frame_all_have_distinct_semantics() {
        let mesh_id = MeshId(4);
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            single_triangle_mesh(mesh_id),
            vec![MaterialSpec::new("A", [8, 8])],
        ));
        let mut camera = OrbitCamera::default();
        camera.set_projection(CameraProjection::Orthographic);
        camera.set_view_direction(Vec3::NEG_X, Vec3::Y);
        camera.target = Vec3::splat(100.0);
        let frame_orientation = camera.orientation;
        let frame_projection = camera.projection;
        reduce(&mut state, Command::SetCamera(camera));

        assert!(reduce(&mut state, Command::FrameAllViewport).is_empty());
        let framed = state.camera();
        assert_eq!(framed.target, Vec3::new(0.5, 0.5, 0.0));
        assert_eq!(framed.orientation, frame_orientation);
        assert_eq!(framed.projection, frame_projection);

        assert!(reduce(&mut state, Command::ResetViewportCamera).is_empty());
        let reset = state.camera();
        let default = OrbitCamera::default();
        assert_eq!(reset.target, Vec3::new(0.5, 0.5, 0.0));
        assert_eq!(reset.orientation, default.orientation);
        assert_eq!(reset.projection, CameraProjection::Perspective);
        assert_eq!(reset.fov_y_radians, default.fov_y_radians);
        assert_eq!(reset.near, default.near);
        assert_eq!(reset.far, default.far);
    }

    #[test]
    fn empty_document_frame_all_is_noop_and_camera_reset_uses_default() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            empty_mesh(),
            vec![MaterialSpec::new("A", [8, 8])],
        ));
        let mut changed = OrbitCamera::default();
        changed.target = Vec3::splat(7.0);
        changed.set_projection(CameraProjection::Orthographic);
        reduce(&mut state, Command::SetCamera(changed.clone()));

        reduce(&mut state, Command::FrameAllViewport);
        assert_eq!(state.camera(), &changed);
        reduce(&mut state, Command::ResetViewportCamera);
        assert_eq!(state.camera(), &OrbitCamera::default());
    }

    #[test]
    fn uv_camera_reset_clears_pan_zoom_rotation_and_initialization() {
        let mut state = AppState::default();
        let transform = UvViewTransform {
            center_uv: Vec2::new(2.0, -1.0),
            zoom: 4.0,
            rotation_radians: 0.75,
        };
        reduce(&mut state, Command::SetUvViewTransform(transform));
        assert!(state.uv_view_transform_initialized());
        assert_eq!(state.uv_view_transform(), transform.normalized());

        assert!(reduce(&mut state, Command::ResetUvViewTransform).is_empty());
        assert_eq!(state.uv_view_transform(), UvViewTransform::default());
        assert!(!state.uv_view_transform_initialized());
    }

    #[test]
    fn viewport_visibility_commands_validate_document_targets() {
        let mesh_id = MeshId(4);
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            single_triangle_mesh(mesh_id),
            vec![MaterialSpec::new("A", [8, 8])],
        ));

        let mesh_plan = reduce(
            &mut state,
            Command::SetViewportMeshVisible {
                mesh_id,
                visible: false,
            },
        );
        assert!(mesh_plan.is_empty());
        assert!(!state.viewport_mesh_visible(mesh_id));
        assert_eq!(state.viewport_scene_visibility().revision(), 1);

        let _ = reduce(
            &mut state,
            Command::SetViewportMeshVisible {
                mesh_id: MeshId(99),
                visible: false,
            },
        );
        assert!(state.viewport_mesh_visible(MeshId(99)));
        assert_eq!(state.viewport_scene_visibility().revision(), 1);

        let material_plan = reduce(
            &mut state,
            Command::SetViewportMaterialVisible {
                material_index: 0,
                visible: false,
            },
        );
        assert!(material_plan.is_empty());
        assert!(!state.viewport_material_visible(0));
        assert_eq!(state.viewport_scene_visibility().revision(), 2);

        let _ = reduce(
            &mut state,
            Command::SetViewportMaterialVisible {
                material_index: 8,
                visible: false,
            },
        );
        assert!(state.viewport_material_visible(8));
        assert_eq!(state.viewport_scene_visibility().revision(), 2);
    }

    #[test]
    fn material_render_settings_update_only_the_target_and_skip_unchanged_values() {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        ));

        let plan = reduce(
            &mut state,
            Command::SetMaterialRenderSettings {
                material_index: 1,
                settings: crate::core::material::MaterialRenderSettings {
                    double_sided: false,
                    render_mode: crate::core::material::MaterialRenderMode::Blend,
                    alpha_cutoff: 64,
                },
            },
        );
        let document = state.document().unwrap();
        assert!(document.materials[0].render_settings.double_sided);
        assert!(!document.materials[1].render_settings.double_sided);
        assert!(matches!(
            plan.document_commands.as_slice(),
            [GpuDocumentCommand::SetMaterialRenderSettings {
                material_index: 1,
                settings,
            }] if !settings.double_sided
                && settings.render_mode == crate::core::material::MaterialRenderMode::Blend
                && settings.alpha_cutoff == 64
        ));
        let unchanged_settings = document.materials[1].render_settings;

        let unchanged = reduce(
            &mut state,
            Command::SetMaterialRenderSettings {
                material_index: 1,
                settings: unchanged_settings,
            },
        );
        assert!(unchanged.is_empty());

        let invalid = reduce(
            &mut state,
            Command::SetMaterialRenderSettings {
                material_index: 99,
                settings: crate::core::material::MaterialRenderSettings::default(),
            },
        );
        assert!(invalid.is_empty());
    }

    #[test]
    fn mesh_reload_initializes_new_material_default_layer_white_on_gpu() {
        let mesh_id = MeshId(4);
        let mut state = AppState::default();
        let mut document = Document::new(
            single_triangle_mesh(mesh_id),
            vec![MaterialSpec::new("A", [8, 8])],
        );
        let default_layer = document.layer_tree.default_raster_layer().unwrap();
        let top_layer = document
            .add_raster_layer(LayerInsertion::Above(default_layer), "Top")
            .unwrap()
            .unwrap()
            .layer_id;
        state.document.document = Some(document);

        let mut mesh = single_triangle_mesh(mesh_id);
        let sub_meshes = std::sync::Arc::make_mut(&mut mesh.sub_meshes);
        sub_meshes[0].material_index = 1;
        sub_meshes[0].material_name = "B".to_owned();
        let plan = reduce(
            &mut state,
            Command::ReloadMesh {
                request: MeshReloadRequest {
                    mesh,
                    new_materials: vec![MaterialSpec::new("B", [8, 8])],
                },
            },
        );

        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::CreateSurface {
                target,
                initial: LayerInitialPixels::SolidRgba8([255, 255, 255, 255]),
            } if *target == PaintSurfaceId::raster(1.into(), default_layer)
        )));
        assert!(plan.document_commands.iter().any(|command| matches!(
            command,
            GpuDocumentCommand::CreateSurface {
                target,
                initial: LayerInitialPixels::Transparent,
            } if *target == PaintSurfaceId::raster(1.into(), top_layer)
        )));
    }

    #[test]
    fn surface_mirror_plane_command_accepts_only_finite_values() {
        let mut state = AppState::default();

        assert!(state.surface_mirror_options().show_x_plane);
        let visibility_plan = reduce(&mut state, Command::SetSurfaceMirrorXPlaneVisible(false));
        assert!(!state.surface_mirror_options().show_x_plane);
        assert!(visibility_plan.edit_commands.is_empty());
        assert!(visibility_plan.document_commands.is_empty());

        let plan = reduce(&mut state, Command::SetSurfaceMirrorXPlane(1.25));
        assert_eq!(state.surface_mirror_options().x_plane, 1.25);
        assert!(plan.edit_commands.is_empty());
        assert!(plan.document_commands.is_empty());

        let _ = reduce(&mut state, Command::SetSurfaceMirrorXPlane(f32::NAN));
        let _ = reduce(&mut state, Command::SetSurfaceMirrorXPlane(f32::INFINITY));
        let _ = reduce(
            &mut state,
            Command::SetSurfaceMirrorXPlane(f32::NEG_INFINITY),
        );
        assert_eq!(state.surface_mirror_options().x_plane, 1.25);
    }

    #[test]
    fn editor_view_commands_do_not_mark_the_project_changed() {
        let mut runtime = ApplicationRuntime::new(AppState::default());
        let commands = [
            Command::SetViewportGizmoVisible(false),
            Command::SetViewportShading(ViewportShading::Unlit),
            Command::SetViewportWireframeVisible(false),
            Command::SetViewportWireframeStyle(WireframeStyle::new([0.2, 0.3, 0.4], 0.5)),
            Command::SetViewportBackgroundColor([0.1, 0.2, 0.3]),
            Command::SetUvWireframeVisible(false),
            Command::SetUvWireframeStyle(WireframeStyle::new([0.4, 0.3, 0.2], 0.5)),
            Command::SetUvViewBackgroundColor([0.3, 0.2, 0.1]),
            Command::SetCamera(OrbitCamera::default()),
            Command::SetUvViewTransform(UvViewTransform::default()),
            Command::SetSurfaceMirrorXEnabled(true),
            Command::SetSurfaceMirrorXPlane(1.25),
            Command::SetSurfaceMirrorXPlaneVisible(false),
        ];

        for command in commands {
            assert!(!runtime.dispatch(command).unwrap());
        }
    }

    #[test]
    fn undo_uploads_every_pixel_edit_in_history_entry() {
        let doc = test_document(vec![
            MaterialSpec::new("A", [1, 1]),
            MaterialSpec::new("B", [1, 1]),
        ]);
        let layer_id = doc.layer_tree.default_raster_layer().unwrap();
        let surface_a = crate::core::surface::PaintSurfaceId::raster(0.into(), layer_id);
        let surface_b = crate::core::surface::PaintSurfaceId::raster(1.into(), layer_id);
        let edit = |surface, value| PixelEdit {
            surface,
            texture_size: [1, 1],
            rect: RectU32::full([1, 1]),
            before: PixelSnapshotData::contiguous(vec![value; 4]),
            after: PixelSnapshotData::contiguous(vec![value + 1; 4]),
        };

        let mut state = AppState::default();
        state.history.undo_stack.push(HistoryTransaction::test(
            "Test",
            vec![HistoryEntry::PixelEdit {
                edits: vec![edit(surface_a, 1), edit(surface_b, 3)],
            }],
        ));

        let plan = reduce(&mut state, Command::Undo);
        assert_eq!(plan.document_commands.len(), 2);
        assert!(matches!(
            &plan.document_commands[0],
            GpuDocumentCommand::UploadSurfaceRgba8 { surface, rgba8, .. }
                if *surface == surface_a && rgba8.as_slice() == [1u8; 4]
        ));
        assert!(matches!(
            &plan.document_commands[1],
            GpuDocumentCommand::UploadSurfaceRgba8 { surface, rgba8, .. }
                if *surface == surface_b && rgba8.as_slice() == [3u8; 4]
        ));
    }

    #[test]
    fn undo_redo_selection_history_restores_active_selection_and_tiles() {
        let before = ActiveSelection::disabled_for_materials([0.into()]);
        let after = ActiveSelection {
            enabled: true,
            visible: true,
            masks: vec![MaterialSelectionMask {
                material_index: 0.into(),
                mask_id: Some(SelectionMaskId(7)),
            }],
        };
        let tile = SelectionTileEdit {
            material_index: 0.into(),
            texture_size: [4, 4],
            coord: crate::core::tile::TileCoord { x: 0, y: 0 },
            rect: RectU32::full([4, 4]),
            before: vec![0; 16],
            after: vec![255; 16],
        };
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![MaterialSpec::new("A", [4, 4])]));
        state.document.document.as_mut().unwrap().active_selection = after.clone();
        state.history.undo_stack.push(HistoryTransaction::test(
            "Test",
            vec![HistoryEntry::SelectionEdit {
                before: before.clone(),
                after: after.clone(),
                tiles: vec![tile],
            }],
        ));

        let undo = reduce(&mut state, Command::Undo);
        let redo = reduce(&mut state, Command::Redo);

        assert_eq!(
            state.document.document.as_ref().unwrap().active_selection,
            after
        );
        assert!(matches!(
            undo.document_commands.first(),
            Some(GpuDocumentCommand::UploadSelectionTiles {
                active_selection,
                tiles,
            }) if *active_selection == before
                && tiles.len() == 1
                && tiles[0].r8 == vec![0; 16]
        ));
        assert!(matches!(
            redo.document_commands.first(),
            Some(GpuDocumentCommand::UploadSelectionTiles {
                active_selection,
                tiles,
            }) if *active_selection == after
                && tiles.len() == 1
                && tiles[0].r8 == vec![255; 16]
        ));
    }

    #[test]
    fn undo_only_uploads_restored_layer_pixels() {
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![
            MaterialSpec::new("A", [1, 1]),
            MaterialSpec::new("B", [1, 1]),
        ]));
        let layer_id = state
            .document
            .editor
            .resolved(state.document.document.as_ref().unwrap())
            .active_layer_id;
        let surface_a = crate::core::surface::PaintSurfaceId::raster(0.into(), layer_id);
        let surface_b = crate::core::surface::PaintSurfaceId::raster(1.into(), layer_id);
        let edit = |surface, value| PixelEdit {
            surface,
            texture_size: [1, 1],
            rect: RectU32::full([1, 1]),
            before: PixelSnapshotData::contiguous(vec![value; 4]),
            after: PixelSnapshotData::contiguous(vec![value + 1; 4]),
        };
        state.history.undo_stack.push(HistoryTransaction::test(
            "Test",
            vec![HistoryEntry::PixelEdit {
                edits: vec![edit(surface_a, 1), edit(surface_b, 3)],
            }],
        ));

        let plan = reduce(&mut state, Command::Undo);

        assert_eq!(plan.document_commands.len(), 2);
        assert!(
            plan.document_commands
                .iter()
                .all(|command| matches!(command, GpuDocumentCommand::UploadSurfaceTiles { .. }))
        );
    }

    #[test]
    fn undo_redo_preserve_rect_pixel_history_uploads() {
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![MaterialSpec::new("A", [16, 16])]));
        let surface = state.active_paint_surface().unwrap();
        let rect = RectU32 {
            origin: [4, 5],
            size: [3, 2],
        };
        state.history.undo_stack.push(HistoryTransaction::test(
            "Test",
            vec![HistoryEntry::PixelEdit {
                edits: vec![PixelEdit {
                    surface,
                    texture_size: [16, 16],
                    rect,
                    before: PixelSnapshotData::contiguous(vec![1; 3 * 2 * 4]),
                    after: PixelSnapshotData::contiguous(vec![2; 3 * 2 * 4]),
                }],
            }],
        ));

        let undo = reduce(&mut state, Command::Undo);
        let redo = reduce(&mut state, Command::Redo);

        assert!(matches!(
            undo.document_commands.first(),
            Some(GpuDocumentCommand::UploadSurfaceTiles {
                surface: upload_surface,
                surface_revision: _,
                tiles,
            }) if *upload_surface == surface
                && tiles.len() == 1
                && tiles[0].rect == RectU32::full([16, 16])
                && tiles[0].rgba8[(5 * 16 + 4) * 4..(5 * 16 + 5) * 4] == [1u8; 4]
        ));
        assert!(matches!(
            redo.document_commands.first(),
            Some(GpuDocumentCommand::UploadSurfaceTiles {
                surface: upload_surface,
                surface_revision: _,
                tiles,
            }) if *upload_surface == surface
                && tiles.len() == 1
                && tiles[0].rect == RectU32::full([16, 16])
                && tiles[0].rgba8[(5 * 16 + 4) * 4..(5 * 16 + 5) * 4] == [2u8; 4]
        ));
        assert_eq!(
            state
                .document
                .document
                .as_ref()
                .unwrap()
                .tiles
                .read_surface_rect(surface, rect)
                .unwrap(),
            PixelSnapshotData::contiguous(vec![2; 24])
        );
    }

    #[test]
    fn failed_undo_restores_history_stack_and_reports_error() {
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![MaterialSpec::new("A", [1, 1])]));
        let surface = state.active_paint_surface().unwrap();
        state.history.undo_stack.push(HistoryTransaction::test(
            "Test",
            vec![HistoryEntry::PixelEdit {
                edits: vec![
                    PixelEdit {
                        surface,
                        texture_size: [1, 1],
                        rect: RectU32::full([1, 1]),
                        before: PixelSnapshotData::contiguous(vec![1; 4]),
                        after: PixelSnapshotData::contiguous(vec![2; 4]),
                    },
                    PixelEdit {
                        surface,
                        texture_size: [2, 2],
                        rect: RectU32::full([2, 2]),
                        before: PixelSnapshotData::contiguous(vec![1; 16]),
                        after: PixelSnapshotData::contiguous(vec![2; 16]),
                    },
                ],
            }],
        ));

        let plan = reduce(&mut state, Command::Undo);

        assert!(plan.is_empty());
        assert_eq!(state.history.undo_stack.len(), 1);
        assert!(state.history.redo_stack.is_empty());
        assert_eq!(
            state
                .document()
                .unwrap()
                .tiles
                .read_surface_rect(surface, RectU32::full([1, 1]))
                .unwrap(),
            PixelSnapshotData::contiguous(vec![0; 4])
        );
        assert!(state.status().contains("History pixel restore failed"));
    }

    #[test]
    fn failed_redo_rolls_back_partial_document_changes_and_restores_history_stack() {
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![MaterialSpec::new("A", [1, 1])]));
        let surface = state.active_paint_surface().unwrap();
        state.history.redo_stack.push(HistoryTransaction::test(
            "Test",
            vec![HistoryEntry::PixelEdit {
                edits: vec![
                    PixelEdit {
                        surface,
                        texture_size: [1, 1],
                        rect: RectU32::full([1, 1]),
                        before: PixelSnapshotData::contiguous(vec![1; 4]),
                        after: PixelSnapshotData::contiguous(vec![2; 4]),
                    },
                    PixelEdit {
                        surface,
                        texture_size: [2, 2],
                        rect: RectU32::full([2, 2]),
                        before: PixelSnapshotData::contiguous(vec![1; 16]),
                        after: PixelSnapshotData::contiguous(vec![2; 16]),
                    },
                ],
            }],
        ));

        let plan = reduce(&mut state, Command::Redo);

        assert!(plan.is_empty());
        assert!(state.history.undo_stack.is_empty());
        assert_eq!(state.history.redo_stack.len(), 1);
        assert_eq!(
            state
                .document()
                .unwrap()
                .tiles
                .read_surface_rect(surface, RectU32::full([1, 1]))
                .unwrap(),
            PixelSnapshotData::contiguous(vec![0; 4])
        );
        assert!(state.status().contains("History pixel restore failed"));
    }

    #[test]
    fn tiled_pixel_edit_undo_redo_emits_tile_uploads() {
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![MaterialSpec::new("A", [512, 512])]));
        let surface = state.active_paint_surface().unwrap();
        let rect = RectU32::full([512, 512]);
        let before_bytes = vec![3; 512 * 512 * 4];
        let after_bytes = vec![7; 512 * 512 * 4];
        let before = crate::core::image::Rgba8Snapshot::new(
            [512, 512],
            rect.origin,
            rect.size,
            before_bytes.clone(),
        )
        .unwrap();
        let after = crate::core::image::Rgba8Snapshot::new(
            [512, 512],
            rect.origin,
            rect.size,
            after_bytes.clone(),
        )
        .unwrap();
        state.history.undo_stack.push(HistoryTransaction::test(
            "Test",
            vec![HistoryEntry::PixelEdit {
                edits: vec![PixelEdit {
                    surface,
                    texture_size: [512, 512],
                    rect,
                    before: PixelSnapshotData::from_rgba_snapshot_tiled(&before, 256).unwrap(),
                    after: PixelSnapshotData::from_rgba_snapshot_tiled(&after, 256).unwrap(),
                }],
            }],
        ));

        let undo = reduce(&mut state, Command::Undo);
        let redo = reduce(&mut state, Command::Redo);

        assert!(matches!(
            undo.document_commands.first(),
            Some(GpuDocumentCommand::UploadSurfaceTiles { tiles, .. })
                if tiles.iter().map(|tile| tile.rgba8.len()).sum::<usize>() == before_bytes.len()
                    && tiles.iter().all(|tile| tile.rgba8.iter().all(|value| *value == 3))
        ));
        assert!(matches!(
            redo.document_commands.first(),
            Some(GpuDocumentCommand::UploadSurfaceTiles { tiles, .. })
                if tiles.iter().map(|tile| tile.rgba8.len()).sum::<usize>() == after_bytes.len()
                    && tiles.iter().all(|tile| tile.rgba8.iter().all(|value| *value == 7))
        ));
    }

    #[test]
    fn add_layer_assigns_next_unused_layer_name() {
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![MaterialSpec::new("A", [8, 8])]));

        reduce(&mut state, Command::AddLayer);
        assert_eq!(active_layer_name(&state), "Layer 1");

        reduce(&mut state, Command::AddLayer);
        assert_eq!(active_layer_name(&state), "Layer 2");
    }

    #[test]
    fn add_group_assigns_next_unused_group_name() {
        let mut state = AppState::default();
        state.document.document = Some(test_document(vec![MaterialSpec::new("A", [8, 8])]));

        reduce(&mut state, Command::AddGroup);
        assert_eq!(active_layer_name(&state), "Group 1");

        reduce(&mut state, Command::AddGroup);
        assert_eq!(active_layer_name(&state), "Group 2");
    }

    fn active_layer_name(state: &AppState) -> String {
        let document = state.document.document.as_ref().unwrap();
        let layer_id = state.document.editor.active_layer_id;
        document
            .layer_tree
            .get(layer_id)
            .unwrap()
            .props
            .name
            .clone()
    }

    fn test_document(materials: Vec<MaterialSpec>) -> Document {
        Document::new(empty_mesh(), materials)
    }

    fn single_triangle_mesh(mesh_id: MeshId) -> MeshData {
        MeshData::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            vec![Vec2::ZERO, Vec2::X, Vec2::Y],
            vec![Vec3::Z; 3],
            vec![[0, 1, 2]],
            vec![SubMesh {
                mesh_id,
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "A".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Mesh".to_owned(),
            }],
            vec![mesh_id],
        )
        .expect("single triangle mesh")
    }

    fn empty_mesh() -> MeshData {
        MeshData::empty()
    }
}

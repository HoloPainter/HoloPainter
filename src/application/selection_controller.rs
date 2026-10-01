use glam::{Mat4, Vec2, Vec3};

use crate::core::{
    composite::{ApplyParams, SelectionCompositeMode},
    document::Document,
    geometry::RectU32,
    mask::{
        FullMaskSource, MaskSource, PolygonMaskSource, ProjectionMaskSource, RectangleMaskSource,
        ShapeMaskSource, ViewportPolygonProjectionMaskSource, ViewportProjection,
        ViewportRectProjectionMaskSource,
    },
    math::gl_to_wgpu_depth,
    selection::{ActiveSelection, SelectionTilePayload},
    stroke::SurfaceProjectionId,
    surface::{LayerContent, LayerId, PaintSurfaceId},
};

use super::command::ViewportInputContext;
use super::{
    PendingDocumentCommit, PendingRendererHistoryTransaction, ReducerOutput, RendererCommitSpec,
    SelectionCommit, history_finalizer, state::AppState,
};
use crate::renderer::{EditCommand, GpuDocumentCommand, SelectionEditCommand};

pub(crate) fn lasso_selection_output(
    state: &mut AppState,
    points_uv: Vec<Vec2>,
    operation: SelectionCompositeMode,
) -> ReducerOutput {
    let Some(selection_plan) = lasso_selection_plan(state, points_uv, operation) else {
        return ReducerOutput::default();
    };
    selection_plan.into_output()
}

pub(crate) fn viewport_lasso_selection_output(
    state: &mut AppState,
    points_px: Vec<Vec2>,
    view: ViewportInputContext,
    operation: SelectionCompositeMode,
) -> ReducerOutput {
    let Some(selection_plan) = viewport_lasso_selection_plan(state, points_px, view, operation)
    else {
        return ReducerOutput::default();
    };
    selection_plan.into_output()
}

pub(crate) fn rectangle_selection_output(
    state: &mut AppState,
    min_uv: Vec2,
    max_uv: Vec2,
    operation: SelectionCompositeMode,
) -> ReducerOutput {
    let Some(selection_plan) = rectangle_selection_plan(state, min_uv, max_uv, operation) else {
        return ReducerOutput::default();
    };
    selection_plan.into_output()
}

pub(crate) fn select_all(state: &mut AppState) -> ReducerOutput {
    whole_selection_output(state, SelectionCompositeMode::Replace, "Select All")
}

pub(crate) fn invert_selection(state: &mut AppState) -> ReducerOutput {
    let operation = if state
        .document()
        .is_some_and(|document| document.active_selection.is_active())
    {
        SelectionCompositeMode::Invert
    } else {
        SelectionCompositeMode::Replace
    };
    whole_selection_output(state, operation, "Invert Selection")
}

pub(crate) fn viewport_rectangle_selection_output(
    state: &mut AppState,
    start_px: glam::Vec2,
    end_px: glam::Vec2,
    view: ViewportInputContext,
    operation: SelectionCompositeMode,
) -> ReducerOutput {
    let Some(selection_plan) =
        viewport_rectangle_selection_plan(state, start_px, end_px, view, operation)
    else {
        return ReducerOutput::default();
    };
    selection_plan.into_output()
}

struct SelectionRenderPlan {
    history_label: &'static str,
    mask: MaskSource,
    params: ApplyParams<SelectionCompositeMode>,
    active_selection: crate::core::selection::ActiveSelection,
    history_transaction: PendingRendererHistoryTransaction,
    document_commit: PendingDocumentCommit,
    renderer_commit: RendererCommitSpec,
}

impl SelectionRenderPlan {
    fn into_output(self) -> ReducerOutput {
        self.into_output_after_clearing_materials(&[])
    }

    fn into_output_after_clearing_materials(self, material_indices: &[usize]) -> ReducerOutput {
        let Self {
            history_label,
            mask,
            params,
            active_selection,
            history_transaction,
            document_commit,
            renderer_commit,
        } = self;
        let mut output = ReducerOutput::default();
        for &material_index in material_indices {
            output.push_edit_command(EditCommand::Selection(SelectionEditCommand::Update {
                mask: MaskSource::Full(FullMaskSource::Material { material_index }),
                params: ApplyParams::new(1.0, SelectionCompositeMode::Clear),
                active_selection: active_selection.clone(),
            }));
        }
        output.push_edit_command(EditCommand::Selection(SelectionEditCommand::Update {
            mask,
            params,
            active_selection,
        }));
        output
            .with_optional_renderer_history_transaction(history_label, Some(history_transaction))
            .with_document_commit(document_commit)
            .with_renderer_commit(renderer_commit)
    }
}

fn whole_selection_output(
    state: &AppState,
    operation: SelectionCompositeMode,
    history_label: &'static str,
) -> ReducerOutput {
    let Some(document) = state.document() else {
        return ReducerOutput::default();
    };
    let before_selection = document.active_selection.clone();
    let mut active_selection = before_selection.clone();
    let mut newly_enabled_materials = Vec::new();
    for material_index in 0..document.materials.len() {
        if before_selection
            .material_mask(material_index.into())
            .and_then(|mask| mask.mask_id)
            .is_none()
        {
            newly_enabled_materials.push(material_index);
        }
        active_selection.enable_material_mask(material_index.into());
    }
    active_selection.visible = true;

    let renderer_commit = RendererCommitSpec::with_selection_commit(
        before_selection.clone(),
        active_selection.clone(),
    );
    let plan = SelectionRenderPlan {
        history_label,
        mask: MaskSource::Full(FullMaskSource::MeshAllMaterials),
        params: ApplyParams::new(1.0, operation),
        active_selection: active_selection.clone(),
        history_transaction: PendingRendererHistoryTransaction::Selection {
            before: before_selection,
            after: active_selection.clone(),
        },
        document_commit: PendingDocumentCommit::Selection {
            after: active_selection,
        },
        renderer_commit,
    };
    if operation == SelectionCompositeMode::Invert {
        plan.into_output_after_clearing_materials(&newly_enabled_materials)
    } else {
        plan.into_output()
    }
}

fn rectangle_selection_plan(
    state: &AppState,
    min_uv: Vec2,
    max_uv: Vec2,
    operation: SelectionCompositeMode,
) -> Option<SelectionRenderPlan> {
    let material_index = state.document.focused_material_index();
    let document = state.document()?;
    let before_selection = document.active_selection.clone();
    let mut active_selection = before_selection.clone();
    active_selection.enable_material_mask(material_index.into());
    active_selection.visible = true;

    let renderer_commit = RendererCommitSpec::with_selection_commit(
        before_selection.clone(),
        active_selection.clone(),
    );
    let mask = MaskSource::Shape(ShapeMaskSource::Rectangle(RectangleMaskSource {
        min_uv,
        max_uv,
        material_index: Some(material_index),
    }));
    let params = ApplyParams::new(1.0, operation);
    let history_transaction = PendingRendererHistoryTransaction::Selection {
        before: before_selection,
        after: active_selection.clone(),
    };
    let document_commit = PendingDocumentCommit::Selection {
        after: active_selection.clone(),
    };
    Some(SelectionRenderPlan {
        history_label: selection_history_label(operation),
        mask,
        params,
        active_selection,
        history_transaction,
        document_commit,
        renderer_commit,
    })
}

fn lasso_selection_plan(
    state: &AppState,
    points_uv: Vec<Vec2>,
    operation: SelectionCompositeMode,
) -> Option<SelectionRenderPlan> {
    polygon_bounds(&points_uv)?;
    let material_index = state.document.focused_material_index();
    let document = state.document()?;
    let before_selection = document.active_selection.clone();
    let mut active_selection = before_selection.clone();
    active_selection.enable_material_mask(material_index.into());
    active_selection.visible = true;

    let renderer_commit = RendererCommitSpec::with_selection_commit(
        before_selection.clone(),
        active_selection.clone(),
    );
    let mask = MaskSource::Shape(ShapeMaskSource::Polygon(PolygonMaskSource {
        points_uv,
        material_index: Some(material_index),
    }));
    let params = ApplyParams::new(1.0, operation);
    let history_transaction = PendingRendererHistoryTransaction::Selection {
        before: before_selection,
        after: active_selection.clone(),
    };
    let document_commit = PendingDocumentCommit::Selection {
        after: active_selection.clone(),
    };
    Some(SelectionRenderPlan {
        history_label: selection_history_label(operation),
        mask,
        params,
        active_selection,
        history_transaction,
        document_commit,
        renderer_commit,
    })
}

fn viewport_lasso_selection_plan(
    state: &AppState,
    points_px: Vec<Vec2>,
    view: ViewportInputContext,
    operation: SelectionCompositeMode,
) -> Option<SelectionRenderPlan> {
    let (min_px, max_px) = polygon_bounds(&points_px)?;
    let document = state.document()?;
    let material_indices = viewport_bounds_candidate_material_indices(
        document,
        state.viewport_scene_visibility(),
        min_px,
        max_px,
        view,
    );
    if material_indices.is_empty() {
        return None;
    }

    let before_selection = document.active_selection.clone();
    let mut active_selection = before_selection.clone();
    for material_index in material_indices {
        active_selection.enable_material_mask(material_index.into());
    }
    active_selection.visible = true;

    let renderer_commit = RendererCommitSpec::with_selection_commit(
        before_selection.clone(),
        active_selection.clone(),
    );
    let mask = MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(
        ViewportPolygonProjectionMaskSource {
            points_px,
            viewport_size: view.size,
            projections: vec![ViewportProjection {
                id: SurfaceProjectionId::Primary,
                view_proj: view.view_proj,
            }],
            material_index: None,
            scene_visibility: state.viewport_scene_visibility().clone(),
        },
    ));
    let params = ApplyParams::new(1.0, operation);
    let history_transaction = PendingRendererHistoryTransaction::Selection {
        before: before_selection,
        after: active_selection.clone(),
    };
    let document_commit = PendingDocumentCommit::Selection {
        after: active_selection.clone(),
    };
    Some(SelectionRenderPlan {
        history_label: selection_history_label(operation),
        mask,
        params,
        active_selection,
        history_transaction,
        document_commit,
        renderer_commit,
    })
}

fn viewport_rectangle_selection_plan(
    state: &AppState,
    start_px: glam::Vec2,
    end_px: glam::Vec2,
    view: ViewportInputContext,
    operation: SelectionCompositeMode,
) -> Option<SelectionRenderPlan> {
    let document = state.document()?;
    let material_indices = viewport_bounds_candidate_material_indices(
        document,
        state.viewport_scene_visibility(),
        start_px.min(end_px),
        start_px.max(end_px),
        view,
    );
    if material_indices.is_empty() {
        return None;
    }

    let before_selection = document.active_selection.clone();
    let mut active_selection = before_selection.clone();
    for material_index in material_indices {
        active_selection.enable_material_mask(material_index.into());
    }
    active_selection.visible = true;

    let renderer_commit = RendererCommitSpec::with_selection_commit(
        before_selection.clone(),
        active_selection.clone(),
    );
    let mask = MaskSource::Projection(ProjectionMaskSource::ViewportRect(
        ViewportRectProjectionMaskSource {
            min_px: start_px.min(end_px),
            max_px: start_px.max(end_px),
            viewport_size: view.size,
            projections: vec![ViewportProjection {
                id: SurfaceProjectionId::Primary,
                view_proj: view.view_proj,
            }],
            material_index: None,
            scene_visibility: state.viewport_scene_visibility().clone(),
        },
    ));
    let params = ApplyParams::new(1.0, operation);
    let history_transaction = PendingRendererHistoryTransaction::Selection {
        before: before_selection,
        after: active_selection.clone(),
    };
    let document_commit = PendingDocumentCommit::Selection {
        after: active_selection.clone(),
    };
    Some(SelectionRenderPlan {
        history_label: selection_history_label(operation),
        mask,
        params,
        active_selection,
        history_transaction,
        document_commit,
        renderer_commit,
    })
}

fn viewport_bounds_candidate_material_indices(
    document: &Document,
    scene_visibility: &crate::core::viewport_visibility::ViewportSceneVisibility,
    min_px: Vec2,
    max_px: Vec2,
    view: ViewportInputContext,
) -> Vec<usize> {
    if max_px.x <= min_px.x || max_px.y <= min_px.y {
        return Vec::new();
    }

    let viewport = [view.size[0].max(1), view.size[1].max(1)];
    let view_proj = gl_to_wgpu_depth() * view.view_proj;
    let mut candidates = Vec::new();
    let mut seen = vec![false; document.materials.len()];

    for sub_mesh in document.mesh.sub_meshes.iter() {
        let material_index = sub_mesh.material_index;
        if material_index >= seen.len()
            || seen[material_index]
            || !scene_visibility.geometry_visible(sub_mesh.mesh_id, material_index.into())
        {
            continue;
        }
        let start = sub_mesh.start_index as usize / 3;
        let count = sub_mesh.index_count as usize / 3;
        let intersects_rect = document
            .mesh
            .indices
            .iter()
            .skip(start)
            .take(count)
            .any(|tri| {
                let positions = (*tri).map(|index| document.mesh.positions[index as usize]);
                projected_triangle_bbox_intersects_rect(
                    view_proj, positions, viewport, min_px, max_px,
                )
            });
        if intersects_rect {
            seen[material_index] = true;
            candidates.push(material_index);
        }
    }

    candidates
}

fn projected_triangle_bbox_intersects_rect(
    view_proj: Mat4,
    positions: [Vec3; 3],
    viewport: [u32; 2],
    min_px: Vec2,
    max_px: Vec2,
) -> bool {
    let mut tri_min = Vec2::splat(f32::INFINITY);
    let mut tri_max = Vec2::splat(f32::NEG_INFINITY);
    let mut has_projected_point = false;

    for position in positions {
        let Some(projected) = project_world_to_viewport_px(view_proj, position, viewport) else {
            continue;
        };
        has_projected_point = true;
        tri_min = tri_min.min(projected);
        tri_max = tri_max.max(projected);
    }

    has_projected_point
        && tri_max.x >= min_px.x
        && tri_max.y >= min_px.y
        && tri_min.x <= max_px.x
        && tri_min.y <= max_px.y
}

fn project_world_to_viewport_px(view_proj: Mat4, world: Vec3, viewport: [u32; 2]) -> Option<Vec2> {
    let clip = view_proj * world.extend(1.0);
    if clip.w <= 1e-6 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if ndc.z < 0.0 || ndc.z > 1.0 {
        return None;
    }
    Some(Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport[0] as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport[1] as f32,
    ))
}

pub(crate) fn deselect_selection_output(state: &AppState) -> ReducerOutput {
    let Some(document) = state.document() else {
        return ReducerOutput::default();
    };
    let before_selection = document.active_selection.clone();
    if !before_selection.enabled || !before_selection.has_any_mask() {
        return ReducerOutput::default();
    }

    let after_selection = before_selection.disabled_preserving_masks();
    let mut renderer_selection = after_selection.clone();
    renderer_selection.visible = false;

    let mut output = ReducerOutput::default();
    for mask in before_selection
        .masks
        .iter()
        .filter(|mask| mask.mask_id.is_some())
    {
        let mask = MaskSource::Shape(ShapeMaskSource::Rectangle(RectangleMaskSource {
            min_uv: Vec2::ZERO,
            max_uv: Vec2::ONE,
            material_index: Some(mask.material_index.as_usize()),
        }));
        let params = ApplyParams::new(1.0, SelectionCompositeMode::Clear);
        output.push_edit_command(EditCommand::Selection(SelectionEditCommand::Update {
            mask,
            params,
            active_selection: renderer_selection.clone(),
        }));
    }

    output
        .with_optional_renderer_history_transaction(
            "Deselect",
            Some(PendingRendererHistoryTransaction::Selection {
                before: before_selection.clone(),
                after: after_selection.clone(),
            }),
        )
        .with_document_commit(PendingDocumentCommit::Selection {
            after: after_selection.clone(),
        })
        .with_renderer_commit(RendererCommitSpec::with_selection_commit(
            before_selection,
            after_selection,
        ))
}

pub(crate) fn select_from_layer_transparency(
    state: &mut AppState,
    layer_id: LayerId,
) -> anyhow::Result<ReducerOutput> {
    let Some(document) = state.document() else {
        return Ok(ReducerOutput::default());
    };
    let Some(node) = document.layer_tree.get(layer_id) else {
        return Ok(ReducerOutput::default());
    };
    if !matches!(node.content, LayerContent::Raster) {
        return Ok(ReducerOutput::default());
    }

    let before_selection = document.active_selection.clone();
    let before_commits = selection_commits_from_document(document, &before_selection)?;
    let material_sizes = document
        .materials
        .iter()
        .enumerate()
        .map(|(material_index, material)| (material_index, material.texture_size))
        .collect::<Vec<_>>();
    let mut layer_alpha = Vec::with_capacity(material_sizes.len());
    let mut has_selected_pixels = false;
    for &(material_index, texture_size) in &material_sizes {
        let image = document
            .tiles
            .read_surface_full(PaintSurfaceId::raster(material_index.into(), layer_id))?;
        let r8 = image
            .rgba8
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>();
        has_selected_pixels |= r8.iter().any(|&value| value != 0);
        layer_alpha.push(SelectionCommit {
            material_index: material_index.into(),
            texture_size,
            r8,
        });
    }

    let after_selection = if has_selected_pixels {
        let mut selection = ActiveSelection::disabled_for_materials(
            material_sizes
                .iter()
                .map(|(material_index, _)| (*material_index).into()),
        );
        for &(material_index, _) in &material_sizes {
            selection.enable_material_mask(material_index.into());
        }
        selection.visible = true;
        selection
    } else {
        let mut selection = before_selection.disabled_preserving_masks();
        selection.visible = true;
        selection
    };

    let after_commits = if has_selected_pixels {
        layer_alpha
    } else {
        after_selection
            .masks
            .iter()
            .filter(|mask| mask.mask_id.is_some())
            .map(|mask| {
                Ok(SelectionCommit {
                    material_index: mask.material_index,
                    texture_size: document
                        .material(mask.material_index)
                        .ok_or_else(|| anyhow::anyhow!("selection material does not exist"))?
                        .texture_size,
                    r8: vec![
                        0;
                        crate::core::selection::selection_mask_len(
                            document
                                .material(mask.material_index)
                                .ok_or_else(|| anyhow::anyhow!(
                                    "selection material does not exist"
                                ))?
                                .texture_size,
                        )?
                    ],
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    };

    let Some(history_atom) = history_finalizer::build_selection_history_atom(
        before_selection,
        after_selection.clone(),
        &before_commits,
        &after_commits,
    )?
    else {
        return Ok(ReducerOutput::default());
    };

    let tiles = after_commits
        .iter()
        .map(|commit| SelectionTilePayload {
            material_index: commit.material_index,
            rect: RectU32::full(commit.texture_size),
            r8: commit.r8.clone(),
        })
        .collect::<Vec<_>>();

    if let Some(document) = state.document_mut() {
        document.active_selection = after_selection.clone();
        for commit in &after_commits {
            document.selection_masks.write_mask(
                commit.material_index,
                commit.texture_size,
                commit.r8.clone(),
            )?;
        }
    }

    state.set_status_key("status-selection-loaded-from-layer");
    let mut output = ReducerOutput::default()
        .with_finalized_history_atom("Select From Layer Transparency", history_atom);
    output.push_document_command(GpuDocumentCommand::UploadSelectionTiles {
        active_selection: after_selection,
        tiles,
    });
    Ok(output)
}

fn selection_commits_from_document(
    document: &Document,
    selection: &ActiveSelection,
) -> anyhow::Result<Vec<SelectionCommit>> {
    selection
        .masks
        .iter()
        .filter(|mask| mask.mask_id.is_some())
        .map(|mask| {
            Ok(SelectionCommit {
                material_index: mask.material_index,
                texture_size: document
                    .material(mask.material_index)
                    .ok_or_else(|| anyhow::anyhow!("selection material does not exist"))?
                    .texture_size,
                r8: document.selection_masks.mask_bytes_or_zeros(
                    mask.material_index,
                    document
                        .material(mask.material_index)
                        .ok_or_else(|| anyhow::anyhow!("selection material does not exist"))?
                        .texture_size,
                )?,
            })
        })
        .collect()
}

fn selection_history_label(operation: SelectionCompositeMode) -> &'static str {
    match operation {
        SelectionCompositeMode::Replace => "Replace Selection",
        SelectionCompositeMode::Add => "Add Selection",
        SelectionCompositeMode::Subtract => "Subtract Selection",
        SelectionCompositeMode::Intersect => "Intersect Selection",
        SelectionCompositeMode::Difference => "Difference Selection",
        SelectionCompositeMode::Clear => "Clear Selection",
        SelectionCompositeMode::Invert => "Invert Selection",
    }
}

fn polygon_bounds(points: &[Vec2]) -> Option<(Vec2, Vec2)> {
    if points.len() < 3 {
        return None;
    }
    let min = points
        .iter()
        .copied()
        .fold(Vec2::splat(f32::INFINITY), Vec2::min);
    let max = points
        .iter()
        .copied()
        .fold(Vec2::splat(f32::NEG_INFINITY), Vec2::max);
    (max.x > min.x && max.y > min.y).then_some((min, max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::PixelSnapshotData,
        core::document::{Document, MaterialSpec, MeshData},
    };

    fn state_with_two_materials() -> (AppState, LayerId) {
        let mut state = AppState::default();
        state.document.document = Some(Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [2, 1]),
                MaterialSpec::new("B", [2, 1]),
            ],
        ));
        state.document.editor = crate::application::state::EditorDocumentState::from_document(
            state.document().unwrap(),
        );
        let layer_id = state
            .document()
            .unwrap()
            .layer_tree
            .default_raster_layer()
            .unwrap();
        (state, layer_id)
    }

    #[test]
    fn select_from_layer_transparency_replaces_selection_with_alpha_for_all_materials() {
        let (mut state, layer_id) = state_with_two_materials();
        {
            let document = state.document_mut().unwrap();
            document.active_selection.enable_material_mask(0.into());
            document
                .selection_masks
                .write_mask(0.into(), [2, 1], vec![255, 255])
                .unwrap();
            document
                .tiles
                .write_surface_rect(
                    PaintSurfaceId::raster(0.into(), layer_id),
                    RectU32::full([2, 1]),
                    &PixelSnapshotData::contiguous(vec![10, 20, 30, 0, 40, 50, 60, 128]),
                )
                .unwrap();
            document
                .tiles
                .write_surface_rect(
                    PaintSurfaceId::raster(1.into(), layer_id),
                    RectU32::full([2, 1]),
                    &PixelSnapshotData::contiguous(vec![70, 80, 90, 255, 1, 2, 3, 64]),
                )
                .unwrap();
        }

        let output = select_from_layer_transparency(&mut state, layer_id).unwrap();

        let document = state.document().unwrap();
        assert!(document.active_selection.enabled);
        assert!(document.active_selection.visible);
        assert!(
            document
                .active_selection
                .masks
                .iter()
                .all(|mask| mask.mask_id.is_some())
        );
        assert_eq!(
            document
                .selection_masks
                .mask_bytes_or_zeros(0.into(), [2, 1])
                .unwrap(),
            vec![0, 128]
        );
        assert_eq!(
            document
                .selection_masks
                .mask_bytes_or_zeros(1.into(), [2, 1])
                .unwrap(),
            vec![255, 64]
        );
        assert!(output.pending_transaction.is_some());
    }

    #[test]
    fn select_from_fully_transparent_layer_clears_existing_selection() {
        let (mut state, layer_id) = state_with_two_materials();
        {
            let document = state.document_mut().unwrap();
            document.active_selection.enable_material_mask(0.into());
            document
                .selection_masks
                .write_mask(0.into(), [2, 1], vec![255, 128])
                .unwrap();
        }

        let output = select_from_layer_transparency(&mut state, layer_id).unwrap();

        let document = state.document().unwrap();
        assert!(!document.active_selection.enabled);
        assert_eq!(
            document
                .selection_masks
                .mask_bytes_or_zeros(0.into(), [2, 1])
                .unwrap(),
            vec![0, 0]
        );
        assert!(output.pending_transaction.is_some());
    }
}

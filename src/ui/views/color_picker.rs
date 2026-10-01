use crate::{
    application::AppState,
    core::surface::{LayerId, PaintSurfaceId},
};

pub(super) fn current_layer_surface(
    state: &AppState,
    material_index: usize,
) -> Option<(PaintSurfaceId, LayerId)> {
    let document = state.document()?;
    let material = document.materials.get(material_index)?;
    let layer_id = state.active_layer_id();
    if !document.layer_tree.is_paintable(layer_id)
        || !document
            .layer_tree
            .effective_material_allowed(layer_id, material.id)
    {
        return None;
    }
    Some((
        PaintSurfaceId::raster(material_index.into(), layer_id),
        layer_id,
    ))
}

pub(super) fn unit_uv(uv: glam::Vec2) -> Option<[f32; 2]> {
    if !uv.is_finite() || !(0.0..=1.0).contains(&uv.x) || !(0.0..=1.0).contains(&uv.y) {
        return None;
    }
    Some([uv.x, uv.y])
}

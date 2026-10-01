use glam::Vec2;

use crate::core::{
    composite::{ApplyParams, TextureCompositeMode},
    document::Document,
    mask::{
        MaskSource, PolygonMaskSource, ProjectionMaskSource, RectangleMaskSource, ShapeMaskSource,
        ViewportPolygonProjectionMaskSource, ViewportProjection, ViewportRectProjectionMaskSource,
    },
    stroke::SurfaceProjectionId,
    tool::ShapeToolKind,
    tool_operation::{PaintOperation, PaintSource, ToolOperation},
    viewport_visibility::ViewportSceneVisibility,
};

use super::{
    ApplyOneShotRenderPlan, DamageMap, command::ViewportInputContext,
    history_capture::PixelHistoryCapture, state::ResolvedPaintTarget,
    stroke::surface_mirror::mirror_x_view, uv_rect_to_pixel_rect,
};

pub(crate) fn lasso_paint_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    focused_material_index: usize,
    points_uv: Vec<Vec2>,
    color: [f32; 3],
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    lasso_plan_for_target_with_composite(
        document,
        paint_target,
        focused_material_index,
        points_uv,
        PaintSource::SolidColor(color),
        TextureCompositeMode::SourceOver,
        opacity,
    )
}

pub(crate) fn lasso_erase_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    focused_material_index: usize,
    points_uv: Vec<Vec2>,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    lasso_plan_for_target_with_composite(
        document,
        paint_target,
        focused_material_index,
        points_uv,
        PaintSource::SolidColor([0.0, 0.0, 0.0]),
        TextureCompositeMode::DestinationOut,
        opacity,
    )
}

fn lasso_plan_for_target_with_composite(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    focused_material_index: usize,
    points_uv: Vec<Vec2>,
    source: PaintSource,
    composite: TextureCompositeMode,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    let target = paint_target.target();
    let (min_uv, max_uv) = polygon_bounds(&points_uv)?;
    let damage = document
        .texture_size_for_surface(target)
        .and_then(|texture_size| uv_rect_to_pixel_rect(texture_size, min_uv, max_uv, 0))
        .map(|rect| {
            let mut damage = DamageMap::default();
            damage.add_rect(target, rect);
            damage
        });
    Some(ApplyOneShotRenderPlan::new(
        shape_history_label(ShapeToolKind::Lasso, composite),
        target,
        vec![target],
        MaskSource::Shape(ShapeMaskSource::Polygon(PolygonMaskSource {
            points_uv,
            material_index: Some(focused_material_index),
        })),
        ToolOperation::Paint(PaintOperation { source }),
        ApplyParams::new(opacity, composite),
        document.active_selection.clone(),
        damage.clone(),
        PixelHistoryCapture::from_optional_damage(damage),
    ))
}

pub(crate) fn viewport_lasso_paint_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    points_px: Vec<Vec2>,
    view: ViewportInputContext,
    mirror_x_plane: Option<f32>,
    scene_visibility: &ViewportSceneVisibility,
    color: [f32; 3],
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    viewport_lasso_plan_for_target_with_composite(
        document,
        paint_target,
        points_px,
        view,
        mirror_x_plane,
        scene_visibility,
        PaintSource::SolidColor(color),
        TextureCompositeMode::SourceOver,
        opacity,
    )
}

pub(crate) fn viewport_lasso_erase_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    points_px: Vec<Vec2>,
    view: ViewportInputContext,
    mirror_x_plane: Option<f32>,
    scene_visibility: &ViewportSceneVisibility,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    viewport_lasso_plan_for_target_with_composite(
        document,
        paint_target,
        points_px,
        view,
        mirror_x_plane,
        scene_visibility,
        PaintSource::SolidColor([0.0, 0.0, 0.0]),
        TextureCompositeMode::DestinationOut,
        opacity,
    )
}

fn viewport_lasso_plan_for_target_with_composite(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    points_px: Vec<Vec2>,
    view: ViewportInputContext,
    mirror_x_plane: Option<f32>,
    scene_visibility: &ViewportSceneVisibility,
    source: PaintSource,
    composite: TextureCompositeMode,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    let target = paint_target.target();
    let surfaces =
        visible_projection_surfaces(document, paint_target.surfaces_vec(), scene_visibility);
    if surfaces.is_empty() || polygon_bounds(&points_px).is_none() {
        return None;
    }
    let damage = viewport_rect_damage(document, &surfaces);
    Some(ApplyOneShotRenderPlan::new(
        shape_history_label(ShapeToolKind::Lasso, composite),
        target,
        surfaces,
        MaskSource::Projection(ProjectionMaskSource::ViewportPolygon(
            ViewportPolygonProjectionMaskSource {
                points_px,
                viewport_size: view.size,
                projections: viewport_shape_projections(view, mirror_x_plane),
                material_index: None,
                scene_visibility: scene_visibility.clone(),
            },
        )),
        ToolOperation::Paint(PaintOperation { source }),
        ApplyParams::new(opacity, composite),
        document.active_selection.clone(),
        damage.clone(),
        PixelHistoryCapture::from_optional_damage(damage),
    ))
}

pub(crate) fn rectangle_paint_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    focused_material_index: usize,
    min_uv: Vec2,
    max_uv: Vec2,
    color: [f32; 3],
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    rectangle_paint_plan_for_target_with_composite(
        document,
        paint_target,
        focused_material_index,
        min_uv,
        max_uv,
        PaintSource::SolidColor(color),
        TextureCompositeMode::SourceOver,
        opacity,
    )
}

pub(crate) fn viewport_rectangle_paint_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    start_px: Vec2,
    end_px: Vec2,
    view: ViewportInputContext,
    mirror_x_plane: Option<f32>,
    scene_visibility: &ViewportSceneVisibility,
    color: [f32; 3],
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    viewport_rectangle_plan_for_target_with_composite(
        document,
        paint_target,
        start_px,
        end_px,
        view,
        mirror_x_plane,
        scene_visibility,
        PaintSource::SolidColor(color),
        TextureCompositeMode::SourceOver,
        opacity,
    )
}

pub(crate) fn viewport_rectangle_erase_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    start_px: Vec2,
    end_px: Vec2,
    view: ViewportInputContext,
    mirror_x_plane: Option<f32>,
    scene_visibility: &ViewportSceneVisibility,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    viewport_rectangle_plan_for_target_with_composite(
        document,
        paint_target,
        start_px,
        end_px,
        view,
        mirror_x_plane,
        scene_visibility,
        PaintSource::SolidColor([0.0, 0.0, 0.0]),
        TextureCompositeMode::DestinationOut,
        opacity,
    )
}

fn viewport_rectangle_plan_for_target_with_composite(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    start_px: Vec2,
    end_px: Vec2,
    view: ViewportInputContext,
    mirror_x_plane: Option<f32>,
    scene_visibility: &ViewportSceneVisibility,
    source: PaintSource,
    composite: TextureCompositeMode,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    let target = paint_target.target();
    let surfaces =
        visible_projection_surfaces(document, paint_target.surfaces_vec(), scene_visibility);
    if surfaces.is_empty() {
        return None;
    }
    let damage = viewport_rect_damage(document, &surfaces);
    Some(ApplyOneShotRenderPlan::new(
        shape_history_label(ShapeToolKind::Rectangle, composite),
        target,
        surfaces,
        MaskSource::Projection(ProjectionMaskSource::ViewportRect(
            ViewportRectProjectionMaskSource {
                min_px: start_px.min(end_px),
                max_px: start_px.max(end_px),
                viewport_size: view.size,
                projections: viewport_shape_projections(view, mirror_x_plane),
                material_index: None,
                scene_visibility: scene_visibility.clone(),
            },
        )),
        ToolOperation::Paint(PaintOperation { source }),
        ApplyParams::new(opacity, composite),
        document.active_selection.clone(),
        damage.clone(),
        PixelHistoryCapture::from_optional_damage(damage),
    ))
}

pub(crate) fn rectangle_erase_plan_for_target(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    focused_material_index: usize,
    min_uv: Vec2,
    max_uv: Vec2,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    rectangle_paint_plan_for_target_with_composite(
        document,
        paint_target,
        focused_material_index,
        min_uv,
        max_uv,
        PaintSource::SolidColor([0.0, 0.0, 0.0]),
        TextureCompositeMode::DestinationOut,
        opacity,
    )
}

fn viewport_shape_projections(
    view: ViewportInputContext,
    mirror_x_plane: Option<f32>,
) -> Vec<ViewportProjection> {
    let mut projections = vec![ViewportProjection {
        id: SurfaceProjectionId::Primary,
        view_proj: view.view_proj,
    }];
    if let Some(mirror_view) = mirror_x_plane.and_then(|plane_x| mirror_x_view(view, plane_x)) {
        projections.push(ViewportProjection {
            id: SurfaceProjectionId::MirrorX,
            view_proj: mirror_view.view_proj,
        });
    }
    projections
}

fn visible_projection_surfaces(
    document: &Document,
    surfaces: Vec<crate::core::surface::PaintSurfaceId>,
    scene_visibility: &ViewportSceneVisibility,
) -> Vec<crate::core::surface::PaintSurfaceId> {
    surfaces
        .into_iter()
        .filter(|surface| {
            document.mesh.sub_meshes.iter().any(|sub_mesh| {
                sub_mesh.material_index == surface.material_index().as_usize()
                    && scene_visibility
                        .geometry_visible(sub_mesh.mesh_id, sub_mesh.material_index.into())
            })
        })
        .collect()
}

fn viewport_rect_damage(
    document: &Document,
    surfaces: &[crate::core::surface::PaintSurfaceId],
) -> Option<DamageMap> {
    let mut damage = DamageMap::default();
    for surface in surfaces {
        let Some(texture_size) = document.texture_size_for_surface(*surface) else {
            continue;
        };
        damage.add_full_surface(*surface, texture_size);
    }
    (!damage.is_empty()).then_some(damage)
}

fn rectangle_paint_plan_for_target_with_composite(
    document: &Document,
    paint_target: ResolvedPaintTarget,
    focused_material_index: usize,
    min_uv: Vec2,
    max_uv: Vec2,
    source: PaintSource,
    composite: TextureCompositeMode,
    opacity: f32,
) -> Option<ApplyOneShotRenderPlan> {
    let target = paint_target.target();
    let damage = document
        .texture_size_for_surface(target)
        .and_then(|texture_size| uv_rect_to_pixel_rect(texture_size, min_uv, max_uv, 0))
        .map(|rect| {
            let mut damage = DamageMap::default();
            damage.add_rect(target, rect);
            damage
        });
    Some(ApplyOneShotRenderPlan::new(
        shape_history_label(ShapeToolKind::Rectangle, composite),
        target,
        vec![target],
        MaskSource::Shape(ShapeMaskSource::Rectangle(RectangleMaskSource {
            min_uv,
            max_uv,
            material_index: Some(focused_material_index),
        })),
        ToolOperation::Paint(PaintOperation { source }),
        ApplyParams::new(opacity, composite),
        document.active_selection.clone(),
        damage.clone(),
        PixelHistoryCapture::from_optional_damage(damage),
    ))
}

fn shape_history_label(shape: ShapeToolKind, composite: TextureCompositeMode) -> &'static str {
    match (shape, composite) {
        (
            ShapeToolKind::Rectangle,
            TextureCompositeMode::DestinationOut | TextureCompositeMode::Clear,
        ) => "Rectangle Erase",
        (
            ShapeToolKind::Rectangle,
            TextureCompositeMode::SourceOver | TextureCompositeMode::Multiply,
        ) => "Rectangle Paint",
        (
            ShapeToolKind::Lasso,
            TextureCompositeMode::DestinationOut | TextureCompositeMode::Clear,
        ) => "Lasso Erase",
        (
            ShapeToolKind::Lasso,
            TextureCompositeMode::SourceOver | TextureCompositeMode::Multiply,
        ) => "Lasso Paint",
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
    use glam::{Mat4, Vec3};

    use crate::{
        application::{ViewportInputContext, stroke::surface_mirror::mirror_x_matrix},
        core::stroke::SurfaceProjectionId,
    };

    use super::viewport_shape_projections;

    fn view() -> ViewportInputContext {
        ViewportInputContext {
            size: [640, 480],
            view_proj: Mat4::from_translation(Vec3::new(0.25, -0.5, 0.75)),
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [2.0, 3.0, 4.0],
        }
    }

    #[test]
    fn viewport_shape_projection_uses_primary_only_without_mirror() {
        let view = view();
        let projections = viewport_shape_projections(view, None);

        assert_eq!(projections.len(), 1);
        assert_eq!(projections[0].id, SurfaceProjectionId::Primary);
        assert_eq!(projections[0].view_proj, view.view_proj);
    }

    #[test]
    fn viewport_shape_projection_adds_mirror_x_view() {
        let view = view();
        let plane_x = 1.25;
        let projections = viewport_shape_projections(view, Some(plane_x));

        assert_eq!(projections.len(), 2);
        assert_eq!(projections[0].id, SurfaceProjectionId::Primary);
        assert_eq!(projections[1].id, SurfaceProjectionId::MirrorX);
        assert!(
            projections[1]
                .view_proj
                .abs_diff_eq(view.view_proj * mirror_x_matrix(plane_x), 1e-6)
        );
    }

    #[test]
    fn viewport_shape_projection_ignores_invalid_mirror_plane() {
        let projections = viewport_shape_projections(view(), Some(f32::NAN));

        assert_eq!(projections.len(), 1);
        assert_eq!(projections[0].id, SurfaceProjectionId::Primary);
    }
}

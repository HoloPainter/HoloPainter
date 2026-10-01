use crate::core::{
    damage::{DamageMap, rects_touch, union_rect},
    document::{Document, MeshId},
    stroke::{PaintSurfaceSet, StrokeDab, StrokeSpace, SurfaceDab},
    stroke_preset::StrokeOp,
    surface::PaintSurfaceId,
    viewport_visibility::ViewportSceneVisibility,
};
use glam::Vec2;

use super::{
    AppState, PointerSample, RectU32, ResolvedStrokeStyle,
    stroke::sampling::predicted_uv_flush_dabs,
};

pub const STROKE_DAMAGE_GUARD_PX: u32 = 8;
pub const SURFACE_STROKE_DAMAGE_GUARD_PX: f32 = 32.0;
pub const SURFACE_STROKE_DAMAGE_MAX_AREA_RATIO: f32 = 0.45;
pub const SURFACE_STROKE_DAMAGE_MAX_RECTS: usize = 16;
pub const GEOMETRY_DAMAGE_GUARD_PX: u32 = 4;

pub fn estimate_uv_stroke_pointer_up_damage(state: &AppState, sample: &PointerSample) -> DamageMap {
    let mut damage = DamageMap::default();
    if sample.space() != StrokeSpace::Uv {
        return damage;
    }
    let Some(document) = state.document() else {
        return damage;
    };
    let Some(session) = state.tool.stroke_session(StrokeSpace::Uv) else {
        return damage;
    };
    let predicted = predicted_uv_flush_dabs(state, sample);
    let target = session.target();
    damage = session.stroke_damage.clone();
    if let Some(predicted_damage) = estimate_uv_stroke_damage_from_dabs(
        document,
        target,
        session.style().as_ref(),
        predicted.iter().copied(),
    ) {
        for pixel in predicted_damage.pixels {
            damage.add_rect(pixel.surface, pixel.rect);
        }
    }
    if damage.is_empty()
        && let Some(texture_size) = document.texture_size_for_surface(target)
    {
        damage.add_full_surface(target, texture_size);
    }
    damage
}

pub fn stroke_dabs_to_pixel_rect_iter<I>(
    texture_size: [u32; 2],
    dabs: I,
    radius_px: f32,
    guard_px: u32,
    positions_are_uv: bool,
) -> Option<RectU32>
where
    I: IntoIterator<Item = StrokeDab>,
{
    if !radius_px.is_finite() || radius_px <= 0.0 {
        return None;
    }
    let mut saw_dab = false;
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for dab in dabs {
        saw_dab = true;
        let center = if positions_are_uv {
            Vec2::new(
                dab.position.x * texture_size[0] as f32,
                dab.position.y * texture_size[1] as f32,
            )
        } else {
            dab.position
        };
        let radius = radius_px * dab.radius_scale.max(0.0) + guard_px as f32;
        if !(center.x.is_finite() && center.y.is_finite() && radius.is_finite()) {
            return None;
        }
        min_x = min_x.min(center.x - radius);
        min_y = min_y.min(center.y - radius);
        max_x = max_x.max(center.x + radius);
        max_y = max_y.max(center.y + radius);
    }
    if !saw_dab {
        return None;
    }
    pixel_bbox_to_rect(texture_size, min_x, min_y, max_x, max_y, 0)
}

pub fn uv_stroke_radius_px(
    document: &Document,
    surface: PaintSurfaceId,
    style: &ResolvedStrokeStyle,
) -> Option<f32> {
    let texture_size = document.texture_size_for_surface(surface)?;
    let StrokeOp::BrushEngine { radius_world, .. } = &style.stroke_op;
    let radius_world = *radius_world;
    let radius = document
        .mesh
        .world_to_uv_texel_radius(radius_world, texture_size);
    (radius.is_finite() && radius > 0.0).then_some(radius.max(1.0))
}

pub fn estimate_uv_stroke_damage_from_dabs<I>(
    document: &Document,
    surface: PaintSurfaceId,
    style: &ResolvedStrokeStyle,
    dabs: I,
) -> Option<DamageMap>
where
    I: IntoIterator<Item = StrokeDab>,
{
    let texture_size = document.texture_size_for_surface(surface)?;
    let radius_px = uv_stroke_radius_px(document, surface, style)?;
    let rect = stroke_dabs_to_pixel_rect_iter(
        texture_size,
        dabs,
        radius_px,
        STROKE_DAMAGE_GUARD_PX,
        true,
    )?;
    let mut damage = DamageMap::default();
    damage.add_rect(surface, rect);
    Some(damage)
}

pub fn estimate_surface_stroke_damage_from_dabs(
    document: &Document,
    target_surfaces: &PaintSurfaceSet,
    target_materials_by_dab: &[Vec<usize>],
    style: &ResolvedStrokeStyle,
    dabs: &[SurfaceDab],
) -> Option<DamageMap> {
    if target_surfaces.is_empty() || dabs.is_empty() {
        return None;
    }
    let target_materials_by_dab =
        (target_materials_by_dab.len() == dabs.len()).then_some(target_materials_by_dab);
    let mut damage = DamageMap::default();
    for surface in target_surfaces.iter() {
        let texture_size = document.texture_size_for_surface(surface)?;
        match surface_stroke_rects_for_dabs(
            document,
            texture_size,
            surface.material_index().as_usize(),
            target_materials_by_dab,
            style,
            dabs,
        ) {
            Some(rects) => {
                for rect in rects {
                    damage.add_rect(surface, rect);
                }
            }
            None => damage.add_full_surface(surface, texture_size),
        }
    }
    (!damage.is_empty()).then_some(damage)
}

fn surface_stroke_rects_for_dabs(
    document: &Document,
    texture_size: [u32; 2],
    material_index: usize,
    target_materials_by_dab: Option<&[Vec<usize>]>,
    style: &ResolvedStrokeStyle,
    dabs: &[SurfaceDab],
) -> Option<Vec<RectU32>> {
    let StrokeOp::BrushEngine { radius_world, .. } = &style.stroke_op;
    let radius_uv_base = document.mesh.world_to_uv_radius(*radius_world);
    if !(radius_uv_base.is_finite() && radius_uv_base > 0.0) {
        return None;
    }

    let texture_width = texture_size[0].max(1) as f32;
    let texture_height = texture_size[1].max(1) as f32;
    let guard_px = SURFACE_STROKE_DAMAGE_GUARD_PX;
    let guard_uv = (guard_px / texture_width).max(guard_px / texture_height);
    let mut rects = Vec::new();
    let mut saw_dab = false;

    for (dab_index, dab) in dabs.iter().enumerate() {
        if let Some(target_materials) = target_materials_by_dab {
            let Some(dab_target_materials) = target_materials.get(dab_index) else {
                return None;
            };
            if !dab_target_materials.contains(&material_index) {
                continue;
            }
        }
        let uv = dab.uv?;
        let uv_paint_boundary_distance = dab.uv_paint_boundary_distance?;
        let radius_uv = radius_uv_base * dab.radius_scale.max(0.0);
        if !(uv.x.is_finite()
            && uv.y.is_finite()
            && !uv_paint_boundary_distance.is_nan()
            && radius_uv.is_finite())
        {
            return None;
        }

        let mut contributed = false;
        if dab.material_index == material_index {
            let center_x = uv.x * texture_width;
            let center_y = uv.y * texture_height;
            let radius_x = radius_uv * texture_width + guard_px;
            let radius_y = radius_uv * texture_height + guard_px;
            let dab_rect = pixel_bbox_to_rect(
                texture_size,
                center_x - radius_x,
                center_y - radius_y,
                center_x + radius_x,
                center_y + radius_y,
                0,
            )?;
            push_merged_surface_damage_rect(&mut rects, dab_rect);
            contributed = true;
        }

        if uv_paint_boundary_distance <= radius_uv + guard_uv {
            let Some(triangle_index) = dab.triangle_index else {
                return None;
            };
            let boundary_rects = document.mesh.uv_boundary_rects_for_target_material(
                triangle_index,
                uv,
                material_index,
                radius_uv,
                texture_size,
                guard_px,
            )?;
            for rect in boundary_rects {
                push_merged_surface_damage_rect(&mut rects, rect);
                contributed = true;
            }
        } else if dab.material_index != material_index {
            return None;
        }

        if contributed {
            saw_dab = true;
        } else {
            return None;
        }
    }

    if !saw_dab || rects.is_empty() {
        return None;
    }
    if rects.iter().any(|rect| rect.size == texture_size) {
        return None;
    }
    if rects.len() > SURFACE_STROKE_DAMAGE_MAX_RECTS {
        return None;
    }
    let full_area = texture_size[0].max(1) as f32 * texture_size[1].max(1) as f32;
    let rect_area: f32 = rects
        .iter()
        .map(|rect| rect.size[0] as f32 * rect.size[1] as f32)
        .sum();
    if rect_area / full_area > SURFACE_STROKE_DAMAGE_MAX_AREA_RATIO {
        return None;
    }
    Some(rects)
}

fn push_merged_surface_damage_rect(rects: &mut Vec<RectU32>, rect: RectU32) {
    rects.push(rect);
    let mut changed = true;
    while changed {
        changed = false;
        'outer: for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                if rects_touch(rects[i], rects[j]) {
                    let merged = union_rect(rects[i], rects[j]);
                    rects[i] = merged;
                    rects.remove(j);
                    changed = true;
                    break 'outer;
                }
            }
        }
    }
}

pub fn mesh_fill_damage_for_surfaces(
    document: &Document,
    mesh_id: MeshId,
    surfaces: &[PaintSurfaceId],
    scene_visibility: &ViewportSceneVisibility,
    guard_px: u32,
) -> DamageMap {
    let mut damage = DamageMap::default();
    if surfaces.is_empty() {
        return damage;
    }
    let mut saw_triangle = false;
    let mut missing = false;
    for (triangle_index, tri) in document.mesh.indices.iter().enumerate() {
        if document.mesh.triangle_mesh_ids.get(triangle_index).copied() != Some(mesh_id) {
            continue;
        }
        saw_triangle = true;
        let material_index = document.mesh.material_index_for_triangle(triangle_index);
        if !scene_visibility.geometry_visible(mesh_id, material_index.into()) {
            continue;
        }
        if material_index >= document.materials.len() {
            missing = true;
            continue;
        };
        let Some(surface) = surfaces
            .iter()
            .copied()
            .find(|surface| surface.material_index.as_usize() == material_index)
        else {
            missing = true;
            continue;
        };
        let Some(texture_size) = document.texture_size_for_surface(surface) else {
            missing = true;
            continue;
        };
        let (Some(&uv0), Some(&uv1), Some(&uv2)) = (
            document.mesh.uvs.get(tri[0] as usize),
            document.mesh.uvs.get(tri[1] as usize),
            document.mesh.uvs.get(tri[2] as usize),
        ) else {
            missing = true;
            continue;
        };
        let min = uv0.min(uv1).min(uv2);
        let max = uv0.max(uv1).max(uv2);
        if let Some(rect) = uv_rect_to_pixel_rect(texture_size, min, max, guard_px) {
            damage.add_rect(surface, rect);
        } else {
            missing = true;
        }
    }
    if missing {
        let mut fallback = DamageMap::default();
        for &surface in surfaces {
            if let Some(texture_size) = document.texture_size_for_surface(surface) {
                fallback.add_full_surface(surface, texture_size);
            }
        }
        return fallback;
    }
    if !saw_triangle {
        return DamageMap::default();
    }
    damage
}

pub fn uv_rect_to_pixel_rect(
    texture_size: [u32; 2],
    min_uv: Vec2,
    max_uv: Vec2,
    guard_px: u32,
) -> Option<RectU32> {
    let min = min_uv.min(max_uv);
    let max = min_uv.max(max_uv);
    pixel_bbox_to_rect(
        texture_size,
        min.x * texture_size[0] as f32,
        min.y * texture_size[1] as f32,
        max.x * texture_size[0] as f32,
        max.y * texture_size[1] as f32,
        guard_px,
    )
}

pub fn pixel_bbox_to_rect(
    texture_size: [u32; 2],
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
    guard_px: u32,
) -> Option<RectU32> {
    if !(min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite()) {
        return None;
    }
    let raw_min_x = min_x.min(max_x);
    let raw_min_y = min_y.min(max_y);
    let raw_max_x = min_x.max(max_x);
    let raw_max_y = min_y.max(max_y);
    if (raw_max_x - raw_min_x).abs() <= f32::EPSILON
        || (raw_max_y - raw_min_y).abs() <= f32::EPSILON
    {
        return None;
    }
    let epsilon = 1e-6;
    let base_min_x = (raw_min_x + epsilon).floor() as i64;
    let base_min_y = (raw_min_y + epsilon).floor() as i64;
    let base_max_x = (raw_max_x - epsilon).ceil() as i64;
    let base_max_y = (raw_max_y - epsilon).ceil() as i64;
    if base_min_x >= base_max_x || base_min_y >= base_max_y {
        return None;
    }
    let guard = guard_px as i64;
    let min_x = (base_min_x - guard).clamp(0, texture_size[0] as i64) as u32;
    let min_y = (base_min_y - guard).clamp(0, texture_size[1] as i64) as u32;
    let max_x = (base_max_x + guard).clamp(0, texture_size[0] as i64) as u32;
    let max_y = (base_max_y + guard).clamp(0, texture_size[1] as i64) as u32;
    if min_x >= max_x || min_y >= max_y {
        return None;
    }
    Some(RectU32 {
        origin: [min_x, min_y],
        size: [max_x - min_x, max_y - min_y],
    })
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec3};

    use crate::core::{
        brush_engine::SurfaceSourceMaterialScope,
        document::{Document, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh},
        stroke::{PaintSurfaceSet, SurfaceDab},
        stroke_preset::{PressureDynamics, StrokeOp},
        stroke_style::ResolvedStrokeStyle,
        surface::PaintSurfaceId,
        tool_operation::ToolOperation,
    };

    use super::estimate_surface_stroke_damage_from_dabs;

    fn test_document() -> Document {
        Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("M0", [1024, 1024]),
                MaterialSpec::new("M1", [1024, 1024]),
            ],
        )
    }

    fn material_boundary_document() -> Document {
        Document::new(
            MeshData::new(
                vec![
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(1.0, 1.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                ],
                vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(1.0, 0.0),
                    Vec2::new(1.0, 1.0),
                    Vec2::new(0.0, 1.0),
                ],
                vec![Vec3::Z; 4],
                vec![[0, 1, 2], [0, 2, 3]],
                vec![
                    SubMesh {
                        mesh_id: MeshId(0),
                        start_index: 0,
                        index_count: 3,
                        material_index: 0,
                        material_name: "M0".to_owned(),
                        wireframe_edges: Vec::new(),
                    },
                    SubMesh {
                        mesh_id: MeshId(0),
                        start_index: 3,
                        index_count: 3,
                        material_index: 1,
                        material_name: "M1".to_owned(),
                        wireframe_edges: Vec::new(),
                    },
                ],
                vec![MeshObject {
                    id: MeshId(0),
                    name: "Mesh".to_owned(),
                }],
                vec![MeshId(0), MeshId(0)],
            )
            .unwrap(),
            vec![
                MaterialSpec::new("M0", [1024, 1024]),
                MaterialSpec::new("M1", [1024, 1024]),
            ],
        )
    }

    fn test_style(radius_world: f32) -> ResolvedStrokeStyle {
        let stroke_op = StrokeOp::BrushEngine {
            engine_id: "test".to_owned(),
            radius_world,
            radius_pressure: PressureDynamics::default(),
            params: Vec::new(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: SurfaceSourceMaterialScope::AllMaterials,
        };
        let operation = ToolOperation::from_stroke_op_and_color(&stroke_op, [1.0; 3]);
        ResolvedStrokeStyle {
            stroke_op,
            operation,
        }
    }

    fn test_dab(material_index: usize, uv: Vec2) -> SurfaceDab {
        let mut dab =
            SurfaceDab::with_scales(Vec2::ZERO, Vec3::ZERO, Vec3::Z, material_index, 1.0, 1.0);
        dab.uv = Some(uv);
        dab.uv_edge_distance = Some(f32::INFINITY);
        dab.uv_paint_boundary_distance = Some(f32::INFINITY);
        dab
    }

    fn test_boundary_dab(material_index: usize, uv: Vec2, triangle_index: usize) -> SurfaceDab {
        let mut dab = test_dab(material_index, uv);
        dab.uv_edge_distance = Some(0.01);
        dab.uv_paint_boundary_distance = Some(0.01);
        dab.triangle_index = Some(triangle_index);
        dab.mesh_id = Some(MeshId(0));
        dab
    }

    #[test]
    fn surface_stroke_damage_uses_target_materials_by_dab_to_skip_irrelevant_materials() {
        let document = test_document();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let surface0 = PaintSurfaceId::raster(0.into(), layer);
        let surface1 = PaintSurfaceId::raster(1.into(), layer);
        let surfaces = PaintSurfaceSet::from_vec(vec![surface0, surface1]);
        let dabs = [
            test_dab(0, Vec2::new(0.25, 0.25)),
            test_dab(1, Vec2::new(0.75, 0.75)),
        ];
        let target_materials_by_dab = vec![vec![0], vec![1]];

        let damage = estimate_surface_stroke_damage_from_dabs(
            &document,
            &surfaces,
            &target_materials_by_dab,
            &test_style(0.01),
            &dabs,
        )
        .expect("per-material surface dabs should produce explicit damage");

        assert_eq!(damage.pixels.len(), 2);
        assert!(damage.pixels.iter().any(|pixel| {
            pixel.surface == surface0
                && pixel.rect.origin != [0, 0]
                && pixel.rect.size[0] < 1024
                && pixel.rect.size[1] < 1024
        }));
        assert!(damage.pixels.iter().any(|pixel| {
            pixel.surface == surface1
                && pixel.rect.origin != [0, 0]
                && pixel.rect.size[0] < 1024
                && pixel.rect.size[1] < 1024
        }));
    }

    #[test]
    fn surface_stroke_damage_keeps_separated_dabs_as_multiple_rects() {
        let document = test_document();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let surface = PaintSurfaceId::raster(0.into(), layer);
        let surfaces = PaintSurfaceSet::from_vec(vec![surface]);
        let dabs = [
            test_dab(0, Vec2::new(0.12, 0.12)),
            test_dab(0, Vec2::new(0.88, 0.88)),
        ];
        let target_materials_by_dab = vec![vec![0], vec![0]];

        let damage = estimate_surface_stroke_damage_from_dabs(
            &document,
            &surfaces,
            &target_materials_by_dab,
            &test_style(0.005),
            &dabs,
        )
        .expect("far surface dabs should keep explicit damage");

        let rects: Vec<_> = damage
            .pixels
            .iter()
            .filter(|pixel| pixel.surface == surface)
            .map(|pixel| pixel.rect)
            .collect();
        assert_eq!(rects.len(), 2);
        assert!(
            rects
                .iter()
                .all(|rect| rect.size[0] < 256 && rect.size[1] < 256)
        );
    }

    #[test]
    fn surface_stroke_damage_adds_neighbor_rect_at_material_boundary() {
        let document = material_boundary_document();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let surface0 = PaintSurfaceId::raster(0.into(), layer);
        let surface1 = PaintSurfaceId::raster(1.into(), layer);
        let surfaces = PaintSurfaceSet::from_vec(vec![surface0, surface1]);
        let dabs = [test_boundary_dab(0, Vec2::new(0.50, 0.49), 0)];
        let target_materials_by_dab = vec![vec![0, 1]];

        let damage = estimate_surface_stroke_damage_from_dabs(
            &document,
            &surfaces,
            &target_materials_by_dab,
            &test_style(0.01),
            &dabs,
        )
        .expect("boundary-crossing dab should produce explicit damage");

        assert!(damage.pixels.iter().any(|pixel| {
            pixel.surface == surface0
                && pixel.rect.origin != [0, 0]
                && pixel.rect.size
                    != document.materials[pixel.surface.material_index().as_usize()].texture_size
        }));
        assert!(damage.pixels.iter().any(|pixel| {
            pixel.surface == surface1
                && pixel.rect.origin != [0, 0]
                && pixel.rect.size
                    != document.materials[pixel.surface.material_index().as_usize()].texture_size
        }));
    }
}

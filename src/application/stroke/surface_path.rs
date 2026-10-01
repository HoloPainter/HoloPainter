use glam::{Vec2, Vec4};

use crate::{
    application::ViewportInputContext,
    core::{
        brush_engine::{ParamDynamicsBinding, ParamValue, SurfaceSourceMaterialScope},
        document::{Document, RaycastScratch, SurfaceHit},
        math::ray_from_viewport_px,
        stroke::{StrokeDab, SurfaceDab},
        stroke_preset::{StrokeStrategy, StrokeToolPreset},
        viewport_visibility::ViewportSceneVisibility,
    },
};

const SURFACE_NORMAL_CONTINUITY_THRESHOLD: f32 = 0.25;
const SURFACE_MIN_CANDIDATE_STEP_PX: f32 = 1.0;
const SURFACE_MAX_CANDIDATE_STEP_PX: f32 = 16.0;
const SURFACE_CANDIDATE_STEP_SPACING_RATIO: f32 = 0.5;
const SURFACE_ADAPTIVE_MIN_PROBE_STEP_PX: f32 = 1.0;
const SURFACE_ADAPTIVE_MAX_PROBE_STEP_PX: f32 = 64.0;
const SURFACE_SPACING_ROOT_FIND_ITERATIONS: usize = 4;
const SURFACE_RATIO_EPSILON: f32 = 1e-5;
const FOOTPRINT_INNER_SAMPLE_RADIUS: f32 = 0.5;
const FOOTPRINT_INNER_SAMPLE_COUNT: usize = 8;
const FOOTPRINT_OUTER_SAMPLE_COUNT: usize = 16;
const FOOTPRINT_COARSE_SAMPLE_COUNT: usize = 8;
const FOOTPRINT_BOUNDARY_NORMAL_DOT_THRESHOLD: f32 = 0.85;
const FOOTPRINT_BOUNDARY_DEPTH_EPSILON_RATIO: f32 = 0.02;
const FOOTPRINT_UNIT_INNER: [[f32; 2]; FOOTPRINT_INNER_SAMPLE_COUNT] = [
    [1.0, 0.0],
    [0.70710677, 0.70710677],
    [0.0, 1.0],
    [-0.70710677, 0.70710677],
    [-1.0, 0.0],
    [-0.70710677, -0.70710677],
    [0.0, -1.0],
    [0.70710677, -0.70710677],
];
const FOOTPRINT_UNIT_OUTER: [[f32; 2]; FOOTPRINT_OUTER_SAMPLE_COUNT] = [
    [1.0, 0.0],
    [0.9238795, 0.38268343],
    [0.70710677, 0.70710677],
    [0.38268343, 0.9238795],
    [0.0, 1.0],
    [-0.38268343, 0.9238795],
    [-0.70710677, 0.70710677],
    [-0.9238795, 0.38268343],
    [-1.0, 0.0],
    [-0.9238795, -0.38268343],
    [-0.70710677, -0.70710677],
    [-0.38268343, -0.9238795],
    [0.0, -1.0],
    [0.38268343, -0.9238795],
    [0.70710677, -0.70710677],
    [0.9238795, -0.38268343],
];
const FOOTPRINT_UNIT_COARSE: [[f32; 2]; FOOTPRINT_COARSE_SAMPLE_COUNT] = [
    [1.0, 0.0],
    [0.70710677, 0.70710677],
    [0.0, 1.0],
    [-0.70710677, 0.70710677],
    [-1.0, 0.0],
    [-0.70710677, -0.70710677],
    [0.0, -1.0],
    [0.70710677, -0.70710677],
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::application) struct VisibleSurfaceMaterialIndices {
    pub target: Vec<usize>,
    pub source: Option<Vec<usize>>,
    pub target_by_dab: Vec<Vec<usize>>,
}

#[cfg(test)]
use crate::application::PointerSampleKind;

#[derive(Debug, Clone, Copy, PartialEq)]
struct SurfacePathSample {
    ratio: f32,
    dab: SurfaceDab,
}

struct SurfaceSampleContext<'a> {
    document: Option<&'a Document>,
    visibility: &'a ViewportSceneVisibility,
    position_px: Vec2,
    view: ViewportInputContext,
    inv_view_proj: glam::Mat4,
    initial_hit: Option<SurfaceHit>,
    raycast_scratch: &'a mut RaycastScratch,
}

pub(in crate::application) fn sample_surface_segment_into(
    document: Option<&Document>,
    visibility: &ViewportSceneVisibility,
    position_px: Vec2,
    view: ViewportInputContext,
    hit: Option<SurfaceHit>,
    from: Option<SurfaceDab>,
    to: StrokeDab,
    strategy: &StrokeStrategy,
    radius_world: f32,
    out: &mut Vec<SurfaceDab>,
    raycast_scratch: &mut RaycastScratch,
) {
    let mut ctx = SurfaceSampleContext {
        document,
        visibility,
        position_px,
        view,
        inv_view_proj: view.inv_view_proj,
        initial_hit: hit,
        raycast_scratch,
    };
    if matches!(strategy, StrokeStrategy::RawEvent) {
        out.extend(surface_dab_at_screen(&mut ctx, to, None));
        return;
    }

    let spacing_ratio = match strategy {
        StrokeStrategy::SpacingDab { spacing } | StrokeStrategy::ContinuousDab { spacing, .. } => {
            *spacing
        }
        StrokeStrategy::RawEvent => unreachable!("handled above"),
    };
    let Some(from) = from else {
        out.extend(surface_dab_at_screen(&mut ctx, to, None));
        return;
    };

    let screen_delta = to.position - from.screen_px;
    let screen_distance = screen_delta.length();
    if screen_distance <= f32::EPSILON {
        return;
    }

    if should_use_adaptive_surface_spacing(view, from, radius_world, spacing_ratio, screen_distance)
    {
        sample_surface_segment_adaptive_into(
            &mut ctx,
            from,
            to,
            radius_world,
            spacing_ratio,
            screen_delta,
            screen_distance,
            out,
        );
    } else {
        sample_surface_segment_candidates_into(
            &mut ctx,
            from,
            to,
            radius_world,
            spacing_ratio,
            screen_delta,
            screen_distance,
            out,
        );
    }
}

fn sample_surface_segment_candidates_into(
    ctx: &mut SurfaceSampleContext<'_>,
    from: SurfaceDab,
    to: StrokeDab,
    radius_world: f32,
    spacing_ratio: f32,
    screen_delta: Vec2,
    screen_distance: f32,
    out: &mut Vec<SurfaceDab>,
) {
    let mut last_accepted = from;
    let mut continuity_broken = false;
    let mut candidate_distance_px = 0.0;
    while candidate_distance_px < screen_distance {
        let step_px =
            surface_candidate_step_px(ctx.view, last_accepted, radius_world, spacing_ratio);
        candidate_distance_px = (candidate_distance_px + step_px).min(screen_distance);
        let ratio = candidate_distance_px / screen_distance;
        let Some(sample) = sample_surface_at_ratio(
            ctx,
            from,
            to,
            screen_delta,
            ratio,
            last_accepted.triangle_index,
        ) else {
            continuity_broken = true;
            continue;
        };
        if !surface_samples_are_continuous(last_accepted, sample.dab) || continuity_broken {
            last_accepted = sample.dab;
            continuity_broken = false;
            out.push(sample.dab);
            continue;
        }
        if !surface_distance_reaches_spacing(last_accepted, sample.dab, radius_world, spacing_ratio)
        {
            continue;
        }
        last_accepted = sample.dab;
        out.push(sample.dab);
    }
}

fn sample_surface_segment_adaptive_into(
    ctx: &mut SurfaceSampleContext<'_>,
    from: SurfaceDab,
    to: StrokeDab,
    radius_world: f32,
    spacing_ratio: f32,
    screen_delta: Vec2,
    screen_distance: f32,
    out: &mut Vec<SurfaceDab>,
) {
    let mut anchor = SurfacePathSample {
        ratio: 0.0,
        dab: from,
    };
    let mut low = anchor;
    let mut probe_ratio = 0.0;
    let mut continuity_broken = false;

    while probe_ratio < 1.0 - SURFACE_RATIO_EPSILON {
        let step_px =
            surface_adaptive_probe_step_px(ctx.view, anchor.dab, radius_world, spacing_ratio);
        let next_distance = (probe_ratio * screen_distance + step_px).min(screen_distance);
        let next_ratio = (next_distance / screen_distance).clamp(probe_ratio, 1.0);
        if next_ratio <= probe_ratio + SURFACE_RATIO_EPSILON {
            break;
        }

        let preferred_triangle = anchor.dab.triangle_index;
        let Some(high) =
            sample_surface_at_ratio(ctx, from, to, screen_delta, next_ratio, preferred_triangle)
        else {
            continuity_broken = true;
            probe_ratio = next_ratio;
            continue;
        };

        if continuity_broken {
            let recovered = recover_surface_hit_in_range(
                ctx,
                from,
                to,
                screen_delta,
                probe_ratio,
                high.ratio,
                screen_distance,
                preferred_triangle,
            )
            .unwrap_or(high);
            out.push(recovered.dab);
            anchor = recovered;
            low = anchor;
            probe_ratio = anchor.ratio;
            continuity_broken = false;
            continue;
        }

        if !surface_samples_are_continuous(anchor.dab, high.dab) {
            let recovered = recover_surface_discontinuity_in_range(
                ctx,
                from,
                to,
                screen_delta,
                probe_ratio,
                high.ratio,
                screen_distance,
                anchor.dab,
                preferred_triangle,
            )
            .unwrap_or(high);
            out.push(recovered.dab);
            anchor = recovered;
            low = anchor;
            probe_ratio = anchor.ratio;
            continue;
        }

        if !surface_distance_reaches_spacing(anchor.dab, high.dab, radius_world, spacing_ratio) {
            low = high;
            probe_ratio = high.ratio;
            continue;
        }

        let refined = refine_surface_spacing_root(
            ctx,
            from,
            to,
            screen_delta,
            anchor,
            low,
            high,
            radius_world,
            spacing_ratio,
            preferred_triangle,
        );
        if refined.ratio <= anchor.ratio + SURFACE_RATIO_EPSILON {
            probe_ratio = high.ratio;
            low = high;
            continue;
        }
        out.push(refined.dab);
        anchor = refined;
        low = anchor;
        probe_ratio = anchor.ratio;
    }
}

fn stroke_dab_at_ratio(
    from: SurfaceDab,
    to: StrokeDab,
    screen_delta: Vec2,
    ratio: f32,
) -> StrokeDab {
    StrokeDab::with_scales(
        from.screen_px + screen_delta * ratio,
        from.pressure + (to.pressure - from.pressure) * ratio,
        from.radius_scale + (to.radius_scale - from.radius_scale) * ratio,
    )
}

fn sample_surface_at_ratio(
    ctx: &mut SurfaceSampleContext<'_>,
    from: SurfaceDab,
    to: StrokeDab,
    screen_delta: Vec2,
    ratio: f32,
    preferred_triangle: Option<usize>,
) -> Option<SurfacePathSample> {
    let ratio = ratio.clamp(0.0, 1.0);
    let dab = stroke_dab_at_ratio(from, to, screen_delta, ratio);
    surface_dab_at_screen(ctx, dab, preferred_triangle).map(|dab| SurfacePathSample { ratio, dab })
}

fn refine_surface_spacing_root(
    ctx: &mut SurfaceSampleContext<'_>,
    from: SurfaceDab,
    to: StrokeDab,
    screen_delta: Vec2,
    anchor: SurfacePathSample,
    mut low: SurfacePathSample,
    mut high: SurfacePathSample,
    radius_world: f32,
    spacing_ratio: f32,
    preferred_triangle: Option<usize>,
) -> SurfacePathSample {
    for _ in 0..SURFACE_SPACING_ROOT_FIND_ITERATIONS {
        let mid_ratio = (low.ratio + high.ratio) * 0.5;
        if mid_ratio <= low.ratio + SURFACE_RATIO_EPSILON
            || mid_ratio >= high.ratio - SURFACE_RATIO_EPSILON
        {
            break;
        }
        let Some(mid) =
            sample_surface_at_ratio(ctx, from, to, screen_delta, mid_ratio, preferred_triangle)
        else {
            break;
        };
        if !surface_samples_are_continuous(anchor.dab, mid.dab) {
            return mid;
        }
        if surface_distance_reaches_spacing(anchor.dab, mid.dab, radius_world, spacing_ratio) {
            high = mid;
        } else {
            low = mid;
        }
    }
    high
}

fn recover_surface_hit_in_range(
    ctx: &mut SurfaceSampleContext<'_>,
    from: SurfaceDab,
    to: StrokeDab,
    screen_delta: Vec2,
    start_ratio: f32,
    end_ratio: f32,
    screen_distance: f32,
    preferred_triangle: Option<usize>,
) -> Option<SurfacePathSample> {
    scan_surface_range(
        ctx,
        from,
        to,
        screen_delta,
        start_ratio,
        end_ratio,
        screen_distance,
        preferred_triangle,
    )
    .into_iter()
    .next()
}

fn recover_surface_discontinuity_in_range(
    ctx: &mut SurfaceSampleContext<'_>,
    from: SurfaceDab,
    to: StrokeDab,
    screen_delta: Vec2,
    start_ratio: f32,
    end_ratio: f32,
    screen_distance: f32,
    anchor: SurfaceDab,
    preferred_triangle: Option<usize>,
) -> Option<SurfacePathSample> {
    scan_surface_range(
        ctx,
        from,
        to,
        screen_delta,
        start_ratio,
        end_ratio,
        screen_distance,
        preferred_triangle,
    )
    .into_iter()
    .find(|sample| !surface_samples_are_continuous(anchor, sample.dab))
}

fn scan_surface_range(
    ctx: &mut SurfaceSampleContext<'_>,
    from: SurfaceDab,
    to: StrokeDab,
    screen_delta: Vec2,
    start_ratio: f32,
    end_ratio: f32,
    screen_distance: f32,
    preferred_triangle: Option<usize>,
) -> Vec<SurfacePathSample> {
    if end_ratio <= start_ratio + SURFACE_RATIO_EPSILON {
        return Vec::new();
    }
    let span_px = (end_ratio - start_ratio).abs() * screen_distance;
    let steps = (span_px / SURFACE_MIN_CANDIDATE_STEP_PX).ceil().max(1.0) as u32;
    let mut samples = Vec::new();
    for step in 1..=steps {
        let ratio = start_ratio + (end_ratio - start_ratio) * step as f32 / steps as f32;
        if let Some(sample) =
            sample_surface_at_ratio(ctx, from, to, screen_delta, ratio, preferred_triangle)
        {
            samples.push(sample);
        }
    }
    samples
}

fn surface_samples_are_continuous(a: SurfaceDab, b: SurfaceDab) -> bool {
    a.world_normal.dot(b.world_normal) > SURFACE_NORMAL_CONTINUITY_THRESHOLD
}

fn surface_distance_reaches_spacing(
    anchor: SurfaceDab,
    sample: SurfaceDab,
    radius_world: f32,
    spacing_ratio: f32,
) -> bool {
    let effective_radius = radius_world * ((anchor.radius_scale + sample.radius_scale) * 0.5);
    let target_spacing = (effective_radius * spacing_ratio).max(1e-6);
    (sample.world_pos - anchor.world_pos).length() >= target_spacing
}

fn should_use_adaptive_surface_spacing(
    view: ViewportInputContext,
    from: SurfaceDab,
    radius_world: f32,
    spacing_ratio: f32,
    screen_distance: f32,
) -> bool {
    let projected_spacing_px = surface_dab_radius_px(view, from, radius_world) * spacing_ratio;
    screen_distance > SURFACE_MAX_CANDIDATE_STEP_PX
        && projected_spacing_px.is_finite()
        && projected_spacing_px > SURFACE_MAX_CANDIDATE_STEP_PX
}

fn surface_adaptive_probe_step_px(
    view: ViewportInputContext,
    anchor: SurfaceDab,
    radius_world: f32,
    spacing_ratio: f32,
) -> f32 {
    let projected_spacing_px = surface_dab_radius_px(view, anchor, radius_world) * spacing_ratio;
    if !projected_spacing_px.is_finite() {
        return SURFACE_ADAPTIVE_MIN_PROBE_STEP_PX;
    }
    projected_spacing_px.clamp(
        SURFACE_ADAPTIVE_MIN_PROBE_STEP_PX,
        SURFACE_ADAPTIVE_MAX_PROBE_STEP_PX,
    )
}

pub(in crate::application) fn surface_dab_from_hit(
    position_px: Vec2,
    pressure: f32,
    tool: &StrokeToolPreset,
    hit: Option<SurfaceHit>,
) -> Option<SurfaceDab> {
    let hit = hit?;
    let dab = super::sampling::stroke_dab_from_pressure(position_px, pressure, tool);
    Some(SurfaceDab::from_hit_with_scales(
        position_px,
        hit,
        dab.pressure,
        dab.radius_scale,
    ))
}

pub(in crate::application) fn visible_material_indices_for_surface_dabs(
    document: Option<&Document>,
    visibility: &ViewportSceneVisibility,
    view: ViewportInputContext,
    dabs: &[SurfaceDab],
    radius_world: f32,
    raycast_scratch: &mut RaycastScratch,
) -> Vec<usize> {
    visible_surface_material_indices_for_surface_dabs(
        document,
        visibility,
        view,
        dabs,
        radius_world,
        &SurfaceSourceMaterialScope::AllMaterials,
        &[],
        &[],
        raycast_scratch,
    )
    .target
}

pub(in crate::application) fn visible_surface_material_indices_for_surface_dabs(
    document: Option<&Document>,
    visibility: &ViewportSceneVisibility,
    view: ViewportInputContext,
    dabs: &[SurfaceDab],
    radius_world: f32,
    source_scope: &SurfaceSourceMaterialScope,
    params: &[(String, ParamValue)],
    param_dynamics: &[ParamDynamicsBinding],
    raycast_scratch: &mut RaycastScratch,
) -> VisibleSurfaceMaterialIndices {
    let Some(document) = document else {
        return VisibleSurfaceMaterialIndices {
            target: Vec::new(),
            source: matches!(
                source_scope,
                SurfaceSourceMaterialScope::BrushFootprint { .. }
            )
            .then(Vec::new),
            target_by_dab: vec![Vec::new(); dabs.len()],
        };
    };

    let material_count = document.materials.len();
    if material_count == 1 && !dabs.is_empty() {
        let target_by_dab = dabs
            .iter()
            .map(|dab| {
                surface_dab_visible(*dab, visibility)
                    .then_some(vec![0])
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        let target = target_by_dab
            .iter()
            .any(|materials| !materials.is_empty())
            .then_some(vec![0])
            .unwrap_or_default();
        let source = matches!(
            source_scope,
            SurfaceSourceMaterialScope::BrushFootprint { .. }
        )
        .then(|| target.clone());
        return VisibleSurfaceMaterialIndices {
            target,
            source,
            target_by_dab,
        };
    }

    let mut target = Vec::new();
    let mut source = matches!(
        source_scope,
        SurfaceSourceMaterialScope::BrushFootprint { .. }
    )
    .then(Vec::new);
    let mut target_by_dab = Vec::with_capacity(dabs.len());
    let inv_view_proj = view.inv_view_proj;

    for dab in dabs {
        let mut dab_target = Vec::new();
        if surface_dab_visible(*dab, visibility) {
            push_unique_material(&mut dab_target, dab.material_index);
        }
        let base_radius_px = surface_dab_radius_px(view, *dab, radius_world);
        collect_visible_materials_for_radius(
            document,
            visibility,
            *dab,
            view.size,
            inv_view_proj,
            base_radius_px,
            material_count,
            &mut dab_target,
            raycast_scratch,
            true,
        );
        for &material_index in &dab_target {
            push_unique_material(&mut target, material_index);
        }
        target_by_dab.push(dab_target);

        if let Some(source_material_indices) = source.as_mut() {
            let extra_radius_px = source_scope
                .extra_radius_px(base_radius_px, dab.pressure, params, param_dynamics)
                .expect("brush footprint source scope should produce extra radius")
                .max(0.0);
            if extra_radius_px > f32::EPSILON {
                collect_visible_materials_for_radius(
                    document,
                    visibility,
                    *dab,
                    view.size,
                    inv_view_proj,
                    base_radius_px + extra_radius_px,
                    material_count,
                    source_material_indices,
                    raycast_scratch,
                    false,
                );
            }
        }
    }

    sort_dedup_materials(&mut target);
    for dab_target in &mut target_by_dab {
        sort_dedup_materials(dab_target);
    }
    if let Some(source_material_indices) = source.as_mut() {
        for &material_index in &target {
            push_unique_material(source_material_indices, material_index);
        }
        sort_dedup_materials(source_material_indices);
    }

    VisibleSurfaceMaterialIndices {
        target,
        source,
        target_by_dab,
    }
}

fn surface_candidate_step_px(
    view: ViewportInputContext,
    last_accepted: SurfaceDab,
    radius_world: f32,
    spacing_ratio: f32,
) -> f32 {
    let projected_spacing_px =
        surface_dab_radius_px(view, last_accepted, radius_world) * spacing_ratio;
    if !projected_spacing_px.is_finite() {
        return SURFACE_MIN_CANDIDATE_STEP_PX;
    }
    (projected_spacing_px * SURFACE_CANDIDATE_STEP_SPACING_RATIO)
        .clamp(SURFACE_MIN_CANDIDATE_STEP_PX, SURFACE_MAX_CANDIDATE_STEP_PX)
}

fn surface_dab_radius_px(view: ViewportInputContext, dab: SurfaceDab, radius_world: f32) -> f32 {
    let radius_world = radius_world.max(0.0) * dab.radius_scale.max(0.0);
    if radius_world <= f32::EPSILON {
        return 0.0;
    }
    let center = world_to_viewport_px(view.view_proj, dab.world_pos, view.size);
    let x_edge = world_to_viewport_px(
        view.view_proj,
        dab.world_pos + dab.tangent_x * radius_world,
        view.size,
    );
    let y_edge = world_to_viewport_px(
        view.view_proj,
        dab.world_pos + dab.tangent_y * radius_world,
        view.size,
    );
    match (center, x_edge, y_edge) {
        (Some(center), Some(x_edge), Some(y_edge)) => {
            (x_edge - center).length().max((y_edge - center).length())
        }
        _ => 0.0,
    }
}

fn world_to_viewport_px(
    view_proj: glam::Mat4,
    world: glam::Vec3,
    viewport_size: [u32; 2],
) -> Option<Vec2> {
    let clip = view_proj * Vec4::new(world.x, world.y, world.z, 1.0);
    if clip.w.abs() <= f32::EPSILON {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite() {
        return None;
    }
    Some(Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport_size[0].max(1) as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size[1].max(1) as f32,
    ))
}

fn footprint_inner_offsets(radius_px: f32) -> impl Iterator<Item = Vec2> {
    FOOTPRINT_UNIT_INNER.into_iter().map(move |offset| {
        Vec2::new(offset[0], offset[1]) * radius_px * FOOTPRINT_INNER_SAMPLE_RADIUS
    })
}

fn footprint_coarse_offsets(radius_px: f32) -> impl Iterator<Item = Vec2> {
    FOOTPRINT_UNIT_COARSE
        .into_iter()
        .map(move |offset| Vec2::new(offset[0], offset[1]) * radius_px)
}

fn footprint_dense_extra_offsets(radius_px: f32) -> impl Iterator<Item = Vec2> {
    let inner = footprint_inner_offsets(radius_px);
    let outer_between_coarse = FOOTPRINT_UNIT_OUTER
        .into_iter()
        .skip(1)
        .step_by(2)
        .map(move |offset| Vec2::new(offset[0], offset[1]) * radius_px);
    inner.chain(outer_between_coarse)
}

#[derive(Debug, Clone, Copy)]
struct FootprintReference {
    material_index: usize,
    mesh_id: Option<crate::core::document::MeshId>,
    world_normal: glam::Vec3,
    t: Option<f32>,
}

impl FootprintReference {
    fn from_dab(dab: SurfaceDab) -> Self {
        Self {
            material_index: dab.material_index,
            mesh_id: dab.mesh_id,
            world_normal: dab.world_normal,
            t: dab.ray_t,
        }
    }

    fn from_hit(hit: SurfaceHit) -> Self {
        Self {
            material_index: hit.material_index.as_usize(),
            mesh_id: Some(hit.mesh_id),
            world_normal: hit.world_normal,
            t: Some(hit.t),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum FootprintRaycastResult {
    OutsideViewport,
    Miss,
    Hit(SurfaceHit),
}

fn collect_visible_materials_for_radius(
    document: &Document,
    visibility: &ViewportSceneVisibility,
    dab: SurfaceDab,
    viewport_size: [u32; 2],
    inv_view_proj: glam::Mat4,
    radius_px: f32,
    material_count: usize,
    material_indices: &mut Vec<usize>,
    raycast_scratch: &mut RaycastScratch,
    include_center: bool,
) {
    let mut reference = FootprintReference::from_dab(dab);
    if (include_center || radius_px <= f32::EPSILON) && surface_dab_visible(dab, visibility) {
        push_unique_material(material_indices, dab.material_index);
        if reference.t.is_none() {
            if let FootprintRaycastResult::Hit(hit) = collect_visible_material_at_px(
                document,
                visibility,
                dab.screen_px,
                viewport_size,
                inv_view_proj,
                dab.triangle_index,
                material_indices,
                raycast_scratch,
            ) {
                reference = FootprintReference::from_hit(hit);
            }
        }
    }
    if material_indices.len() == material_count || radius_px <= f32::EPSILON {
        return;
    }

    let mut needs_dense_sampling = false;
    for offset in footprint_coarse_offsets(radius_px) {
        match collect_visible_material_at_px(
            document,
            visibility,
            dab.screen_px + offset,
            viewport_size,
            inv_view_proj,
            dab.triangle_index,
            material_indices,
            raycast_scratch,
        ) {
            FootprintRaycastResult::Hit(hit) => {
                needs_dense_sampling |= footprint_hit_needs_dense_sampling(reference, hit);
            }
            FootprintRaycastResult::Miss => {
                needs_dense_sampling = true;
            }
            FootprintRaycastResult::OutsideViewport => {}
        }
        if material_indices.len() == material_count {
            return;
        }
    }

    if !needs_dense_sampling {
        return;
    }

    for offset in footprint_dense_extra_offsets(radius_px) {
        collect_visible_material_at_px(
            document,
            visibility,
            dab.screen_px + offset,
            viewport_size,
            inv_view_proj,
            dab.triangle_index,
            material_indices,
            raycast_scratch,
        );
        if material_indices.len() == material_count {
            return;
        }
    }
}

fn footprint_hit_needs_dense_sampling(reference: FootprintReference, hit: SurfaceHit) -> bool {
    if hit.material_index.as_usize() != reference.material_index {
        return true;
    }
    if reference
        .mesh_id
        .is_some_and(|mesh_id| hit.mesh_id != mesh_id)
    {
        return true;
    }
    if reference.world_normal.dot(hit.world_normal) < FOOTPRINT_BOUNDARY_NORMAL_DOT_THRESHOLD {
        return true;
    }
    if let Some(reference_t) = reference.t {
        let depth_epsilon =
            reference_t.abs().max(hit.t.abs()).max(1.0) * FOOTPRINT_BOUNDARY_DEPTH_EPSILON_RATIO;
        if (hit.t - reference_t).abs() > depth_epsilon {
            return true;
        }
    }
    false
}

fn collect_visible_material_at_px(
    document: &Document,
    visibility: &ViewportSceneVisibility,
    px: Vec2,
    viewport_size: [u32; 2],
    inv_view_proj: glam::Mat4,
    preferred_triangle: Option<usize>,
    material_indices: &mut Vec<usize>,
    raycast_scratch: &mut RaycastScratch,
) -> FootprintRaycastResult {
    let result = visible_material_hit_at_px(
        document,
        visibility,
        px,
        viewport_size,
        inv_view_proj,
        preferred_triangle,
        raycast_scratch,
    );
    if let FootprintRaycastResult::Hit(hit) = result {
        push_unique_material(material_indices, hit.material_index.as_usize());
    }
    result
}

fn visible_material_hit_at_px(
    document: &Document,
    visibility: &ViewportSceneVisibility,
    px: Vec2,
    viewport_size: [u32; 2],
    inv_view_proj: glam::Mat4,
    preferred_triangle: Option<usize>,
    raycast_scratch: &mut RaycastScratch,
) -> FootprintRaycastResult {
    if !viewport_contains_px(px, viewport_size) {
        return FootprintRaycastResult::OutsideViewport;
    }
    let Some((ray_origin, ray_dir)) = ray_from_viewport_px(px, viewport_size, inv_view_proj) else {
        return FootprintRaycastResult::Miss;
    };
    let Some(hit) = document.raycast_visible_with_preferred_triangle(
        ray_origin,
        ray_dir,
        preferred_triangle,
        visibility,
        raycast_scratch,
    ) else {
        return FootprintRaycastResult::Miss;
    };
    FootprintRaycastResult::Hit(hit)
}

fn push_unique_material(material_indices: &mut Vec<usize>, material_index: usize) {
    if !material_indices.contains(&material_index) {
        material_indices.push(material_index);
    }
}

fn sort_dedup_materials(material_indices: &mut Vec<usize>) {
    material_indices.sort_unstable();
    material_indices.dedup();
}

fn viewport_contains_px(px: Vec2, viewport_size: [u32; 2]) -> bool {
    px.x >= 0.0
        && px.y >= 0.0
        && px.x < viewport_size[0].max(1) as f32
        && px.y < viewport_size[1].max(1) as f32
}

fn surface_dab_at_screen(
    ctx: &mut SurfaceSampleContext<'_>,
    dab: StrokeDab,
    preferred_triangle: Option<usize>,
) -> Option<SurfaceDab> {
    if (dab.position - ctx.position_px).length_squared() <= f32::EPSILON
        && let Some(hit) = ctx.initial_hit
        && ctx
            .visibility
            .geometry_visible(hit.mesh_id, hit.material_index.into())
    {
        return Some(SurfaceDab::from_hit_with_scales(
            dab.position,
            hit,
            dab.pressure,
            dab.radius_scale,
        ));
    }

    let document = ctx.document?;
    let (ray_origin, ray_dir) =
        ray_from_viewport_px(dab.position, ctx.view.size, ctx.inv_view_proj)?;
    let hit = document.raycast_visible_with_preferred_triangle(
        ray_origin,
        ray_dir,
        preferred_triangle,
        ctx.visibility,
        ctx.raycast_scratch,
    )?;
    Some(SurfaceDab::from_hit_with_scales(
        dab.position,
        hit,
        dab.pressure,
        dab.radius_scale,
    ))
}

fn surface_dab_visible(dab: SurfaceDab, visibility: &ViewportSceneVisibility) -> bool {
    visibility.material_visible(dab.material_index.into())
        && dab
            .mesh_id
            .is_none_or(|mesh_id| visibility.mesh_visible(mesh_id))
}

#[cfg(test)]
mod tests {
    use glam::{Mat4, Vec2, Vec3};

    use crate::core::document::{MaterialSpec, MeshData, MeshId, MeshObject, SubMesh};

    use super::*;

    #[test]
    fn surface_dab_from_hit_keeps_triangle_metadata() {
        let hit = SurfaceHit {
            world_pos: Vec3::new(1.0, 2.0, 3.0),
            world_normal: Vec3::Z,
            uv: Vec2::ZERO,
            uv_edge_distance: f32::INFINITY,
            uv_paint_boundary_distance: f32::INFINITY,
            triangle_index: 7,
            material_index: 3.into(),
            mesh_id: crate::core::document::MeshId(5),
            t: 1.0,
        };
        let dab = SurfaceDab::from_hit_with_scales(Vec2::new(4.0, 5.0), hit, 0.5, 0.25);

        assert_eq!(dab.triangle_index, Some(7));
        assert_eq!(dab.mesh_id, Some(crate::core::document::MeshId(5)));
        assert_eq!(dab.ray_t, Some(1.0));
        assert_eq!(dab.material_index, 3);
    }

    #[test]
    fn adaptive_surface_spacing_is_reserved_for_large_projected_spacing() {
        let view = ViewportInputContext {
            size: [800, 600],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, 2.0],
        };
        let dab = SurfaceDab::with_scales(Vec2::ZERO, Vec3::ZERO, Vec3::Z, 0, 1.0, 1.0);

        assert!(!should_use_adaptive_surface_spacing(
            view, dab, 0.001, 1.0, 128.0
        ));
        assert!(should_use_adaptive_surface_spacing(
            view, dab, 1.0, 1.0, 128.0
        ));
    }

    #[test]
    fn projected_surface_candidate_step_keeps_small_brushes_at_one_px() {
        let view = ViewportInputContext {
            size: [800, 600],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, 2.0],
        };
        let dab = SurfaceDab::with_scales(Vec2::ZERO, Vec3::ZERO, Vec3::Z, 0, 1.0, 1.0);

        let step_px = surface_candidate_step_px(view, dab, 0.001, 1.0);

        assert_eq!(step_px, SURFACE_MIN_CANDIDATE_STEP_PX);
    }

    #[test]
    fn projected_surface_candidate_step_is_capped_for_large_brushes() {
        let view = ViewportInputContext {
            size: [800, 600],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, 2.0],
        };
        let dab = SurfaceDab::with_scales(Vec2::ZERO, Vec3::ZERO, Vec3::Z, 0, 1.0, 1.0);

        let step_px = surface_candidate_step_px(view, dab, 1.0, 1.0);

        assert_eq!(step_px, SURFACE_MAX_CANDIDATE_STEP_PX);
    }

    fn two_material_diagonal_document() -> Document {
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

    fn material_test_dab(screen_px: Vec2, world_pos: Vec3, material_index: usize) -> SurfaceDab {
        SurfaceDab::with_scales(screen_px, world_pos, Vec3::Z, material_index, 1.0, 1.0)
    }

    #[test]
    fn finding_all_materials_does_not_assign_them_to_remaining_dabs() {
        let document = two_material_diagonal_document();
        let view = ViewportInputContext {
            size: [100, 100],
            view_proj: Mat4::IDENTITY,
            inv_view_proj: Mat4::IDENTITY,
            camera_world: [0.0, 0.0, 2.0],
        };
        let dabs = [
            material_test_dab(Vec2::new(75.0, 25.0), Vec3::new(0.5, 0.5, 0.0), 0),
            material_test_dab(Vec2::new(92.5, 42.5), Vec3::new(0.85, 0.15, 0.0), 0),
            material_test_dab(Vec2::new(57.5, 7.5), Vec3::new(0.15, 0.85, 0.0), 1),
        ];
        let mut scratch = RaycastScratch::default();

        let visible = visible_surface_material_indices_for_surface_dabs(
            Some(&document),
            &ViewportSceneVisibility::default(),
            view,
            &dabs,
            0.2,
            &SurfaceSourceMaterialScope::BrushFootprint {
                extra_radius: Vec::new(),
            },
            &[],
            &[],
            &mut scratch,
        );

        assert_eq!(visible.target, vec![0, 1]);
        assert_eq!(visible.target_by_dab[0], vec![0, 1]);
        assert_eq!(visible.target_by_dab[1], vec![0]);
        assert_eq!(visible.target_by_dab[2], vec![1]);
        assert_eq!(visible.target_by_dab.iter().map(Vec::len).sum::<usize>(), 4);
        assert_eq!(
            visible
                .target_by_dab
                .iter()
                .filter(|materials| materials.len() > 1)
                .count(),
            1
        );
    }
}

#[cfg(test)]
pub(in crate::application) fn sample_surface_segment(
    state: &crate::application::AppState,
    sample: &crate::application::PointerSample,
    from: Option<SurfaceDab>,
    to: StrokeDab,
    strategy: &StrokeStrategy,
    radius_world: f32,
) -> Vec<SurfaceDab> {
    let mut out = Vec::new();
    let PointerSampleKind::Surface { view, hit } = sample.kind else {
        return out;
    };
    let mut raycast_scratch = RaycastScratch::default();
    sample_surface_segment_into(
        state.document(),
        state.viewport_scene_visibility(),
        sample.screen_px(),
        view,
        hit,
        from,
        to,
        strategy,
        radius_world,
        &mut out,
        &mut raycast_scratch,
    );
    out
}

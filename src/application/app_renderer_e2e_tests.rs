use std::sync::Arc;

use eframe::egui_wgpu::wgpu;
use glam::{Mat4, Quat, Vec2, Vec3};

use crate::{
    application::{
        AppState, ApplicationRuntime, Command, InputHoldToken, InputModifiers, ToolInputEvent,
        TransformNumericEdit, UvViewInputContext, ViewPointerEvent, ViewPointerMeta,
        ViewPointerPhase, ViewportInputContext, fit_camera_to_document, load_rgba8_as_command_at,
        load_rgba8_as_paste_command_at,
        state::{DecalSession, ToolSession},
    },
    core::{
        adjustment::{
            Adjustment, AdjustmentKind, BrightnessContrastAdjustment, CurveChannel, CurvePoint,
            CurvesAdjustment, HueSaturationAdjustment, LevelsAdjustment, LevelsChannel,
        },
        brush_engine::ParamValue,
        camera::OrbitCamera,
        composite::{GroupCompositeMode, LayerBlendMode, SelectionCompositeMode},
        decal::{DecalImageAsset, DecalImageId, DecalTransform},
        document::{
            ImportedAsset, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh, SurfaceHit,
        },
        image::{MaterialPayload, Rgba8Snapshot},
        stroke::SurfaceProjectionId,
        stroke_preset::{StrokeInputFilter, StrokePresetOp, StrokeStrategy},
        surface::{LayerContent, LayerMaskInitMode, PaintSurfaceId},
        surface_filter::SpatialBlurOrientation,
        tool::ToolId,
        uv_view::UvViewTransform,
    },
    renderer::{ReadbackRequest, ReadbackResult, RenderEngine, RendererDiagnostic},
};

fn gpu_engine(texture_size: [u32; 2]) -> Option<RenderEngine> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
    Some(RenderEngine::new(device, queue, texture_size).expect("renderer should initialize"))
}

fn single_triangle_mesh() -> MeshData {
    scaled_single_triangle_mesh(1.0)
}

fn scaled_single_triangle_mesh(scale: f32) -> MeshData {
    MeshData::new(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(scale, 0.0, 0.0),
            Vec3::new(0.0, scale, 0.0),
        ],
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
        ],
        vec![Vec3::Z, Vec3::Z, Vec3::Z],
        vec![[0, 1, 2]],
        vec![SubMesh {
            mesh_id: MeshId(0),
            start_index: 0,
            index_count: 3,
            material_index: 0,
            material_name: "Material".to_owned(),
            wireframe_edges: vec![[0, 1], [1, 2], [2, 0]],
        }],
        vec![MeshObject {
            id: MeshId(0),
            name: "Triangle".to_owned(),
        }],
        vec![MeshId(0)],
    )
    .expect("test mesh should be valid")
}

fn uv_seam_pair_mesh() -> MeshData {
    let mesh_id = MeshId(0);
    MeshData::new(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        ],
        vec![
            Vec2::new(0.05, 0.05),
            Vec2::new(0.35, 0.05),
            Vec2::new(0.05, 0.95),
            Vec2::new(0.55, 0.05),
            Vec2::new(0.80, 0.95),
            Vec2::new(0.80, 0.05),
        ],
        vec![Vec3::Z; 6],
        vec![[0, 1, 2], [3, 4, 5]],
        vec![SubMesh {
            mesh_id,
            start_index: 0,
            index_count: 6,
            material_index: 0,
            material_name: "Material".to_owned(),
            wireframe_edges: Vec::new(),
        }],
        vec![MeshObject {
            id: mesh_id,
            name: "UvSeamPair".to_owned(),
        }],
        vec![mesh_id; 2],
    )
    .expect("UV seam test mesh should be valid")
}

fn folded_material_pair_mesh() -> MeshData {
    let mesh_id = MeshId(0);
    MeshData::new(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        ],
        vec![
            Vec2::new(0.05, 0.05),
            Vec2::new(0.95, 0.05),
            Vec2::new(0.05, 0.95),
            Vec2::new(0.05, 0.05),
            Vec2::new(0.95, 0.05),
            Vec2::new(0.95, 0.95),
        ],
        vec![Vec3::Z, Vec3::Z, Vec3::Z, Vec3::X, Vec3::X, Vec3::X],
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
            name: "FoldedMaterialPair".to_owned(),
        }],
        vec![mesh_id; 2],
    )
    .expect("folded material test mesh should be valid")
}

fn parallel_material_pair_mesh(mesh_ids: [MeshId; 2], reverse_second_normal: bool) -> MeshData {
    let mut positions = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ];
    let mut uvs = vec![Vec2::ZERO, Vec2::X, Vec2::Y];
    let second_normal = if reverse_second_normal {
        positions.extend([
            Vec3::new(0.0, 0.0, 0.25),
            Vec3::new(0.0, 1.0, 0.25),
            Vec3::new(1.0, 0.0, 0.25),
        ]);
        uvs.extend([Vec2::ZERO, Vec2::Y, Vec2::X]);
        -Vec3::Z
    } else {
        positions.extend([
            Vec3::new(0.0, 0.0, 0.25),
            Vec3::new(1.0, 0.0, 0.25),
            Vec3::new(0.0, 1.0, 0.25),
        ]);
        uvs.extend([Vec2::ZERO, Vec2::X, Vec2::Y]);
        Vec3::Z
    };
    let mut mesh_objects = vec![MeshObject {
        id: mesh_ids[0],
        name: "A".to_owned(),
    }];
    if mesh_ids[1] != mesh_ids[0] {
        mesh_objects.push(MeshObject {
            id: mesh_ids[1],
            name: "B".to_owned(),
        });
    }
    MeshData::new(
        positions,
        uvs,
        vec![
            Vec3::Z,
            Vec3::Z,
            Vec3::Z,
            second_normal,
            second_normal,
            second_normal,
        ],
        vec![[0, 1, 2], [3, 4, 5]],
        vec![
            SubMesh {
                mesh_id: mesh_ids[0],
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "A".to_owned(),
                wireframe_edges: Vec::new(),
            },
            SubMesh {
                mesh_id: mesh_ids[1],
                start_index: 3,
                index_count: 3,
                material_index: 1,
                material_name: "B".to_owned(),
                wireframe_edges: Vec::new(),
            },
        ],
        mesh_objects,
        vec![mesh_ids[0], mesh_ids[1]],
    )
    .expect("parallel material pair should be valid")
}

fn parallel_three_material_mesh() -> MeshData {
    let mesh_id = MeshId(0);
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    for z in [-0.1, 0.1, 0.0] {
        positions.extend([
            Vec3::new(0.0, 0.0, z),
            Vec3::new(1.0, 0.0, z),
            Vec3::new(0.0, 1.0, z),
        ]);
        uvs.extend([Vec2::ZERO, Vec2::X, Vec2::Y]);
        normals.extend([Vec3::Z; 3]);
    }
    MeshData::new(
        positions,
        uvs,
        normals,
        vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]],
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
            SubMesh {
                mesh_id,
                start_index: 6,
                index_count: 3,
                material_index: 2,
                material_name: "C".to_owned(),
                wireframe_edges: Vec::new(),
            },
        ],
        vec![MeshObject {
            id: mesh_id,
            name: "ParallelThree".to_owned(),
        }],
        vec![mesh_id; 3],
    )
    .expect("parallel three-material mesh should be valid")
}

fn symmetric_panel_mesh() -> MeshData {
    let positions = vec![
        Vec3::new(-0.9, -0.8, 0.0),
        Vec3::new(-0.1, -0.8, 0.0),
        Vec3::new(-0.9, 0.8, 0.0),
        Vec3::new(-0.1, 0.8, 0.0),
        Vec3::new(0.1, -0.8, 0.0),
        Vec3::new(0.9, -0.8, 0.0),
        Vec3::new(0.1, 0.8, 0.0),
        Vec3::new(0.9, 0.8, 0.0),
    ];
    MeshData::new(
        positions.clone(),
        vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(0.45, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(0.45, 1.0),
            Vec2::new(0.55, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.55, 1.0),
            Vec2::new(1.0, 1.0),
        ],
        vec![Vec3::Z; positions.len()],
        vec![[0, 1, 2], [1, 3, 2], [4, 5, 6], [5, 7, 6]],
        vec![SubMesh {
            mesh_id: MeshId(0),
            start_index: 0,
            index_count: 12,
            material_index: 0,
            material_name: "Material".to_owned(),
            wireframe_edges: Vec::new(),
        }],
        vec![MeshObject {
            id: MeshId(0),
            name: "SymmetricPanels".to_owned(),
        }],
        vec![MeshId(0); 4],
    )
    .expect("symmetric test mesh should be valid")
}

fn solid_rgba(texture_size: [u32; 2], rgba: [u8; 4]) -> Vec<u8> {
    let pixel_count = texture_size[0] as usize * texture_size[1] as usize;
    let mut pixels = Vec::with_capacity(pixel_count * 4);
    for _ in 0..pixel_count {
        pixels.extend_from_slice(&rgba);
    }
    pixels
}

fn vertical_stripes_rgba(texture_size: [u32; 2], even: [u8; 4], odd: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(texture_size[0] as usize * texture_size[1] as usize * 4);
    for _y in 0..texture_size[1] {
        for x in 0..texture_size[0] {
            let rgba = if x % 2 == 0 { even } else { odd };
            pixels.extend_from_slice(&rgba);
        }
    }
    pixels
}

fn load_runtime_with_pixels(
    engine: &mut RenderEngine,
    texture_size: [u32; 2],
    rgba8: Vec<u8>,
) -> (ApplicationRuntime, PaintSurfaceId) {
    let asset = ImportedAsset {
        mesh: single_triangle_mesh(),
        materials: vec![MaterialSpec::new("Material", texture_size)],
    };
    let mut runtime = ApplicationRuntime::new(AppState::default());
    runtime
        .dispatch(Command::AssetLoaded {
            asset,
            materials: vec![MaterialPayload {
                width: texture_size[0],
                height: texture_size[1],
                rgba8,
            }],
        })
        .expect("asset load command should be accepted");
    execute_runtime_effects(&mut runtime, engine);
    let surface = runtime
        .state
        .active_paint_surface()
        .expect("loaded document should have an active paint surface");
    (runtime, surface)
}

fn load_runtime_with_mesh_pixels(
    engine: &mut RenderEngine,
    mesh: MeshData,
    texture_size: [u32; 2],
    rgba8: Vec<u8>,
) -> (ApplicationRuntime, PaintSurfaceId) {
    let asset = ImportedAsset {
        mesh,
        materials: vec![MaterialSpec::new("Material", texture_size)],
    };
    let mut runtime = ApplicationRuntime::new(AppState::default());
    runtime
        .dispatch(Command::AssetLoaded {
            asset,
            materials: vec![MaterialPayload {
                width: texture_size[0],
                height: texture_size[1],
                rgba8,
            }],
        })
        .expect("asset load command should be accepted");
    execute_runtime_effects(&mut runtime, engine);
    let surface = runtime
        .state
        .active_paint_surface()
        .expect("loaded document should have an active paint surface");
    (runtime, surface)
}

fn load_runtime_with_material_pixels(
    engine: &mut RenderEngine,
    mesh: MeshData,
    materials: Vec<MaterialSpec>,
    rgba8_by_material: Vec<Vec<u8>>,
) -> (ApplicationRuntime, Vec<PaintSurfaceId>) {
    assert_eq!(materials.len(), rgba8_by_material.len());
    let material_sizes = materials
        .iter()
        .map(|material| material.texture_size)
        .collect::<Vec<_>>();
    let material_count = materials.len();
    let asset = ImportedAsset { mesh, materials };
    let mut runtime = ApplicationRuntime::new(AppState::default());
    runtime
        .dispatch(Command::AssetLoaded {
            asset,
            materials: rgba8_by_material
                .into_iter()
                .zip(material_sizes)
                .map(|(rgba8, texture_size)| MaterialPayload {
                    width: texture_size[0],
                    height: texture_size[1],
                    rgba8,
                })
                .collect(),
        })
        .expect("asset load command should be accepted");
    execute_runtime_effects(&mut runtime, engine);
    let layer_id = runtime
        .state
        .active_paint_surface()
        .expect("loaded document should have an active paint surface")
        .layer_id;
    let surfaces = (0..material_count)
        .map(|material_index| PaintSurfaceId::raster(material_index.into(), layer_id))
        .collect();
    (runtime, surfaces)
}

fn load_runtime(
    engine: &mut RenderEngine,
    texture_size: [u32; 2],
    rgba: [u8; 4],
) -> (ApplicationRuntime, PaintSurfaceId) {
    load_runtime_with_pixels(engine, texture_size, solid_rgba(texture_size, rgba))
}

fn select_only_material_b(runtime: &mut ApplicationRuntime, engine: &mut RenderEngine) {
    runtime.dispatch(Command::SetFocusedMaterial(1)).unwrap();
    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::ZERO,
            max_uv: Vec2::ONE,
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(runtime, engine)
        .expect("material B selection should produce selection commit artifacts");
}

fn execute_runtime_effects(
    runtime: &mut ApplicationRuntime,
    engine: &mut RenderEngine,
) -> Option<crate::renderer::RenderCommitArtifacts> {
    execute_runtime_effects_with_project_change(runtime, engine).map(|(artifacts, _)| artifacts)
}

fn execute_runtime_effects_with_project_change(
    runtime: &mut ApplicationRuntime,
    engine: &mut RenderEngine,
) -> Option<(crate::renderer::RenderCommitArtifacts, bool)> {
    execute_runtime_effects_with_plan_mutator_and_project_change(runtime, engine, |_| {})
}

fn execute_runtime_effects_with_plan_mutator(
    runtime: &mut ApplicationRuntime,
    engine: &mut RenderEngine,
    mutate: impl FnOnce(&mut crate::renderer::RendererFramePlan),
) -> Option<crate::renderer::RenderCommitArtifacts> {
    execute_runtime_effects_with_plan_mutator_and_project_change(runtime, engine, mutate)
        .map(|(artifacts, _)| artifacts)
}

fn execute_runtime_effects_with_plan_mutator_and_project_change(
    runtime: &mut ApplicationRuntime,
    engine: &mut RenderEngine,
    mutate: impl FnOnce(&mut crate::renderer::RendererFramePlan),
) -> Option<(crate::renderer::RenderCommitArtifacts, bool)> {
    let mut submission = runtime.drain_renderer_submission_work(Vec::new());
    mutate(&mut submission.renderer_plan);
    let execution = engine.execute(submission.renderer_plan);
    assert!(
        execution.is_ok(),
        "renderer batch failed: {:?}",
        execution.error_message()
    );
    let started_commit = execution
        .started_commit
        .expect("successful render batch should report commit readback status");
    runtime
        .accept_renderer_submission(submission.submission, execution.report, Ok(started_commit))
        .expect("runtime should stage render finalization for the batch");

    if runtime.has_pending_render_finalizations() {
        let completed = poll_one_completed_commit(engine);
        let commit_id = completed.id;
        let (artifacts, project_changed) = runtime
            .finalize_completed_render_commit(completed)
            .expect("runtime should finalize renderer commit artifacts");
        engine
            .accept_commit(commit_id, &artifacts)
            .expect("renderer surface shadow should accept finalized artifacts");
        Some((artifacts, project_changed))
    } else {
        drain_unstaged_renderer_commits(engine);
        None
    }
}

fn reverse_surface_projection_batches(plan: &mut crate::renderer::RendererFramePlan) {
    for command in &mut plan.edit_commands {
        let crate::renderer::EditCommand::Stroke(crate::renderer::StrokeCommand::AddDabs {
            dabs:
                crate::renderer::StrokeDabPayload::Surface {
                    projection_batches, ..
                },
            ..
        }) = command
        else {
            continue;
        };
        for logical_batch in projection_batches.chunks_mut(2) {
            if logical_batch.len() == 2
                && logical_batch[0].projection_id != logical_batch[1].projection_id
            {
                logical_batch.swap(0, 1);
            }
        }
    }
}

fn retain_surface_projection_batches(
    plan: &mut crate::renderer::RendererFramePlan,
    projection_id: SurfaceProjectionId,
) {
    for command in &mut plan.edit_commands {
        let crate::renderer::EditCommand::Stroke(crate::renderer::StrokeCommand::AddDabs {
            dabs:
                crate::renderer::StrokeDabPayload::Surface {
                    projection_batches, ..
                },
            ..
        }) = command
        else {
            continue;
        };
        projection_batches.retain(|batch| batch.projection_id == projection_id);
    }
}

fn poll_one_completed_commit(engine: &mut RenderEngine) -> crate::renderer::CompletedRenderCommit {
    for _ in 0..100 {
        let mut completed = engine.wait_completed_commits_for_test();
        if !completed.is_empty() {
            assert_eq!(
                completed.len(),
                1,
                "E2E helper expects one renderer commit at a time"
            );
            return completed.remove(0);
        }
        std::thread::yield_now();
    }
    panic!("renderer commit readback did not complete");
}

fn drain_unstaged_renderer_commits(engine: &mut RenderEngine) {
    for _ in 0..100 {
        let completed = engine.poll_completed_commits();
        if completed.is_empty() {
            return;
        }
        for commit in completed {
            assert!(
                commit.artifacts.is_ok(),
                "unstaged renderer commit failed: {:?}",
                commit.artifacts
            );
        }
    }
    panic!("renderer kept producing unstaged commits");
}

fn read_surface(engine: &mut RenderEngine, surface: PaintSurfaceId) -> Rgba8Snapshot {
    match engine
        .readback(ReadbackRequest::SurfaceRgba8 {
            surface,
            rect: None,
        })
        .expect("surface readback should succeed")
    {
        ReadbackResult::Rgba8(snapshot) => snapshot,
        other => panic!("unexpected readback result: {other:?}"),
    }
}

fn read_composite(engine: &mut RenderEngine) -> Rgba8Snapshot {
    match engine
        .readback(ReadbackRequest::CompositeRgba8 {
            material_index: 0,
            rect: None,
        })
        .expect("composite readback should succeed")
    {
        ReadbackResult::Rgba8(snapshot) => snapshot,
        other => panic!("unexpected readback result: {other:?}"),
    }
}

#[test]
fn startup_cube_uploads_white_composite_and_transparent_paint_surface_without_history() {
    let Some(mut engine) = gpu_engine([1024; 2]) else {
        eprintln!("Skipping startup Cube GPU test: no GPU adapter available");
        return;
    };
    let project = super::create_startup_project(4096).unwrap();
    let paint_surface = project.surfaces[1].surface;
    let mut runtime = ApplicationRuntime::new(AppState::with_status("ready"));
    runtime.dispatch(Command::ProjectLoaded(project)).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(
        read_surface(&mut engine, paint_surface)
            .rgba8
            .iter()
            .all(|value| *value == 0)
    );
    assert!(
        read_composite(&mut engine)
            .rgba8
            .iter()
            .all(|value| *value == 255)
    );
    assert!(!runtime.state.can_undo());
    assert!(!runtime.has_pending_history_transaction());
}

fn document_surface(runtime: &ApplicationRuntime, surface: PaintSurfaceId) -> Rgba8Snapshot {
    let image = runtime
        .state
        .document()
        .expect("document should be loaded")
        .tiles
        .read_surface_full(surface)
        .expect("document surface should be readable");
    Rgba8Snapshot::new(image.texture_size, [0, 0], image.texture_size, image.rgba8)
        .expect("document image should form a valid snapshot")
}

fn pixel_at(snapshot: &Rgba8Snapshot, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * snapshot.texture_size[0] + x) as usize) * 4;
    snapshot.rgba8[index..index + 4].try_into().unwrap()
}

fn center_pixel(snapshot: &Rgba8Snapshot) -> [u8; 4] {
    pixel_at(
        snapshot,
        snapshot.texture_size[0] / 2,
        snapshot.texture_size[1] / 2,
    )
}

fn uv_triangle_pixels(texture_size: [u32; 2], triangle: [Vec2; 3], rgba: [u8; 4]) -> Vec<u8> {
    let mut pixels = solid_rgba(texture_size, [0, 0, 0, 0]);
    for y in 0..texture_size[1] {
        for x in 0..texture_size[0] {
            let uv = Vec2::new(
                (x as f32 + 0.5) / texture_size[0] as f32,
                (y as f32 + 0.5) / texture_size[1] as f32,
            );
            if uv_point_in_triangle(uv, triangle) {
                let index = ((y * texture_size[0] + x) as usize) * 4;
                pixels[index..index + 4].copy_from_slice(&rgba);
            }
        }
    }
    pixels
}

fn uv_point_in_triangle(point: Vec2, triangle: [Vec2; 3]) -> bool {
    let edge0 = triangle[1] - triangle[0];
    let edge1 = triangle[2] - triangle[0];
    let delta = point - triangle[0];
    let determinant = edge0.x * edge1.y - edge0.y * edge1.x;
    if determinant.abs() <= f32::EPSILON {
        return false;
    }
    let b1 = (delta.x * edge1.y - delta.y * edge1.x) / determinant;
    let b2 = (edge0.x * delta.y - edge0.y * delta.x) / determinant;
    let b0 = 1.0 - b1 - b2;
    b0 >= 0.0 && b1 >= 0.0 && b2 >= 0.0
}

fn assert_premultiplied_red(snapshot: &Rgba8Snapshot) {
    for pixel in snapshot.rgba8.chunks_exact(4) {
        assert_eq!(pixel[1], 0);
        assert_eq!(pixel[2], 0);
        assert!(
            pixel[0] <= pixel[3].saturating_add(1),
            "red channel {} exceeds premultiplied alpha {}",
            pixel[0],
            pixel[3]
        );
    }
}

fn set_param(params: &mut [(String, ParamValue)], name: &str, value: ParamValue) {
    let Some((_, existing)) = params.iter_mut().find(|(param, _)| param == name) else {
        panic!("brush preset parameter {name:?} should exist");
    };
    *existing = value;
}

fn set_brush_radius_and_params(
    runtime: &mut ApplicationRuntime,
    radius_world: f32,
    configure_params: impl FnOnce(&mut Vec<(String, ParamValue)>),
) {
    let mut preset = runtime
        .state
        .active_tool_preset()
        .expect("brush test requires an active stroke preset")
        .clone();
    let scene_diagonal = runtime
        .state
        .document()
        .expect("brush test requires a document")
        .mesh
        .scene_diagonal();
    let StrokePresetOp::BrushEngine {
        size_scene_ratio,
        size_pressure,
        params,
        ..
    } = &mut preset.stroke_op;
    size_scene_ratio.value = radius_world * 2.0 / scene_diagonal;
    size_pressure.enabled = false;
    preset.stroke_strategy = StrokeStrategy::RawEvent;
    preset.input_filter = StrokeInputFilter::default();
    configure_params(params);
    runtime
        .dispatch(Command::UpdateActiveToolPreset(preset))
        .expect("active brush runtime preset override should be accepted");
}

fn select_brush_preset(runtime: &mut ApplicationRuntime, config_id: &str) {
    let tool_id = runtime
        .state
        .tool_id_by_config_id(config_id)
        .unwrap_or_else(|| panic!("brush preset {config_id:?} should exist"));
    runtime
        .dispatch(Command::SetActiveTool(tool_id))
        .expect("brush preset should be selectable");
}

fn set_paint_brush(runtime: &mut ApplicationRuntime, radius_world: f32) {
    select_brush_preset(runtime, "builtin.brush.paint.round_soft");
    set_brush_radius_and_params(runtime, radius_world, |params| {
        set_param(params, "composite_mode", ParamValue::Enum(0));
        set_param(params, "opacity", ParamValue::F32(1.0));
        set_param(params, "flow", ParamValue::F32(1.0));
        set_param(params, "hardness", ParamValue::F32(1.0));
    });
}

fn set_smudge_brush(runtime: &mut ApplicationRuntime, radius_world: f32) {
    select_brush_preset(runtime, "builtin.brush.smudge.soft");
    set_brush_radius_and_params(runtime, radius_world, |params| {
        set_param(params, "opacity", ParamValue::F32(1.0));
        set_param(params, "flow", ParamValue::F32(1.0));
        set_param(params, "hardness", ParamValue::F32(1.0));
        set_param(params, "smudge_length_ratio", ParamValue::F32(3.0));
        set_param(params, "sample_count", ParamValue::U32(16));
    });
}

fn set_blur_brush(runtime: &mut ApplicationRuntime, radius_world: f32) {
    select_brush_preset(runtime, "builtin.brush.filter.blur_soft");
    set_brush_radius_and_params(runtime, radius_world, |params| {
        set_param(params, "opacity", ParamValue::F32(1.0));
        set_param(params, "strength", ParamValue::F32(1.0));
        set_param(params, "blur_radius_ratio", ParamValue::F32(1.0));
        set_param(params, "hardness", ParamValue::F32(0.5));
    });
}

fn assert_pixel_near(actual: [u8; 4], expected: [u8; 4], tolerance: u8) {
    for channel in 0..4 {
        let delta = actual[channel].abs_diff(expected[channel]);
        assert!(
            delta <= tolerance,
            "pixel mismatch at channel {channel}: actual {actual:?}, expected {expected:?}, tolerance {tolerance}"
        );
    }
}

fn surface_test_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(0.5, 0.5, 0.0),
        orientation: Quat::IDENTITY,
        distance: 2.0,
        projection: crate::core::camera::CameraProjection::Perspective,
        fov_y_radians: 50.0_f32.to_radians(),
        orthographic_height: 4.0 * (25.0_f32.to_radians()).tan(),
        near: 0.1,
        far: 10.0,
    }
}

fn surface_test_view(viewport_size: [u32; 2]) -> (OrbitCamera, ViewportInputContext) {
    let camera = surface_test_camera();
    let view_proj =
        camera.view_projection_matrix(viewport_size[0] as f32 / viewport_size[1] as f32);
    let view = ViewportInputContext {
        size: viewport_size,
        view_proj,
        inv_view_proj: view_proj.inverse(),
        camera_world: camera.position().into(),
    };
    (camera, view)
}

fn surface_orthographic_test_view(viewport_size: [u32; 2]) -> (OrbitCamera, ViewportInputContext) {
    let mut camera = surface_test_camera();
    camera.set_projection(crate::core::camera::CameraProjection::Orthographic);
    let view_proj =
        camera.view_projection_matrix(viewport_size[0] as f32 / viewport_size[1] as f32);
    let view = ViewportInputContext {
        size: viewport_size,
        view_proj,
        inv_view_proj: view_proj.inverse(),
        camera_world: camera.position().into(),
    };
    (camera, view)
}

fn symmetric_panel_view(viewport_size: [u32; 2]) -> (OrbitCamera, ViewportInputContext) {
    let camera = OrbitCamera {
        target: Vec3::ZERO,
        orientation: Quat::IDENTITY,
        distance: 2.0,
        projection: crate::core::camera::CameraProjection::Perspective,
        fov_y_radians: 50.0_f32.to_radians(),
        orthographic_height: 4.0 * (25.0_f32.to_radians()).tan(),
        near: 0.1,
        far: 10.0,
    };
    let view_proj =
        camera.view_projection_matrix(viewport_size[0] as f32 / viewport_size[1] as f32);
    let view = ViewportInputContext {
        size: viewport_size,
        view_proj,
        inv_view_proj: view_proj.inverse(),
        camera_world: camera.position().into(),
    };
    (camera, view)
}

fn world_to_screen(view_proj: Mat4, world: Vec3, viewport_size: [u32; 2]) -> Vec2 {
    let ndc = view_proj.project_point3(world);
    Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport_size[0] as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size[1] as f32,
    )
}

fn uv_sample(uv: Vec2, view_size: [u32; 2], time_s: f64) -> crate::application::PointerSample {
    crate::application::PointerSample::uv_with_pressure_at(
        uv,
        Vec2::new(
            uv.x * view_size[0].max(1) as f32,
            uv.y * view_size[1].max(1) as f32,
        ),
        view_size,
        1.0,
        time_s,
        InputModifiers::default(),
    )
}

fn stroke_uv(runtime: &mut ApplicationRuntime, points: &[Vec2], view_size: [u32; 2]) {
    assert!(
        points.len() >= 2,
        "UV stroke needs at least down and up samples"
    );
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerDown(uv_sample(
            points[0], view_size, 0.0,
        ))))
        .unwrap();
    for (index, point) in points
        .iter()
        .copied()
        .enumerate()
        .skip(1)
        .take(points.len() - 2)
    {
        runtime
            .dispatch(Command::ToolInput(ToolInputEvent::PointerMove(uv_sample(
                point,
                view_size,
                index as f64 * 0.1,
            ))))
            .unwrap();
    }
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerUp(uv_sample(
            *points.last().unwrap(),
            view_size,
            points.len() as f64 * 0.1,
        ))))
        .unwrap();
}

fn assert_snapshot_matches_document_and_renderer(
    runtime: &ApplicationRuntime,
    engine: &mut RenderEngine,
    surface: PaintSurfaceId,
) -> Rgba8Snapshot {
    let document = document_surface(runtime, surface);
    let renderer = read_surface(engine, surface);
    assert_eq!(document.rgba8, renderer.rgba8);
    document
}

fn view_pointer_event(
    phase: ViewPointerPhase,
    position_px: Vec2,
    time_s: f64,
    view: ViewportInputContext,
) -> Command {
    Command::ViewPointer(ViewPointerEvent::Viewport3d {
        meta: ViewPointerMeta {
            phase,
            position_px,
            pressure: 1.0,
            time_s,
            modifiers: InputModifiers::default(),
        },
        view,
    })
}

fn lasso_view_pointer_path(
    runtime: &mut ApplicationRuntime,
    points: &[Vec2],
    view: ViewportInputContext,
) {
    assert!(
        points.len() >= 3,
        "viewport lasso needs at least three samples"
    );
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            points[0],
            0.0,
            view,
        ))
        .unwrap();
    for (index, point) in points
        .iter()
        .copied()
        .enumerate()
        .skip(1)
        .take(points.len() - 2)
    {
        runtime
            .dispatch(view_pointer_event(
                ViewPointerPhase::Move,
                point,
                index as f64 * 0.1,
                view,
            ))
            .unwrap();
    }
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            *points.last().unwrap(),
            points.len() as f64 * 0.1,
            view,
        ))
        .unwrap();
}

#[test]
fn single_pass_through_group_merge_bakes_to_a_raster_surface() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, source_surface) = load_runtime(&mut engine, size, [255, 0, 0, 255]);
    let source_layer = source_surface.layer_id;

    runtime.dispatch(Command::AddGroup).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let group = runtime.state.document.active_layer_id();

    runtime
        .dispatch(Command::MoveLayer {
            layer_id: source_layer,
            new_parent: group,
            new_index: 0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::SetLayerGroupCompositeMode {
            layer_id: group,
            mode: GroupCompositeMode::PassThrough,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let before = read_composite(&mut engine);

    runtime
        .dispatch(Command::MergeLayers {
            layer_ids: vec![group],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("group merge should produce renderer commit artifacts");

    let merged_surface = runtime
        .state
        .active_paint_surface()
        .expect("group merge should activate the baked raster layer");
    assert_ne!(merged_surface, source_surface);
    let merged =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, merged_surface);
    assert_eq!(merged.rgba8, solid_rgba(size, [255, 0, 0, 255]));
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("merge should record history")
            .label,
        "Merge Layers"
    );
}

#[test]
fn solid_fill_rasterize_preserves_composite_properties_mask_and_history() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [32, 64, 96, 255]);
    runtime
        .dispatch(Command::AddSolidFillLayer {
            color: [0.8, 0.4, 0.2],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let layer_id = runtime.state.active_layer_id();
    runtime
        .dispatch(Command::SetLayerOpacity {
            layer_id,
            opacity: 0.5,
        })
        .unwrap();
    runtime
        .dispatch(Command::SetLayerBlendMode {
            layer_id,
            blend_mode: LayerBlendMode::Multiply,
        })
        .unwrap();
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let before = read_composite(&mut engine);

    runtime
        .dispatch(Command::RasterizeLayer { layer_id })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("solid fill rasterize should produce renderer commit artifacts");
    let document = runtime.state.document().unwrap();
    let node = document.layer_tree.get(layer_id).unwrap();
    assert!(matches!(node.content, LayerContent::Raster));
    assert_eq!(node.props.opacity, 0.5);
    assert_eq!(node.props.blend_mode, LayerBlendMode::Multiply);
    assert!(node.mask.is_some());
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
    let raster = PaintSurfaceId::raster(0.into(), layer_id);
    assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, raster);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(matches!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .get(layer_id)
            .unwrap()
            .content,
        LayerContent::SolidFill { .. }
    ));
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .is_paintable(layer_id)
    );
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
}

#[test]
fn embedded_image_rasterize_bakes_transform_and_restores_asset_on_undo() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime
        .dispatch(
            load_rgba8_as_command_at(
                "rasterize.png",
                "rasterize",
                [2, 2],
                solid_rgba([2, 2], [0, 255, 64, 192]),
                4096,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let layer_id = runtime.state.active_layer_id();
    let image_id = runtime
        .state
        .document()
        .unwrap()
        .layer_tree
        .embedded_image(layer_id)
        .unwrap()
        .0;
    let before = read_composite(&mut engine);

    runtime
        .dispatch(Command::RasterizeLayer { layer_id })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("embedded image rasterize should produce renderer commit artifacts");
    let document = runtime.state.document().unwrap();
    assert!(document.layer_tree.is_paintable(layer_id));
    assert!(document.embedded_image(image_id).is_none());
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert_eq!(
        document.layer_tree.embedded_image(layer_id).unwrap().0,
        image_id
    );
    assert!(document.embedded_image(image_id).is_some());
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert!(document.layer_tree.is_paintable(layer_id));
    assert!(document.embedded_image(image_id).is_none());
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
}

#[test]
fn embedded_image_mask_apply_bakes_once_and_restores_asset_and_mask_on_undo() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime
        .dispatch(
            load_rgba8_as_command_at(
                "masked.png",
                "masked",
                [2, 2],
                solid_rgba([2, 2], [64, 128, 255, 192]),
                4096,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let layer_id = runtime.state.active_layer_id();
    let image_id = runtime
        .state
        .document()
        .unwrap()
        .layer_tree
        .embedded_image(layer_id)
        .unwrap()
        .0;
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let before = read_composite(&mut engine);

    runtime
        .dispatch(Command::ApplyLayerMask { layer_id })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("embedded image mask apply should produce renderer commit artifacts");
    let document = runtime.state.document().unwrap();
    assert!(document.layer_tree.is_paintable(layer_id));
    assert!(!document.layer_tree.has_layer_mask(layer_id));
    assert!(document.embedded_image(image_id).is_none());
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert_eq!(
        document.layer_tree.embedded_image(layer_id).unwrap().0,
        image_id
    );
    assert!(document.layer_tree.has_layer_mask(layer_id));
    assert!(document.embedded_image(image_id).is_some());
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert!(document.layer_tree.is_paintable(layer_id));
    assert!(!document.layer_tree.has_layer_mask(layer_id));
    assert!(document.embedded_image(image_id).is_none());
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
}

#[test]
fn apply_disabled_layer_mask_uses_saved_pixels_and_undo_redo_restores_state() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, raster) = load_runtime(&mut engine, size, [255, 64, 32, 255]);
    let layer_id = raster.layer_id;
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::HideAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::SetLayerMaskEnabled {
            layer_id,
            enabled: false,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::SelectLayerMask { layer_id })
        .unwrap();
    assert_eq!(
        read_composite(&mut engine).rgba8,
        solid_rgba(size, [255, 64, 32, 255])
    );

    runtime
        .dispatch(Command::ApplyLayerMask { layer_id })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mask apply should produce renderer commit artifacts");
    assert!(
        !runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .has_layer_mask(layer_id)
    );
    assert_eq!(runtime.state.active_paint_surface(), Some(raster));
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, raster).rgba8,
        solid_rgba(size, [0, 0, 0, 0])
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let mask = runtime
        .state
        .document()
        .unwrap()
        .layer_tree
        .layer_mask(layer_id)
        .unwrap();
    assert!(!mask.enabled);
    assert_eq!(
        read_composite(&mut engine).rgba8,
        solid_rgba(size, [255, 64, 32, 255])
    );
    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(
        !runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .has_layer_mask(layer_id)
    );
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, raster).rgba8,
        solid_rgba(size, [0, 0, 0, 0])
    );
}

#[test]
fn solid_fill_mask_apply_creates_raster_and_restores_fill_and_mask_on_undo() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [16, 32, 48, 255]);
    runtime
        .dispatch(Command::AddSolidFillLayer {
            color: [1.0, 0.5, 0.25],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let layer_id = runtime.state.active_layer_id();
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::HideAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let before = read_composite(&mut engine);

    runtime
        .dispatch(Command::ApplyLayerMask { layer_id })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("solid fill mask apply should produce renderer commit artifacts");
    let raster = PaintSurfaceId::raster(0.into(), layer_id);
    let document = runtime.state.document().unwrap();
    assert!(document.layer_tree.is_paintable(layer_id));
    assert!(!document.layer_tree.has_layer_mask(layer_id));
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, raster).rgba8,
        solid_rgba(size, [0, 0, 0, 0])
    );
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
    assert_eq!(
        runtime.state.history.undo_stack.last().unwrap().label,
        "Apply Layer Mask"
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert!(matches!(
        document.layer_tree.get(layer_id).unwrap().content,
        LayerContent::SolidFill { .. }
    ));
    assert!(document.layer_tree.has_layer_mask(layer_id));
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert!(document.layer_tree.is_paintable(layer_id));
    assert!(!document.layer_tree.has_layer_mask(layer_id));
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, raster).rgba8,
        solid_rgba(size, [0, 0, 0, 0])
    );
}

#[test]
fn layer_blend_modes_match_reference_results() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [51, 102, 153, 255]);
    runtime
        .dispatch(Command::AddSolidFillLayer {
            color: [0.25, 0.5, 0.75],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let blend_layer = runtime.state.document.active_layer_id();

    let cases: [(LayerBlendMode, [u8; 4]); 11] = [
        (LayerBlendMode::Normal, [64, 128, 191, 255]),
        (LayerBlendMode::Darken, [51, 102, 153, 255]),
        (LayerBlendMode::Multiply, [13, 51, 115, 255]),
        (LayerBlendMode::Lighten, [64, 128, 191, 255]),
        (LayerBlendMode::Screen, [102, 178, 230, 255]),
        (LayerBlendMode::ColorDodge, [68, 204, 255, 255]),
        (LayerBlendMode::LinearDodge, [115, 230, 255, 255]),
        (LayerBlendMode::Overlay, [26, 102, 204, 255]),
        (LayerBlendMode::SoftLight, [31, 102, 175, 255]),
        (LayerBlendMode::HardLight, [26, 102, 204, 255]),
        (LayerBlendMode::Color, [41, 104, 168, 255]),
    ];

    for (mode, expected) in cases {
        runtime
            .dispatch(Command::SetLayerBlendMode {
                layer_id: blend_layer,
                blend_mode: mode,
            })
            .unwrap();
        execute_runtime_effects(&mut runtime, &mut engine);
        let composite = read_composite(&mut engine);
        let actual = &composite.rgba8[..4];
        for channel in 0..4 {
            assert!(
                (i16::from(actual[channel]) - i16::from(expected[channel])).abs() <= 1,
                "{mode:?} channel {channel}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

#[test]
fn color_dodge_keeps_black_backdrop_black_with_white_source() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 255]);
    runtime
        .dispatch(Command::AddSolidFillLayer {
            color: [1.0, 1.0, 1.0],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let blend_layer = runtime.state.document.active_layer_id();
    runtime
        .dispatch(Command::SetLayerBlendMode {
            layer_id: blend_layer,
            blend_mode: LayerBlendMode::ColorDodge,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);

    assert_eq!(&read_composite(&mut engine).rgba8[..4], &[0, 0, 0, 255]);
}

#[test]
fn multiply_layer_merge_bakes_to_a_raster_surface() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, source_surface) = load_runtime(&mut engine, size, [192, 96, 48, 255]);
    let source_layer = source_surface.layer_id;

    runtime
        .dispatch(Command::AddSolidFillLayer {
            color: [0.5, 1.0, 0.5],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let multiply_layer = runtime.state.document.active_layer_id();
    runtime
        .dispatch(Command::SetLayerBlendMode {
            layer_id: multiply_layer,
            blend_mode: LayerBlendMode::Multiply,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let before = read_composite(&mut engine);

    runtime
        .dispatch(Command::MergeLayers {
            layer_ids: vec![source_layer, multiply_layer],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("multiply layer merge should produce renderer commit artifacts");

    let merged_surface = runtime
        .state
        .active_paint_surface()
        .expect("multiply layer merge should activate the baked raster layer");
    let merged =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, merged_surface);
    assert_eq!(merged.rgba8, before.rgba8);
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
}

#[test]
fn adjustment_layer_merge_bakes_to_a_raster_surface() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, source_surface) = load_runtime(&mut engine, size, [255, 64, 0, 255]);
    let source_layer = source_surface.layer_id;

    runtime
        .dispatch(Command::AddAdjustmentLayer {
            kind: AdjustmentKind::Invert,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let adjustment_layer = runtime.state.document.active_layer_id();
    let before = read_composite(&mut engine);

    runtime
        .dispatch(Command::MergeLayers {
            layer_ids: vec![source_layer, adjustment_layer],
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("adjustment layer merge should produce renderer commit artifacts");

    let merged_surface = runtime
        .state
        .active_paint_surface()
        .expect("adjustment layer merge should activate the baked raster layer");
    let merged =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, merged_surface);
    assert_eq!(merged.rgba8, before.rgba8);
    assert_eq!(read_composite(&mut engine).rgba8, before.rgba8);
}

#[test]
fn pasted_image_commits_immediately_without_changing_tool() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, original_surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    let active_tool = runtime.state.active_tool_id();
    let effective_tool = runtime.state.effective_tool_id();
    let panel_tool = runtime.state.panel_tool_id();
    let original_layer_count = runtime.state.document().unwrap().layer_tree.rows().len();

    let command = load_rgba8_as_paste_command_at(
        "Clipboard Image",
        "Clipboard Image",
        [2, 2],
        solid_rgba([2, 2], [255, 0, 0, 255]),
        4096,
        Some([1, 0]),
    )
    .unwrap();
    runtime.dispatch(command).expect("Paste should be accepted");

    let pasted_surface = runtime
        .state
        .active_paint_surface()
        .expect("Paste should activate the new raster layer");
    assert_ne!(pasted_surface, original_surface);
    assert_eq!(runtime.state.active_tool_id(), active_tool);
    assert_eq!(runtime.state.effective_tool_id(), effective_tool);
    assert_eq!(runtime.state.panel_tool_id(), panel_tool);
    assert!(!runtime.state.has_active_modal_tool());
    assert!(!runtime.state.has_active_transform_session());
    assert!(!runtime.state.is_tool_interacting());
    assert_eq!(
        runtime.state.document().unwrap().layer_tree.rows().len(),
        original_layer_count + 1
    );

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Paste should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    let pasted =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, pasted_surface);
    assert_eq!(pixel_at(&pasted, 1, 0), [255, 0, 0, 255]);
    assert_eq!(pixel_at(&pasted, 2, 1), [255, 0, 0, 255]);
    assert_eq!(pixel_at(&pasted, 0, 0), [0, 0, 0, 0]);
    assert_eq!(pixel_at(&pasted, 3, 3), [0, 0, 0, 0]);
    assert_eq!(runtime.state.history.undo_stack.len(), 1);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(runtime.state.active_paint_surface(), Some(original_surface));
    assert_eq!(
        runtime.state.document().unwrap().layer_tree.rows().len(),
        original_layer_count
    );
    assert_eq!(runtime.state.effective_tool_id(), effective_tool);
    assert!(!runtime.state.has_active_modal_tool());

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(runtime.state.active_paint_surface(), Some(pasted_surface));
    let redone =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, pasted_surface);
    assert_eq!(redone.rgba8, pasted.rgba8);
    assert_eq!(runtime.state.effective_tool_id(), effective_tool);
    assert!(!runtime.state.has_active_modal_tool());
}

#[test]
fn imported_image_creates_an_embedded_layer_without_a_raster_surface() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    let active_tool = runtime.state.active_tool_id();
    let command = load_rgba8_as_command_at(
        "image.png",
        "image",
        [2, 2],
        solid_rgba([2, 2], [255, 0, 0, 255]),
        4096,
        None,
    )
    .unwrap();

    runtime
        .dispatch(command)
        .expect("Image import should be accepted");

    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(runtime.state.active_tool_id(), active_tool);
    assert!(!runtime.state.has_active_transform_session());
    assert_eq!(runtime.state.effective_tool_id(), ToolId::Transform);
    assert_eq!(runtime.state.panel_tool_id(), ToolId::Transform);
    let document = runtime.state.document().unwrap();
    let active = runtime.state.active_layer_id();
    let (image_id, transform) = document
        .layer_tree
        .embedded_image(active)
        .expect("active layer must be embedded image");
    assert_eq!(
        document.embedded_image(image_id).unwrap().rgba8.as_ref(),
        solid_rgba([2, 2], [255, 0, 0, 255])
    );
    assert_eq!(transform.center_uv, Vec2::splat(0.5));
    assert!(
        !document
            .tiles
            .contains_surface(PaintSurfaceId::raster(0usize.into(), active))
    );
    assert_eq!(center_pixel(&read_composite(&mut engine)), [255, 0, 0, 255]);

    runtime
        .dispatch(Command::BeginTransformNumericEdit)
        .unwrap();
    runtime
        .dispatch(Command::UpdateTransformNumeric(
            TransformNumericEdit::Width(f32::MIN_POSITIVE),
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!((runtime.state.transform_numeric_values().unwrap().size_px.x - 0.02).abs() < 1.0e-6);
    runtime
        .dispatch(Command::UpdateTransformNumeric(
            TransformNumericEdit::Width(2.0),
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);

    runtime
        .dispatch(Command::UpdateTransformNumeric(
            TransformNumericEdit::CenterX(1.0),
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(runtime.state.has_active_transform_session());
    let preview = read_composite(&mut engine);
    assert_eq!(pixel_at(&preview, 0, 1), [255, 0, 0, 255]);
    assert_eq!(pixel_at(&preview, 3, 1), [0, 0, 0, 0]);

    runtime
        .dispatch(Command::CommitTransformNumericEdit)
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .embedded_image(active)
            .unwrap()
            .1
            .center_uv
            .x,
        0.25
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .embedded_image(active)
            .unwrap()
            .1
            .center_uv
            .x,
        0.5
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(
        !runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .contains(active)
    );

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    let restored = runtime.state.active_layer_id();
    let (restored_image_id, _) = document.layer_tree.embedded_image(restored).unwrap();
    assert_eq!(restored_image_id, image_id);
    assert_eq!(
        document
            .embedded_image(restored_image_id)
            .unwrap()
            .rgba8
            .as_ref(),
        solid_rgba([2, 2], [255, 0, 0, 255])
    );

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .embedded_image(restored)
            .unwrap()
            .1
            .center_uv
            .x,
        0.25
    );

    runtime
        .dispatch(Command::DeleteLayer { layer_id: restored })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        runtime.state.document().unwrap().embedded_images().count(),
        0
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert_eq!(document.embedded_images().count(), 1);
    assert_eq!(
        document.layer_tree.embedded_image(restored).unwrap().0,
        image_id
    );

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        runtime.state.document().unwrap().embedded_images().count(),
        0
    );
}

#[test]
fn embedded_image_transform_handle_accepts_uv_view_pointer_drag() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    let original_layer = runtime.state.active_layer_id();
    runtime
        .dispatch(
            load_rgba8_as_command_at(
                "drag.png",
                "drag",
                [2, 2],
                solid_rgba([2, 2], [255, 0, 0, 255]),
                4096,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let history_after_import = runtime.state.history.undo_stack.len();
    assert!(!runtime.state.has_active_transform_session());
    assert!(runtime.state.uv_transform_preview().is_some());
    let _ = runtime.take_last_effect_metrics();

    let view = UvViewInputContext {
        size: [400, 400],
        canvas_size: size,
        transform: UvViewTransform {
            zoom: 25.0,
            ..UvViewTransform::default()
        },
    };
    let mut preview_full_composites = 0;
    let mut preview_partial_composites = 0;
    for (phase, position_px, time_s) in [
        (ViewPointerPhase::Down, Vec2::new(200.0, 200.0), 1.0),
        (ViewPointerPhase::Move, Vec2::new(225.0, 200.0), 1.1),
        (ViewPointerPhase::Up, Vec2::new(225.0, 200.0), 1.2),
    ] {
        runtime
            .dispatch(Command::ViewPointer(ViewPointerEvent::Uv {
                meta: ViewPointerMeta {
                    phase,
                    position_px,
                    pressure: 1.0,
                    time_s,
                    modifiers: InputModifiers::default(),
                },
                view,
            }))
            .unwrap();
        if phase == ViewPointerPhase::Down {
            assert!(runtime.state.is_tool_pointer_gesture_active());
        }
        if phase == ViewPointerPhase::Move {
            assert_eq!(
                runtime.state.transform_numeric_values().unwrap().center_px,
                Vec2::new(9.0, 8.0)
            );
        }
        execute_runtime_effects(&mut runtime, &mut engine);
        let metrics = runtime.take_last_effect_metrics();
        preview_full_composites += metrics.full_composite_count;
        preview_partial_composites += metrics.partial_composite_count;
    }

    assert_eq!(
        runtime.state.transform_numeric_values().unwrap().center_px,
        Vec2::new(9.0, 8.0)
    );
    assert_eq!(preview_full_composites, 1);
    assert!(preview_partial_composites > 0);
    assert_eq!(
        runtime.state.history.undo_stack.len(),
        history_after_import + 1
    );
    let preview = read_composite(&mut engine);
    assert_eq!(pixel_at(&preview, 7, 8), [0, 0, 0, 0]);
    assert_eq!(pixel_at(&preview, 9, 8), [255, 0, 0, 255]);

    for (phase, position_px, time_s) in [
        (ViewPointerPhase::Down, Vec2::new(225.0, 200.0), 2.0),
        (ViewPointerPhase::Move, Vec2::new(237.5, 200.0), 2.1),
    ] {
        runtime
            .dispatch(Command::ViewPointer(ViewPointerEvent::Uv {
                meta: ViewPointerMeta {
                    phase,
                    position_px,
                    pressure: 1.0,
                    time_s,
                    modifiers: InputModifiers::default(),
                },
                view,
            }))
            .unwrap();
        execute_runtime_effects(&mut runtime, &mut engine);
    }
    assert_eq!(
        runtime.state.transform_numeric_values().unwrap().center_px,
        Vec2::new(9.5, 8.0)
    );
    runtime
        .dispatch(Command::ViewPointer(ViewPointerEvent::Cancel {
            reason: crate::application::ToolCancelReason::Escape,
        }))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(!runtime.state.has_active_transform_session());
    assert!(!runtime.state.is_tool_pointer_gesture_active());
    assert_eq!(
        runtime.state.transform_numeric_values().unwrap().center_px,
        Vec2::new(9.0, 8.0)
    );
    assert_eq!(
        runtime.state.history.undo_stack.len(),
        history_after_import + 1
    );

    for (phase, position_px, time_s) in [
        (ViewPointerPhase::Down, Vec2::new(225.0, 200.0), 3.0),
        (ViewPointerPhase::Move, Vec2::new(250.0, 200.0), 3.1),
    ] {
        runtime
            .dispatch(Command::ViewPointer(ViewPointerEvent::Uv {
                meta: ViewPointerMeta {
                    phase,
                    position_px,
                    pressure: 1.0,
                    time_s,
                    modifiers: InputModifiers::default(),
                },
                view,
            }))
            .unwrap();
        execute_runtime_effects(&mut runtime, &mut engine);
    }
    runtime
        .dispatch(Command::SelectLayer {
            layer_id: original_layer,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(!runtime.state.has_active_transform_session());
    assert_eq!(runtime.state.active_layer_id(), original_layer);
    assert_eq!(
        runtime.state.history.undo_stack.len(),
        history_after_import + 2
    );
}

#[test]
fn embedded_image_tree_history_retains_only_reference_transitions() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime
        .dispatch(
            load_rgba8_as_command_at(
                "history.png",
                "history",
                [2, 2],
                solid_rgba([2, 2], [255, 255, 255, 255]),
                4096,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let original = runtime.state.active_layer_id();
    assert!(matches!(
        runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
        Some(crate::application::HistoryAtom::LayerTreeEdit { edit })
            if edit.retained_embedded_images.len() == 1
    ));

    runtime
        .dispatch(Command::DuplicateLayer { layer_id: original })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let duplicate = runtime.state.active_layer_id();
    assert!(matches!(
        runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
        Some(crate::application::HistoryAtom::LayerTreeEdit { edit })
            if edit.retained_embedded_images.is_empty()
    ));

    runtime
        .dispatch(Command::DeleteLayer {
            layer_id: duplicate,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(matches!(
        runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
        Some(crate::application::HistoryAtom::LayerTreeEdit { edit })
            if edit.retained_embedded_images.is_empty()
    ));

    runtime
        .dispatch(Command::DeleteLayer { layer_id: original })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(matches!(
        runtime.state.history.undo_stack.last().and_then(|tx| tx.atoms.first()),
        Some(crate::application::HistoryAtom::LayerTreeEdit { edit })
            if edit.retained_embedded_images.len() == 1
    ));
}

#[test]
fn imported_embedded_image_is_immediately_committed_and_undoable() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    let original_layer_count = runtime.state.document().unwrap().layer_tree.rows().len();
    let original_layer = runtime.state.active_layer_id();
    let original_tool = runtime.state.active_tool_id();
    let command = load_rgba8_as_command_at(
        "cancel.png",
        "cancel",
        [2, 2],
        solid_rgba([2, 2], [0, 255, 0, 255]),
        4096,
        None,
    )
    .unwrap();

    runtime.dispatch(command).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let imported_layer = runtime.state.active_layer_id();
    assert!(!runtime.state.has_active_transform_session());
    assert_eq!(
        runtime.state.document().unwrap().embedded_images().count(),
        1
    );
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    assert_eq!(runtime.state.effective_tool_id(), ToolId::Transform);
    let unavailable_tool = runtime
        .state
        .tool_catalog()
        .iter()
        .map(|tool| tool.id)
        .find(|tool_id| *tool_id != original_tool)
        .unwrap();
    runtime
        .dispatch(Command::SetActiveTool(unavailable_tool))
        .unwrap();
    assert_eq!(runtime.state.active_tool_id(), original_tool);
    let shortcut_token = InputHoldToken(91);
    runtime
        .dispatch(Command::BeginMomentaryTool {
            token: shortcut_token,
            tool_id: unavailable_tool,
        })
        .unwrap();
    runtime
        .dispatch(Command::CompleteMomentaryTool {
            token: shortcut_token,
            tool_id: unavailable_tool,
            select_tool: true,
        })
        .unwrap();
    assert_eq!(runtime.state.active_tool_id(), original_tool);

    runtime.dispatch(Command::CancelActiveTransform).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .contains(imported_layer)
    );

    runtime
        .dispatch(Command::SelectLayer {
            layer_id: original_layer,
        })
        .unwrap();
    assert_eq!(runtime.state.effective_tool_id(), original_tool);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let document = runtime.state.document().unwrap();
    assert!(!runtime.state.has_active_transform_session());
    assert!(!document.layer_tree.contains(imported_layer));
    assert_eq!(document.layer_tree.rows().len(), original_layer_count);
    assert_eq!(document.embedded_images().count(), 0);
    assert!(runtime.state.history.undo_stack.is_empty());
}

#[test]
fn embedded_image_transform_is_available_only_on_its_target_material() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime_with_material_pixels(
        &mut engine,
        folded_material_pair_mesh(),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![solid_rgba(size, [0, 0, 0, 0]); 2],
    );
    runtime
        .dispatch(
            load_rgba8_as_command_at(
                "material.png",
                "material",
                [2, 2],
                solid_rgba([2, 2], [255, 0, 0, 255]),
                4096,
                None,
            )
            .unwrap(),
        )
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let image_layer = runtime.state.active_layer_id();
    assert!(runtime.state.uv_transform_preview().is_some());

    runtime
        .dispatch(Command::BeginTransformNumericEdit)
        .unwrap();
    runtime
        .dispatch(Command::UpdateTransformNumeric(
            TransformNumericEdit::CenterX(3.0),
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(runtime.state.has_active_transform_session());

    runtime.dispatch(Command::SetFocusedMaterial(1)).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(!runtime.state.has_active_transform_session());
    assert_eq!(runtime.state.effective_tool_id(), ToolId::Transform);
    assert!(runtime.state.uv_transform_preview().is_none());
    assert!(runtime.state.transform_numeric_values().is_none());
    assert_eq!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .embedded_image(image_layer)
            .unwrap()
            .1
            .center_uv
            .x,
        3.0 / size[0] as f32
    );

    runtime
        .dispatch(Command::ViewPointer(ViewPointerEvent::Uv {
            meta: ViewPointerMeta {
                phase: ViewPointerPhase::Down,
                position_px: Vec2::new(100.0, 100.0),
                pressure: 1.0,
                time_s: 1.0,
                modifiers: InputModifiers::default(),
            },
            view: UvViewInputContext {
                size: [200, 200],
                canvas_size: size,
                transform: UvViewTransform::default(),
            },
        }))
        .unwrap();
    assert!(!runtime.state.has_active_transform_session());

    runtime.dispatch(Command::SetFocusedMaterial(0)).unwrap();
    assert!(runtime.state.uv_transform_preview().is_some());

    runtime
        .dispatch(Command::BeginTransformNumericEdit)
        .unwrap();
    runtime
        .dispatch(Command::UpdateTransformNumeric(
            TransformNumericEdit::CenterY(3.0),
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(runtime.state.has_active_transform_session());
    runtime
        .dispatch(Command::SetLayerLocked {
            layer_id: image_layer,
            locked: true,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(!runtime.state.has_active_transform_session());
    assert_eq!(runtime.state.effective_tool_id(), ToolId::Transform);
    assert!(runtime.state.uv_transform_preview().is_none());
    assert!(runtime.state.transform_numeric_values().is_none());

    runtime
        .dispatch(Command::SetLayerLocked {
            layer_id: image_layer,
            locked: false,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(runtime.state.uv_transform_preview().is_some());
}

#[test]
fn embedded_image_filtering_premultiplies_each_source_texel_before_bilinear_mix() {
    let size = [4, 1];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    let command = load_rgba8_as_command_at(
        "alpha-edge.png",
        "alpha-edge",
        [2, 1],
        vec![255, 0, 0, 255, 0, 0, 255, 0],
        4096,
        None,
    )
    .unwrap();
    runtime.dispatch(command).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);

    runtime
        .dispatch(Command::BeginTransformNumericEdit)
        .unwrap();
    runtime
        .dispatch(Command::UpdateTransformNumeric(
            TransformNumericEdit::Width(4.0),
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let preview = read_composite(&mut engine);
    let mixed = pixel_at(&preview, 1, 0);
    assert!(mixed[0] > 0 && mixed[3] > 0);
    assert_eq!(mixed[2], 0, "transparent blue must not bleed into the edge");

    runtime.dispatch(Command::CancelActiveTransform).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
}

#[test]
fn destructive_adjustment_filters_match_adjustment_layer_math_inside_uv_islands() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let triangle = [Vec2::ZERO, Vec2::X, Vec2::Y];
    let source_pixels = uv_triangle_pixels(size, triangle, [96, 48, 24, 128]);

    let mut levels = LevelsAdjustment::default();
    levels.master.gamma = 1.8;
    levels.red.output_black = 26;
    levels.blue.output_white = 204;

    let mut curves = crate::core::adjustment::CurvesAdjustment::default();
    curves.master.insert_point(CurvePoint {
        input: 102,
        output: 166,
    });
    curves.green.insert_point(CurvePoint {
        input: 153,
        output: 89,
    });

    let adjustments = [
        Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
            brightness: 20,
            contrast: -15,
        }),
        Adjustment::Levels(levels),
        Adjustment::Curves(curves),
        Adjustment::HueSaturation(HueSaturationAdjustment {
            hue: 75,
            saturation: 35,
            lightness: -10,
        }),
        Adjustment::Invert,
    ];

    for (case_index, adjustment) in adjustments.into_iter().enumerate() {
        let (mut layer_runtime, _) =
            load_runtime_with_pixels(&mut engine, size, source_pixels.clone());
        layer_runtime
            .dispatch(Command::AddAdjustmentLayer {
                kind: adjustment.kind(),
            })
            .unwrap();
        execute_runtime_effects(&mut layer_runtime, &mut engine);
        let adjustment_layer = layer_runtime.state.document.active_layer_id();
        if adjustment != adjustment.kind().default_adjustment() {
            layer_runtime
                .dispatch(Command::SetAdjustment {
                    layer_id: adjustment_layer,
                    adjustment: adjustment.clone(),
                    edit_session: case_index as u64 + 1,
                })
                .unwrap();
            execute_runtime_effects(&mut layer_runtime, &mut engine);
        }
        let layer_result = read_composite(&mut engine);

        let (mut filter_runtime, filter_surface) =
            load_runtime_with_pixels(&mut engine, size, source_pixels.clone());
        filter_runtime
            .dispatch(Command::ApplyAdjustmentFilter {
                layer_id: filter_surface.layer_id,
                adjustment,
            })
            .unwrap();
        execute_runtime_effects(&mut filter_runtime, &mut engine)
            .expect("destructive adjustment filter should commit its target surface");
        let filter_result = assert_snapshot_matches_document_and_renderer(
            &filter_runtime,
            &mut engine,
            filter_surface,
        );

        for y in 0..size[1] {
            for x in 0..size[0] {
                let uv = Vec2::new(
                    (x as f32 + 0.5) / size[0] as f32,
                    (y as f32 + 0.5) / size[1] as f32,
                );
                if uv_point_in_triangle(uv, triangle) {
                    assert_pixel_near(
                        pixel_at(&filter_result, x, y),
                        pixel_at(&layer_result, x, y),
                        1,
                    );
                }
            }
        }
    }
}

#[test]
fn adjustment_filter_preview_is_gpu_only_non_cumulative_and_cancel_restores_original() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let source_pixels = solid_rgba(size, [32, 48, 64, 128]);
    let final_adjustment = Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
        brightness: 40,
        contrast: 0,
    });

    let (mut expected_runtime, expected_surface) =
        load_runtime_with_pixels(&mut engine, size, source_pixels.clone());
    expected_runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id: expected_surface.layer_id,
            adjustment: final_adjustment.clone(),
        })
        .unwrap();
    execute_runtime_effects(&mut expected_runtime, &mut engine)
        .expect("one-shot adjustment filter should commit");
    let expected = document_surface(&expected_runtime, expected_surface);

    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, source_pixels);
    let initial = document_surface(&runtime, surface);
    let history_len = runtime.state.history.undo_stack.len();
    let _ = runtime.take_last_effect_metrics();
    runtime
        .dispatch(Command::BeginAdjustmentFilterSession {
            layer_id: surface.layer_id,
            kind: AdjustmentKind::BrightnessContrast,
        })
        .unwrap();

    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Some(Adjustment::BrightnessContrast(
                BrightnessContrastAdjustment {
                    brightness: 20,
                    contrast: 0,
                },
            )),
        })
        .unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "preview must not request a surface commit/readback"
    );
    let preview_metrics = runtime.take_last_effect_metrics();
    assert_eq!(preview_metrics.readback_calls, 0);
    assert_eq!(preview_metrics.upload_calls, 0);
    assert_eq!(preview_metrics.layer_gpu_rehydration_count, 0);
    assert_eq!(document_surface(&runtime, surface).rgba8, initial.rgba8);
    assert_ne!(read_surface(&mut engine, surface).rgba8, initial.rgba8);
    // The explicit test readback above is diagnostic and must not be attributed to
    // the next preview update when asserting the GPU-only preview contract.
    let _ = engine.take_metrics();
    assert_eq!(runtime.state.history.undo_stack.len(), history_len);

    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Some(final_adjustment.clone()),
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    let preview_metrics = runtime.take_last_effect_metrics();
    assert_eq!(preview_metrics.readback_calls, 0);
    assert_eq!(preview_metrics.upload_calls, 0);
    assert_eq!(preview_metrics.layer_gpu_rehydration_count, 0);
    let final_preview = read_surface(&mut engine, surface);
    assert_eq!(final_preview.rgba8, expected.rgba8);
    assert_eq!(document_surface(&runtime, surface).rgba8, initial.rgba8);

    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: None,
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert_eq!(read_surface(&mut engine, surface).rgba8, initial.rgba8);

    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Some(final_adjustment.clone()),
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    runtime
        .dispatch(Command::CancelAdjustmentFilterSession {
            layer_id: surface.layer_id,
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert_eq!(read_surface(&mut engine, surface).rgba8, initial.rgba8);
    assert_eq!(document_surface(&runtime, surface).rgba8, initial.rgba8);
    assert_eq!(runtime.state.history.undo_stack.len(), history_len);
    assert!(!runtime.state.has_adjustment_filter_session());
}

#[test]
fn adjustment_filter_preview_commit_matches_one_shot_and_undo_redo() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let source_pixels = solid_rgba(size, [40, 60, 80, 160]);
    let final_adjustment = Adjustment::HueSaturation(HueSaturationAdjustment {
        hue: 55,
        saturation: 25,
        lightness: -12,
    });

    let (mut expected_runtime, expected_surface) =
        load_runtime_with_pixels(&mut engine, size, source_pixels.clone());
    expected_runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id: expected_surface.layer_id,
            adjustment: final_adjustment.clone(),
        })
        .unwrap();
    execute_runtime_effects(&mut expected_runtime, &mut engine)
        .expect("one-shot adjustment filter should commit");
    let expected = document_surface(&expected_runtime, expected_surface);

    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, source_pixels);
    let initial = document_surface(&runtime, surface);
    let history_len = runtime.state.history.undo_stack.len();
    runtime
        .dispatch(Command::BeginAdjustmentFilterSession {
            layer_id: surface.layer_id,
            kind: AdjustmentKind::HueSaturation,
        })
        .unwrap();
    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Some(Adjustment::HueSaturation(HueSaturationAdjustment {
                hue: -30,
                saturation: 10,
                lightness: 5,
            })),
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Some(final_adjustment.clone()),
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert_eq!(read_surface(&mut engine, surface).rgba8, expected.rgba8);
    assert_eq!(document_surface(&runtime, surface).rgba8, initial.rgba8);

    runtime
        .dispatch(Command::CommitAdjustmentFilterSession {
            layer_id: surface.layer_id,
            adjustment: final_adjustment.clone(),
        })
        .unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("committed preview should request one surface commit");
    assert_eq!(artifacts.surface_commits.len(), 1);
    let committed = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(committed.rgba8, expected.rgba8);
    assert_eq!(runtime.state.history.undo_stack.len(), history_len + 1);
    assert_eq!(
        runtime.state.history.undo_stack.last().unwrap().label,
        "Hue / Saturation"
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface).rgba8,
        initial.rgba8
    );

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface).rgba8,
        expected.rgba8
    );
}

#[test]
fn adjustment_filter_preview_respects_hidden_selection_without_cpu_sync() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [10, 20, 30, 255]);

    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selection should produce selection commit artifacts");
    runtime
        .state
        .document
        .document
        .as_mut()
        .unwrap()
        .active_selection
        .visible = false;
    let initial = document_surface(&runtime, surface);
    let history_len = runtime.state.history.undo_stack.len();

    runtime
        .dispatch(Command::BeginAdjustmentFilterSession {
            layer_id: surface.layer_id,
            kind: AdjustmentKind::BrightnessContrast,
        })
        .unwrap();
    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Some(Adjustment::BrightnessContrast(
                BrightnessContrastAdjustment {
                    brightness: 30,
                    contrast: 0,
                },
            )),
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());

    let preview = read_surface(&mut engine, surface);
    assert_ne!(pixel_at(&preview, 2, 4), [10, 20, 30, 255]);
    assert_pixel_near(pixel_at(&preview, 10, 2), [10, 20, 30, 255], 1);
    assert_eq!(document_surface(&runtime, surface).rgba8, initial.rgba8);
    assert_eq!(runtime.state.history.undo_stack.len(), history_len);

    runtime
        .dispatch(Command::CancelAdjustmentFilterSession {
            layer_id: surface.layer_id,
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert_eq!(read_surface(&mut engine, surface).rgba8, initial.rgba8);
    assert_eq!(runtime.state.history.undo_stack.len(), history_len);
}

#[test]
fn destructive_invert_filter_updates_gutter_and_supports_undo_redo() {
    let size = [64, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let source_triangle = [
        Vec2::new(0.05, 0.05),
        Vec2::new(0.35, 0.05),
        Vec2::new(0.05, 0.95),
    ];
    let source_pixels = uv_triangle_pixels(size, source_triangle, [64, 0, 0, 128]);
    let initial = Rgba8Snapshot::new(size, [0, 0], size, source_pixels.clone())
        .expect("adjustment filter source should form a valid snapshot");
    let (mut runtime, surface) =
        load_runtime_with_mesh_pixels(&mut engine, uv_seam_pair_mesh(), size, source_pixels);

    runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Adjustment::Invert,
        })
        .unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Invert filter should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);

    let filtered = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&filtered, 6, 6), [64, 128, 128, 128], 1);
    let gutter_uv = Vec2::new((19.5) / 64.0, (8.5) / 32.0);
    assert!(!uv_point_in_triangle(gutter_uv, source_triangle));
    assert!(
        pixel_at(&filtered, 19, 8)[3] > 0,
        "Adjustment filter should refresh the UV island gutter"
    );
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("Invert filter should record history")
            .label,
        "Invert"
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let undone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(undone.rgba8, initial.rgba8);

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let redone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(redone.rgba8, filtered.rgba8);
}

#[test]
fn destructive_adjustment_filter_edits_the_full_raster_surface() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [64, 32, 16, 128]);

    runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Adjustment::Invert,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Invert filter should produce renderer commit artifacts");

    let filtered = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&filtered, 15, 15), [64, 96, 112, 128], 1);
}

#[test]
fn destructive_adjustment_filter_respects_hidden_active_selection() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [10, 20, 30, 255]);

    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selection should produce selection commit artifacts");
    runtime
        .state
        .document
        .document
        .as_mut()
        .unwrap()
        .active_selection
        .visible = false;

    runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id: surface.layer_id,
            adjustment: Adjustment::Invert,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected Invert should produce renderer commit artifacts");

    let filtered = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&filtered, 2, 4), [245, 235, 225, 255], 1);
    assert_pixel_near(pixel_at(&filtered, 10, 2), [10, 20, 30, 255], 1);
    assert!(runtime.state.document().unwrap().active_selection.enabled);
    assert!(!runtime.state.document().unwrap().active_selection.visible);
}

#[test]
fn surface_blur_respects_hidden_active_selection_and_supports_undo_redo() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let mut pixels = Vec::with_capacity((size[0] * size[1] * 4) as usize);
    for _y in 0..size[1] {
        for x in 0..size[0] {
            pixels.extend_from_slice(if x < 8 {
                &[128, 0, 0, 128]
            } else {
                &[0, 0, 0, 0]
            });
        }
    }
    let initial = pixels.clone();
    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, pixels);
    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selection should produce selection commit artifacts");
    runtime
        .state
        .document
        .document
        .as_mut()
        .unwrap()
        .active_selection
        .visible = false;

    runtime
        .dispatch(Command::ApplySurfaceBlur { radius_px: 8.0 })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected Surface Blur should produce renderer commit artifacts");
    let filtered = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_ne!(pixel_at(&filtered, 7, 2), [128, 0, 0, 128]);
    assert_pixel_near(pixel_at(&filtered, 9, 2), [0, 0, 0, 0], 0);
    assert!(runtime.state.document().unwrap().active_selection.enabled);
    assert!(!runtime.state.document().unwrap().active_selection.visible);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface).rgba8,
        initial
    );
    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface).rgba8,
        filtered.rgba8
    );
}

#[test]
fn surface_blur_keeps_unaffected_material_as_a_source() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let source_a = solid_rgba(size, [128, 0, 0, 128]);
    let source_b = solid_rgba(size, [0, 0, 0, 0]);
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        folded_material_pair_mesh(),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![source_a.clone(), source_b],
    );
    select_only_material_b(&mut runtime, &mut engine);

    runtime
        .dispatch(Command::ApplySurfaceBlur { radius_px: 10.0 })
        .unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected Surface Blur should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]).rgba8,
        source_a
    );
    let blurred_b =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(blurred_b.rgba8.chunks_exact(4).any(|pixel| pixel[3] > 0));
}

#[test]
fn spatial_blur_keeps_unaffected_material_as_a_source() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let source_a = solid_rgba(size, [128, 0, 0, 128]);
    let source_b = solid_rgba(size, [0, 0, 0, 0]);
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(0)], false),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![source_a.clone(), source_b],
    );
    select_only_material_b(&mut runtime, &mut engine);

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 12.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 60.0,
        })
        .unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected Spatial Blur should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]).rgba8,
        source_a
    );
    let blurred_b =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(blurred_b.rgba8.chunks_exact(4).any(|pixel| pixel[3] > 0));
}

#[test]
fn layer_surface_blur_crosses_uv_seam_updates_gutter_and_undo_redo() {
    let size = [64, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let seam_source = uv_triangle_pixels(
        size,
        [
            Vec2::new(0.05, 0.05),
            Vec2::new(0.35, 0.05),
            Vec2::new(0.05, 0.95),
        ],
        [128, 0, 0, 128],
    );
    let initial = Rgba8Snapshot::new(size, [0, 0], size, seam_source.clone())
        .expect("surface blur source should form a valid snapshot");
    let (mut runtime, surface) =
        load_runtime_with_mesh_pixels(&mut engine, uv_seam_pair_mesh(), size, seam_source);

    runtime
        .dispatch(Command::ApplySurfaceBlur { radius_px: 12.0 })
        .expect("Surface Blur should be accepted");
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Surface Blur should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);

    let blurred = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(
        (0..size[1]).any(|y| {
            (0..size[0]).any(|x| {
                let uv = Vec2::new(
                    (x as f32 + 0.5) / size[0] as f32,
                    (y as f32 + 0.5) / size[1] as f32,
                );
                uv_point_in_triangle(
                    uv,
                    [
                        Vec2::new(0.55, 0.05),
                        Vec2::new(0.80, 0.95),
                        Vec2::new(0.80, 0.05),
                    ],
                ) && pixel_at(&blurred, x, y)[3] > 0
            })
        }),
        "Surface Blur should cross the geometric edge into the opposite UV island"
    );
    assert!(
        pixel_at(&blurred, 42, 16)[3] > 0,
        "Surface Blur should refresh the four-pixel gutter outside the UV island"
    );
    assert_premultiplied_red(&blurred);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let undone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(undone.rgba8, initial.rgba8);

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let redone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(redone.rgba8, blurred.rgba8);
}

#[test]
fn layer_surface_blur_crosses_hard_material_edge() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        folded_material_pair_mesh(),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![
            solid_rgba(size, [128, 0, 0, 128]),
            solid_rgba(size, [0, 0, 0, 0]),
        ],
    );

    runtime
        .dispatch(Command::ApplySurfaceBlur { radius_px: 10.0 })
        .expect("Surface Blur should be accepted");
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Surface Blur should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 2);

    let blurred_a =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]);
    let blurred_b =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(
        blurred_b.rgba8.chunks_exact(4).any(|pixel| pixel[3] > 0),
        "Surface Blur should cross a hard edge and material boundary"
    );
    assert_premultiplied_red(&blurred_a);
    assert_premultiplied_red(&blurred_b);
}

#[test]
fn layer_spatial_blur_propagates_by_distance_and_supports_undo_redo() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let source_a = solid_rgba(size, [128, 0, 0, 128]);
    let source_b = solid_rgba(size, [0, 0, 0, 0]);
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(0)], false),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![source_a.clone(), source_b.clone()],
    );

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 4.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 60.0,
        })
        .expect("small Spatial Blur should be accepted");
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("small Spatial Blur should produce renderer commit artifacts");
    let outside_radius =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(
        outside_radius
            .rgba8
            .chunks_exact(4)
            .all(|pixel| pixel[3] == 0)
    );

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 12.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 60.0,
        })
        .expect("large Spatial Blur should be accepted");
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("large Spatial Blur should produce renderer commit artifacts");
    assert!(matches!(
        artifacts.diagnostics.as_slice(),
        [RendererDiagnostic::SpatialBlur {
            target_stride_px: 4,
            actual_stride_px: 4,
            traversal_overflow_texels: 0,
            capacity_overflow_samples: 0,
        }]
    ));

    let blurred_a =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]);
    let blurred_b =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(
        blurred_b.rgba8.chunks_exact(4).any(|pixel| pixel[3] > 0),
        "Spatial Blur should cross disconnected surfaces in the same mesh"
    );
    assert_premultiplied_red(&blurred_a);
    assert_premultiplied_red(&blurred_b);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]).rgba8,
        source_a
    );
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]).rgba8,
        source_b
    );

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]).rgba8,
        blurred_a.rgba8
    );
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]).rgba8,
        blurred_b.rgba8
    );
}

#[test]
fn layer_spatial_blur_respects_cross_mesh_setting() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(1)], false),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![
            solid_rgba(size, [128, 0, 0, 128]),
            solid_rgba(size, [0, 0, 0, 0]),
        ],
    );

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 12.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 60.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let isolated =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(isolated.rgba8.chunks_exact(4).all(|pixel| pixel[3] == 0));

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 12.0,
            cross_meshes: true,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 60.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let crossed = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(crossed.rgba8.chunks_exact(4).any(|pixel| pixel[3] > 0));
    assert_premultiplied_red(&crossed);
}

#[test]
fn layer_spatial_blur_respects_geometric_normal_limit() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(0)], true),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![
            solid_rgba(size, [128, 0, 0, 128]),
            solid_rgba(size, [0, 0, 0, 0]),
        ],
    );

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 12.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::SimilarNormals,
            maximum_normal_angle_degrees: 30.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let limited = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(limited.rgba8.chunks_exact(4).all(|pixel| pixel[3] == 0));

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 12.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::SimilarNormals,
            maximum_normal_angle_degrees: 180.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let unrestricted =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert!(unrestricted.rgba8.chunks_exact(4).any(|pixel| pixel[3] > 0));
    assert_premultiplied_red(&unrestricted);
}

#[test]
fn layer_spatial_blur_weights_samples_by_world_area() {
    let sizes = [[16, 16], [32, 32], [16, 16]];
    let Some(mut engine) = gpu_engine(sizes[1]) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_three_material_mesh(),
        vec![
            MaterialSpec::new("A", sizes[0]),
            MaterialSpec::new("B", sizes[1]),
            MaterialSpec::new("C", sizes[2]),
        ],
        vec![
            solid_rgba(sizes[0], [128, 0, 0, 128]),
            solid_rgba(sizes[1], [0, 0, 128, 128]),
            solid_rgba(sizes[2], [0, 0, 0, 0]),
        ],
    );

    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 12.0,
            cross_meshes: true,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 60.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let target = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[2]);
    let pixel = pixel_at(&target, 4, 4);
    let maximum = pixel[0].max(pixel[2]);
    assert!(
        pixel[0] > 0 && pixel[2] > 0,
        "both equal-area sources should contribute"
    );
    assert!(
        pixel[0].abs_diff(pixel[2]) <= maximum.saturating_mul(2) / 5 + 2,
        "equal world areas should have similar influence despite different texture resolutions: {pixel:?}"
    );
    assert!(pixel[0] <= pixel[3].saturating_add(1));
    assert!(pixel[2] <= pixel[3].saturating_add(1));
}

#[test]
fn brush_paint_tool_input_e2e_commits_document_history_and_composite() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    set_paint_brush(&mut runtime, 0.25);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let view_size = [64, 64];
    let sample = crate::application::PointerSample::uv_with_pressure_at(
        Vec2::new(0.5, 0.5),
        Vec2::new(32.0, 32.0),
        view_size,
        1.0,
        0.0,
        InputModifiers::default(),
    );
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerDown(sample)))
        .unwrap();
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerUp(sample)))
        .unwrap();

    let (artifacts, project_changed) =
        execute_runtime_effects_with_project_change(&mut runtime, &mut engine)
            .expect("UV paint stroke should produce renderer commit artifacts");
    assert!(project_changed);
    assert_eq!(artifacts.surface_commits.len(), 1);
    assert!(runtime.state.can_undo());
    assert!(!runtime.state.can_redo());
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    assert!(runtime.state.is_tool_idle());

    let doc_pixel = center_pixel(&document_surface(&runtime, surface));
    let renderer_pixel = center_pixel(&read_surface(&mut engine, surface));
    let composite_pixel = center_pixel(&read_composite(&mut engine));
    assert_pixel_near(doc_pixel, [255, 0, 0, 255], 4);
    assert_pixel_near(renderer_pixel, [255, 0, 0, 255], 4);
    assert_pixel_near(composite_pixel, [255, 0, 0, 255], 4);
}

#[test]
fn decal_apply_view_pointer_e2e_commits_history_and_undo_redo() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    let _ = runtime.take_last_effect_metrics();
    runtime
        .dispatch(Command::SetDecalImage(Arc::new(DecalImageAsset {
            id: DecalImageId(1),
            file_name: "e2e-decal.png".to_owned(),
            size: [2, 2],
            rgba8: Arc::from(solid_rgba([2, 2], [255, 0, 0, 255])),
        })))
        .expect("decal image should be accepted");
    runtime
        .dispatch(Command::SetActiveTool(ToolId::SurfaceDecal))
        .expect("Decal tool should be selectable");

    let viewport_size = [96, 96];
    let (camera, view) = surface_test_view(viewport_size);
    runtime
        .dispatch(Command::SetCamera(camera))
        .expect("Decal renderer context should use the same camera as the view input");
    let point = world_to_screen(view.view_proj, Vec3::new(0.35, 0.35, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, point, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, point, 0.1, view))
        .unwrap();
    assert!(runtime.state.has_active_decal_session());

    runtime
        .dispatch(Command::ApplyActiveDecal)
        .expect("active Decal should be applicable");
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Decal Apply should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    let commit_rect = artifacts.surface_commits[0].rect;
    assert!(
        commit_rect.origin != [0, 0] || commit_rect.size != size,
        "small Decal Apply should not read back the full surface: {commit_rect:?}"
    );
    let metrics = runtime.take_last_effect_metrics();
    assert_eq!(metrics.full_readback_calls, 0);
    assert!(metrics.rect_readback_calls > 0);
    assert!(metrics.partial_composite_count > 0);
    assert_eq!(metrics.full_composite_count, 0);
    assert!(metrics.partial_composite_executions > 0);
    assert_eq!(metrics.full_composite_executions, 0);
    assert!(runtime.state.can_undo());
    assert!(!runtime.state.can_redo());
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    assert!(!runtime.state.has_active_decal_session());

    let applied = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&applied, 11, 11), [255, 0, 0, 255], 4);
    assert_pixel_near(
        pixel_at(&read_composite(&mut engine), 11, 11),
        [255, 0, 0, 255],
        4,
    );

    runtime.dispatch(Command::Undo).unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert!(runtime.state.can_redo());
    let undone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&undone, 11, 11), [0, 0, 0, 0], 0);

    runtime.dispatch(Command::Redo).unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert!(!runtime.state.can_redo());
    let redone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&redone, 11, 11), [255, 0, 0, 255], 4);
}

#[test]
fn surface_decal_apply_e2e_supports_tiny_mesh_world_scale() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let world_scale = 0.01;
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        scaled_single_triangle_mesh(world_scale),
        size,
        solid_rgba(size, [0, 0, 0, 0]),
    );
    runtime
        .dispatch(Command::SetDecalImage(Arc::new(DecalImageAsset {
            id: DecalImageId(4),
            file_name: "tiny-mesh-decal.png".to_owned(),
            size: [2, 2],
            rgba8: Arc::from(solid_rgba([2, 2], [255, 0, 0, 255])),
        })))
        .unwrap();
    runtime
        .dispatch(Command::SetActiveTool(ToolId::SurfaceDecal))
        .unwrap();

    let hit = SurfaceHit {
        world_pos: Vec3::new(world_scale * 0.35, world_scale * 0.35, 0.0),
        world_normal: Vec3::Z,
        uv: Vec2::splat(0.35),
        uv_edge_distance: 0.35,
        uv_paint_boundary_distance: 0.35,
        triangle_index: 0,
        material_index: 0.into(),
        mesh_id: MeshId(0),
        t: 1.0,
    };
    let scene_diagonal = runtime
        .state
        .document()
        .expect("tiny mesh document should be loaded")
        .mesh
        .scene_diagonal();
    let transform =
        DecalTransform::from_surface_hit(hit, Vec3::Z, Vec3::X, Vec3::Y, scene_diagonal, [2, 2])
            .expect("tiny mesh decal transform should remain valid");
    runtime
        .state
        .tool
        .set_session(ToolSession::Decal(DecalSession {
            transform,
            source_hit: hit,
            gesture: None,
            scene_visibility: Default::default(),
        }));

    runtime.dispatch(Command::ApplyActiveDecal).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("tiny mesh Surface Decal should commit");

    let applied = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&applied, 11, 11), [255, 0, 0, 255], 4);
}

#[test]
fn view_projection_decal_e2e_projects_current_view_to_visible_surface() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime
        .dispatch(Command::SetDecalImage(Arc::new(DecalImageAsset {
            id: DecalImageId(2),
            file_name: "view-projection.png".to_owned(),
            size: [2, 2],
            rgba8: Arc::from(solid_rgba([2, 2], [0, 255, 0, 255])),
        })))
        .unwrap();
    runtime
        .dispatch(Command::SetActiveTool(ToolId::ViewProjectionDecal))
        .unwrap();
    let viewport_size = [96, 96];
    let (camera, _) = surface_test_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    runtime
        .dispatch(Command::BeginViewProjectionDecal { viewport_size })
        .unwrap();
    runtime.dispatch(Command::ApplyActiveDecal).unwrap();

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("View Projection Decal should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    let applied = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(center_pixel(&applied), [0, 255, 0, 255], 4);
    assert!(runtime.state.can_undo());
    assert!(!runtime.state.has_active_decal_session());
}

#[test]
fn orthographic_view_projection_decal_e2e_projects_current_view() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime
        .dispatch(Command::SetDecalImage(Arc::new(DecalImageAsset {
            id: DecalImageId(3),
            file_name: "orthographic-view-projection.png".to_owned(),
            size: [2, 2],
            rgba8: Arc::from(solid_rgba([2, 2], [0, 0, 255, 255])),
        })))
        .unwrap();
    runtime
        .dispatch(Command::SetActiveTool(ToolId::ViewProjectionDecal))
        .unwrap();
    let viewport_size = [96, 96];
    let (mut camera, _) = surface_test_view(viewport_size);
    camera.set_projection(crate::core::camera::CameraProjection::Orthographic);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    runtime
        .dispatch(Command::BeginViewProjectionDecal { viewport_size })
        .unwrap();
    runtime.dispatch(Command::ApplyActiveDecal).unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Orthographic View Projection Decal should commit");
    let applied = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(center_pixel(&applied), [0, 0, 255, 255], 4);
}

#[test]
fn surface_paint_view_pointer_e2e_dilates_across_uv_island_edge() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    set_paint_brush(&mut runtime, 0.12);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let viewport_size = [64, 64];
    let (camera, view) = surface_test_view(viewport_size);
    runtime
        .dispatch(Command::SetCamera(camera))
        .expect("surface stroke renderer context should use the same camera as the view input");

    let stroke_px = world_to_screen(view.view_proj, Vec3::new(0.48, 0.48, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            stroke_px,
            0.0,
            view,
        ))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            stroke_px,
            0.1,
            view,
        ))
        .unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Surface paint stroke should produce renderer commit artifacts");

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    let dilated_pixel = pixel_at(&document_after, 16, 16);
    assert!(
        dilated_pixel[0] > 32 && dilated_pixel[3] > 32,
        "surface paint should dilate color just outside the UV island edge, got {dilated_pixel:?}"
    );
}

fn assert_surface_paint_view_pointer_uv_mirror_survives_commit(isolated: bool) {
    let size = [512, 512];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime
        .dispatch(Command::AddAdjustmentLayer {
            kind: AdjustmentKind::UvMirror,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let adjustment = runtime.state.active_layer_id();
    if isolated {
        runtime.dispatch(Command::AddGroup).unwrap();
        execute_runtime_effects(&mut runtime, &mut engine);
        let group = runtime.state.active_layer_id();
        runtime
            .dispatch(Command::MoveLayer {
                layer_id: surface.layer_id,
                new_parent: group,
                new_index: 0,
            })
            .unwrap();
        execute_runtime_effects(&mut runtime, &mut engine);
        runtime
            .dispatch(Command::MoveLayer {
                layer_id: adjustment,
                new_parent: group,
                new_index: 1,
            })
            .unwrap();
        execute_runtime_effects(&mut runtime, &mut engine);
    }
    runtime
        .dispatch(Command::SelectLayer {
            layer_id: surface.layer_id,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    set_paint_brush(&mut runtime, 0.015);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let viewport_size = [96, 96];
    let (camera, view) = surface_test_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let down = world_to_screen(view.view_proj, Vec3::new(0.72, 0.18, 0.0), viewport_size);
    let moved = world_to_screen(view.view_proj, Vec3::new(0.75, 0.18, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, down, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Move, moved, 0.1, view))
        .unwrap();
    execute_runtime_effects_with_plan_mutator(&mut runtime, &mut engine, |plan| {
        assert!(plan.edit_commands.iter().any(|command| matches!(
            command,
            crate::renderer::EditCommand::Stroke(crate::renderer::StrokeCommand::Begin {
                target: crate::renderer::StrokeTarget::Surface { .. },
                ..
            })
        )));
        assert!(plan.edit_commands.iter().any(|command| matches!(
            command,
            crate::renderer::EditCommand::Stroke(crate::renderer::StrokeCommand::AddDabs {
                dabs: crate::renderer::StrokeDabPayload::Surface { .. },
                ..
            })
        )));
    });

    let contains_red = |snapshot: &Rgba8Snapshot, x_range: std::ops::Range<u32>| {
        (0..size[1]).any(|y| {
            x_range.clone().any(|x| {
                let pixel = pixel_at(snapshot, x, y);
                pixel[0] > 32 && pixel[3] > 32
            })
        })
    };
    let assert_source_and_mirror = |phase: &str, snapshot: &Rgba8Snapshot| {
        assert!(
            contains_red(snapshot, size[0] * 13 / 20..size[0] * 17 / 20),
            "{phase}: source side must contain the Surface stroke"
        );
        assert!(
            contains_red(snapshot, size[0] * 3 / 20..size[0] * 7 / 20),
            "{phase}: UV Mirror destination must contain the Surface stroke"
        );
    };
    let preview = read_composite(&mut engine);
    assert_source_and_mirror("active preview", &preview);

    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, moved, 0.2, view))
        .unwrap();
    execute_runtime_effects_with_plan_mutator(&mut runtime, &mut engine, |plan| {
        let damage = plan
            .edit_commands
            .iter()
            .find_map(|command| match command {
                crate::renderer::EditCommand::Stroke(crate::renderer::StrokeCommand::End {
                    damage,
                }) => damage.as_ref(),
                _ => None,
            })
            .expect("Surface stroke End must carry explicit partial damage");
        assert!(!damage.pixels.is_empty());
        for pixel in &damage.pixels {
            assert!(
                pixel.rect.origin != [0, 0] || pixel.rect.size != size,
                "test must exercise partial Surface commit damage, got {:?}",
                pixel.rect
            );
            let source_start = pixel.rect.origin[0];
            let source_end = source_start + pixel.rect.size[0];
            let mirrored_start = size[0] - source_end;
            let mirrored_end = size[0] - source_start;
            assert!(
                source_end <= mirrored_start || mirrored_end <= source_start,
                "source damage {:?} must not overlap its UV Mirror destination {}..{}",
                pixel.rect,
                mirrored_start,
                mirrored_end
            );
        }
    })
    .expect("Surface paint stroke should produce renderer commit artifacts");

    let committed = read_composite(&mut engine);
    assert!(
        contains_red(&committed, size[0] * 13 / 20..size[0] * 17 / 20),
        "after commit: source side must contain the Surface stroke"
    );
    assert!(
        contains_red(&committed, size[0] * 3 / 20..size[0] * 7 / 20),
        "after commit: UV Mirror destination must contain the Surface stroke"
    );
}

#[test]
fn surface_paint_view_pointer_uv_mirror_survives_commit() {
    assert_surface_paint_view_pointer_uv_mirror_survives_commit(false);
}

#[test]
fn surface_paint_view_pointer_uv_mirror_inside_isolated_group_survives_commit() {
    assert_surface_paint_view_pointer_uv_mirror_survives_commit(true);
}

#[test]
fn orthographic_surface_paint_view_pointer_e2e_commits_at_hit() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    set_paint_brush(&mut runtime, 0.12);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let viewport_size = [64, 64];
    let (camera, view) = surface_orthographic_test_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let stroke_px = world_to_screen(view.view_proj, Vec3::new(0.5, 0.5, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            stroke_px,
            0.0,
            view,
        ))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            stroke_px,
            0.1,
            view,
        ))
        .unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Orthographic surface paint should produce renderer commit artifacts");
    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(center_pixel(&document_after), [255, 0, 0, 255], 12);
}

#[test]
fn surface_paint_x_mirror_e2e_updates_both_uv_islands_with_one_history() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        symmetric_panel_mesh(),
        size,
        solid_rgba(size, [0, 0, 0, 0]),
    );
    set_paint_brush(&mut runtime, 0.15);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::SetSurfaceMirrorXPlane(0.05))
        .unwrap();
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(true))
        .unwrap();

    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let point = world_to_screen(view.view_proj, Vec3::new(0.5, 0.0, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, point, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, point, 0.1, view))
        .unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mirrored Surface paint should produce renderer commit artifacts");
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    let painted = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    let left = pixel_at(&painted, 7, 16);
    let right = pixel_at(&painted, 24, 16);
    assert!(
        left[0] > 32 && left[3] > 32,
        "left mirror island was not painted: {left:?}"
    );
    assert!(
        right[0] > 32 && right[3] > 32,
        "right primary island was not painted: {right:?}"
    );

    runtime.dispatch(Command::Undo).unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    let undone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(pixel_at(&undone, 7, 16), [0, 0, 0, 0]);
    assert_eq!(pixel_at(&undone, 24, 16), [0, 0, 0, 0]);

    runtime.dispatch(Command::Redo).unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    let redone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(pixel_at(&redone, 7, 16)[0] > 32);
    assert!(pixel_at(&redone, 24, 16)[0] > 32);
}

#[test]
fn surface_blur_x_mirror_e2e_updates_both_uv_islands() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial = vertical_stripes_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]);
    let (mut runtime, surface) =
        load_runtime_with_mesh_pixels(&mut engine, symmetric_panel_mesh(), size, initial.clone());
    set_blur_brush(&mut runtime, 0.2);
    runtime
        .dispatch(Command::SetSurfaceMirrorXPlane(0.05))
        .unwrap();
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(true))
        .unwrap();
    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let point = world_to_screen(view.view_proj, Vec3::new(0.5, 0.0, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, point, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, point, 0.1, view))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mirrored Surface blur should produce renderer commit artifacts");

    let blurred = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_ne!(pixel_at(&blurred, 7, 16), [0, 0, 255, 255]);
    assert_ne!(pixel_at(&blurred, 24, 16), [255, 0, 0, 255]);
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
}

#[test]
fn surface_smudge_x_mirror_e2e_updates_both_uv_islands() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial = vertical_stripes_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]);
    let (mut runtime, surface) =
        load_runtime_with_mesh_pixels(&mut engine, symmetric_panel_mesh(), size, initial.clone());
    set_smudge_brush(&mut runtime, 0.22);
    runtime
        .dispatch(Command::SetSurfaceMirrorXPlane(0.05))
        .unwrap();
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(true))
        .unwrap();
    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let points: Vec<Vec2> = [0.3, 0.4, 0.5, 0.6, 0.7]
        .into_iter()
        .map(|x| world_to_screen(view.view_proj, Vec3::new(x, 0.0, 0.0), viewport_size))
        .collect();
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            points[0],
            0.0,
            view,
        ))
        .unwrap();
    for (index, point) in points.iter().copied().enumerate().skip(1) {
        runtime
            .dispatch(view_pointer_event(
                ViewPointerPhase::Move,
                point,
                index as f64 * 0.1,
                view,
            ))
            .unwrap();
    }
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            *points.last().unwrap(),
            0.6,
            view,
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mirrored Surface smudge should produce renderer commit artifacts");

    let smeared = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    let island_changed = |x_start: u32, x_end: u32| {
        (0..size[1]).any(|y| {
            (x_start..x_end).any(|x| {
                let index = ((y * size[0] + x) * 4) as usize;
                smeared.rgba8[index..index + 4] != initial[index..index + 4]
            })
        })
    };
    assert!(island_changed(0, 15), "left mirror island was not smudged");
    assert!(
        island_changed(17, 32),
        "right primary island was not smudged"
    );
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
}

fn asymmetric_island_patterns(texture_size: [u32; 2]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(texture_size[0] as usize * texture_size[1] as usize * 4);
    for y in 0..texture_size[1] {
        for x in 0..texture_size[0] {
            let rgba = if x < texture_size[0] / 2 {
                if (x + y) % 3 == 0 {
                    [255, 32, 0, 255]
                } else {
                    [0, 192, 48, 255]
                }
            } else if (x * 2 + y) % 5 < 2 {
                [16, 32, 255, 255]
            } else {
                [240, 192, 0, 255]
            };
            pixels.extend_from_slice(&rgba);
        }
    }
    pixels
}

fn render_symmetric_brush_stroke(
    initial: &[u8],
    configure_brush: fn(&mut ApplicationRuntime, f32),
    radius_world: f32,
    mirror: bool,
    world_xs: &[f32],
    reverse_projection_order: bool,
    retained_projection: Option<SurfaceProjectionId>,
) -> Option<Rgba8Snapshot> {
    let size = [32, 32];
    let mut engine = gpu_engine(size)?;
    let (mut runtime, surface) =
        load_runtime_with_mesh_pixels(&mut engine, symmetric_panel_mesh(), size, initial.to_vec());
    configure_brush(&mut runtime, radius_world);
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(mirror))
        .unwrap();
    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let points: Vec<Vec2> = world_xs
        .iter()
        .map(|x| world_to_screen(view.view_proj, Vec3::new(*x, 0.0, 0.0), viewport_size))
        .collect();
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            points[0],
            0.0,
            view,
        ))
        .unwrap();
    for (index, point) in points.iter().copied().enumerate().skip(1) {
        runtime
            .dispatch(view_pointer_event(
                ViewPointerPhase::Move,
                point,
                index as f64 * 0.1,
                view,
            ))
            .unwrap();
    }
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            *points.last().unwrap(),
            points.len() as f64 * 0.1,
            view,
        ))
        .unwrap();
    execute_runtime_effects_with_plan_mutator(&mut runtime, &mut engine, |plan| {
        if reverse_projection_order {
            reverse_surface_projection_batches(plan);
        }
        if let Some(projection_id) = retained_projection {
            retain_surface_projection_batches(plan, projection_id);
        }
    })
    .expect("surface brush stroke should produce renderer commit artifacts");
    Some(assert_snapshot_matches_document_and_renderer(
        &runtime,
        &mut engine,
        surface,
    ))
}

fn assert_region_eq(
    actual: &Rgba8Snapshot,
    expected: &Rgba8Snapshot,
    x_range: std::ops::Range<u32>,
    label: &str,
) {
    for y in 0..actual.texture_size[1] {
        for x in x_range.clone() {
            assert_eq!(
                pixel_at(actual, x, y),
                pixel_at(expected, x, y),
                "{label} differs at ({x}, {y})"
            );
        }
    }
}

#[test]
fn surface_mirror_projection_order_is_pixel_identical_for_disjoint_uv_islands() {
    let initial = asymmetric_island_patterns([32, 32]);
    for (name, brush, radius, points) in [
        (
            "blur",
            set_blur_brush as fn(&mut ApplicationRuntime, f32),
            0.18,
            vec![0.5],
        ),
        (
            "smudge",
            set_smudge_brush as fn(&mut ApplicationRuntime, f32),
            0.18,
            vec![0.3, 0.4, 0.5, 0.6],
        ),
    ] {
        let normal =
            render_symmetric_brush_stroke(&initial, brush, radius, true, &points, false, None)
                .expect("GPU should remain available");
        let reversed =
            render_symmetric_brush_stroke(&initial, brush, radius, true, &points, true, None)
                .expect("GPU should remain available");
        assert_region_eq(&normal, &reversed, 0..15, &format!("{name} mirror island"));
        assert_region_eq(
            &normal,
            &reversed,
            17..32,
            &format!("{name} primary island"),
        );
    }
}

#[test]
fn surface_mirror_batch_source_matches_each_projection_run_in_isolation() {
    let initial = asymmetric_island_patterns([32, 32]);
    for (name, brush, radius, primary) in [
        (
            "blur",
            set_blur_brush as fn(&mut ApplicationRuntime, f32),
            0.18,
            vec![0.5],
        ),
        (
            "smudge",
            set_smudge_brush as fn(&mut ApplicationRuntime, f32),
            0.18,
            vec![0.3, 0.4, 0.5, 0.6],
        ),
    ] {
        let mirrored =
            render_symmetric_brush_stroke(&initial, brush, radius, true, &primary, false, None)
                .expect("GPU should remain available");
        let primary_alone = render_symmetric_brush_stroke(
            &initial,
            brush,
            radius,
            true,
            &primary,
            false,
            Some(SurfaceProjectionId::Primary),
        )
        .expect("GPU should remain available");
        let mirror_alone = render_symmetric_brush_stroke(
            &initial,
            brush,
            radius,
            true,
            &primary,
            false,
            Some(SurfaceProjectionId::MirrorX),
        )
        .expect("GPU should remain available");
        assert_region_eq(
            &mirrored,
            &mirror_alone,
            0..15,
            &format!("{name} mirror projection"),
        );
        assert_region_eq(
            &mirrored,
            &primary_alone,
            17..32,
            &format!("{name} primary projection"),
        );
    }
}

#[test]
fn surface_mirror_projection_reaches_opposite_island_without_primary_projection() {
    let initial = asymmetric_island_patterns([32, 32]);
    let initial_snapshot = Rgba8Snapshot::new([32, 32], [0, 0], [32, 32], initial.clone())
        .expect("test pixels should form a snapshot");
    let mirror_only = render_symmetric_brush_stroke(
        &initial,
        set_blur_brush,
        0.18,
        true,
        &[0.5],
        false,
        Some(SurfaceProjectionId::MirrorX),
    )
    .expect("GPU should be available");

    assert!(
        (0..32).any(|y| (0..15)
            .any(|x| { pixel_at(&mirror_only, x, y) != pixel_at(&initial_snapshot, x, y) })),
        "the opposite UV island must be editable by the mirror projection alone"
    );
    assert_region_eq(
        &mirror_only,
        &initial_snapshot,
        17..32,
        "primary island without a primary projection",
    );
}

#[test]
fn lasso_paint_tool_input_e2e_fills_polygon_only() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime.state.set_active_tool(ToolId::LassoPaint);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let view_size = [64, 64];
    stroke_uv(
        &mut runtime,
        &[
            Vec2::new(0.20, 0.20),
            Vec2::new(0.70, 0.20),
            Vec2::new(0.20, 0.70),
            Vec2::new(0.20, 0.20),
        ],
        view_size,
    );

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("UV lasso paint should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 8, 8), [255, 0, 0, 255], 4);
    assert_pixel_near(pixel_at(&document_after, 24, 24), [0, 0, 0, 0], 4);
}

#[test]
fn lasso_erase_tool_input_e2e_erases_polygon_only() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [255, 0, 0, 255]);
    runtime.state.set_active_tool(ToolId::LassoErase);

    let view_size = [64, 64];
    stroke_uv(
        &mut runtime,
        &[
            Vec2::new(0.20, 0.20),
            Vec2::new(0.70, 0.20),
            Vec2::new(0.20, 0.70),
            Vec2::new(0.20, 0.20),
        ],
        view_size,
    );

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("UV lasso erase should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    assert_eq!(runtime.state.history.undo_stack.len(), 1);

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 8, 8), [0, 0, 0, 0], 4);
    assert_pixel_near(pixel_at(&document_after, 24, 24), [255, 0, 0, 255], 4);
}

#[test]
fn lasso_paint_view_pointer_e2e_dilates_across_uv_island_edge() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime.state.set_active_tool(ToolId::LassoPaint);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let viewport_size = [64, 64];
    let (_camera, view) = surface_test_view(viewport_size);
    let points = [
        Vec3::new(0.40, 0.40, 0.0),
        Vec3::new(0.52, 0.40, 0.0),
        Vec3::new(0.52, 0.52, 0.0),
        Vec3::new(0.40, 0.52, 0.0),
        Vec3::new(0.40, 0.40, 0.0),
    ]
    .map(|world| world_to_screen(view.view_proj, world, viewport_size));
    lasso_view_pointer_path(&mut runtime, &points, view);

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("3D lasso paint should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    let seam_pixel = pixel_at(&document_after, 16, 16);
    assert!(
        seam_pixel[0] > 32 && seam_pixel[3] > 32,
        "3D lasso paint should dilate color into UV seam padding, got {seam_pixel:?}"
    );
    assert_pixel_near(pixel_at(&document_after, 2, 2), [0, 0, 0, 0], 4);
}

#[test]
fn rectangle_paint_view_pointer_e2e_projects_viewport_rect_to_surface() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime.state.set_active_tool(ToolId::RectanglePaint);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let viewport_size = [64, 64];
    let (_camera, view) = surface_test_view(viewport_size);
    let start = world_to_screen(view.view_proj, Vec3::new(0.25, 0.25, 0.0), viewport_size);
    let end = world_to_screen(view.view_proj, Vec3::new(0.75, 0.75, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, start, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, end, 0.1, view))
        .unwrap();

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("3D rectangle paint should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 8, 8), [255, 0, 0, 255], 4);
    assert_pixel_near(pixel_at(&document_after, 2, 2), [0, 0, 0, 0], 4);
}

#[test]
fn rectangle_paint_x_mirror_e2e_updates_both_uv_islands() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        symmetric_panel_mesh(),
        size,
        solid_rgba(size, [0, 0, 0, 0]),
    );
    runtime.state.set_active_tool(ToolId::RectanglePaint);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(true))
        .unwrap();

    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let start = world_to_screen(view.view_proj, Vec3::new(0.35, -0.30, 0.0), viewport_size);
    let end = world_to_screen(view.view_proj, Vec3::new(0.65, 0.30, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, start, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, end, 0.1, view))
        .unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mirrored rectangle paint should produce renderer commit artifacts");
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    let painted = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(pixel_at(&painted, 7, 16)[0] > 32);
    assert!(pixel_at(&painted, 24, 16)[0] > 32);
    assert_eq!(pixel_at(&painted, 1, 1), [0, 0, 0, 0]);
}

#[test]
fn rectangle_erase_x_mirror_e2e_updates_both_uv_islands() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        symmetric_panel_mesh(),
        size,
        solid_rgba(size, [255, 0, 0, 255]),
    );
    runtime.state.set_active_tool(ToolId::RectangleErase);
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(true))
        .unwrap();

    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let start = world_to_screen(view.view_proj, Vec3::new(0.35, -0.30, 0.0), viewport_size);
    let end = world_to_screen(view.view_proj, Vec3::new(0.65, 0.30, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, start, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, end, 0.1, view))
        .unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mirrored rectangle erase should produce renderer commit artifacts");
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    let erased = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(pixel_at(&erased, 7, 16)[3] < 32);
    assert!(pixel_at(&erased, 24, 16)[3] < 32);
    assert_eq!(pixel_at(&erased, 1, 1), [255, 0, 0, 255]);
}

#[test]
fn lasso_paint_x_mirror_e2e_updates_both_uv_islands() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        symmetric_panel_mesh(),
        size,
        solid_rgba(size, [0, 0, 0, 0]),
    );
    runtime.state.set_active_tool(ToolId::LassoPaint);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(true))
        .unwrap();

    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let points = [
        Vec3::new(0.35, -0.30, 0.0),
        Vec3::new(0.65, -0.30, 0.0),
        Vec3::new(0.65, 0.30, 0.0),
        Vec3::new(0.35, 0.30, 0.0),
        Vec3::new(0.35, -0.30, 0.0),
    ]
    .map(|world| world_to_screen(view.view_proj, world, viewport_size));
    lasso_view_pointer_path(&mut runtime, &points, view);

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mirrored lasso paint should produce renderer commit artifacts");
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    let painted = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(pixel_at(&painted, 7, 16)[0] > 32);
    assert!(pixel_at(&painted, 24, 16)[0] > 32);
    assert_eq!(pixel_at(&painted, 1, 1), [0, 0, 0, 0]);
}

#[test]
fn lasso_erase_x_mirror_e2e_updates_both_uv_islands() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        symmetric_panel_mesh(),
        size,
        solid_rgba(size, [255, 0, 0, 255]),
    );
    runtime.state.set_active_tool(ToolId::LassoErase);
    runtime
        .dispatch(Command::SetSurfaceMirrorXEnabled(true))
        .unwrap();

    let viewport_size = [96, 96];
    let (camera, view) = symmetric_panel_view(viewport_size);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let points = [
        Vec3::new(0.35, -0.30, 0.0),
        Vec3::new(0.65, -0.30, 0.0),
        Vec3::new(0.65, 0.30, 0.0),
        Vec3::new(0.35, 0.30, 0.0),
        Vec3::new(0.35, -0.30, 0.0),
    ]
    .map(|world| world_to_screen(view.view_proj, world, viewport_size));
    lasso_view_pointer_path(&mut runtime, &points, view);

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mirrored lasso erase should produce renderer commit artifacts");
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    let erased = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(pixel_at(&erased, 7, 16)[3] < 32);
    assert!(pixel_at(&erased, 24, 16)[3] < 32);
    assert_eq!(pixel_at(&erased, 1, 1), [255, 0, 0, 255]);
}

#[test]
fn lasso_selection_view_pointer_e2e_dilates_projection_mask_across_uv_island_edge() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime.state.set_active_tool(ToolId::LassoSelection);

    let viewport_size = [64, 64];
    let (_camera, view) = surface_test_view(viewport_size);
    let points = [
        Vec3::new(0.40, 0.40, 0.0),
        Vec3::new(0.52, 0.40, 0.0),
        Vec3::new(0.52, 0.52, 0.0),
        Vec3::new(0.40, 0.52, 0.0),
        Vec3::new(0.40, 0.40, 0.0),
    ]
    .map(|world| world_to_screen(view.view_proj, world, viewport_size));
    lasso_view_pointer_path(&mut runtime, &points, view);

    let selection_artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("3D lasso selection should produce renderer commit artifacts");
    assert!(selection_artifacts.surface_commits.is_empty());
    assert_eq!(selection_artifacts.selection_after_commits.len(), 1);

    let selection = &selection_artifacts.selection_after_commits[0];
    assert_eq!(selection.texture_size, size);
    let seam_pixel = selection.r8[(16 * size[0] + 16) as usize];
    assert!(
        seam_pixel > 0,
        "3D lasso selection should dilate the projected mask into UV seam padding, got {seam_pixel}"
    );
    let outside_pixel = selection.r8[(2 * size[0] + 2) as usize];
    assert_eq!(
        outside_pixel, 0,
        "dilation should not select distant UV pixels"
    );
}

#[test]
fn rectangle_selection_view_pointer_e2e_projects_viewport_rect_to_selection_mask() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime.state.set_active_tool(ToolId::RectangleSelection);

    let viewport_size = [64, 64];
    let (_camera, view) = surface_test_view(viewport_size);
    let start = world_to_screen(view.view_proj, Vec3::new(0.25, 0.25, 0.0), viewport_size);
    let end = world_to_screen(view.view_proj, Vec3::new(0.75, 0.75, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, start, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, end, 0.1, view))
        .unwrap();

    let selection_artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("3D rectangle selection should produce renderer commit artifacts");
    assert!(selection_artifacts.surface_commits.is_empty());
    assert_eq!(selection_artifacts.selection_after_commits.len(), 1);

    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::FillMaterial {
            material_index: 0,
            opacity: 1.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected fill should produce renderer commit artifacts");

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 8, 8), [255, 0, 0, 255], 4);
    assert_pixel_near(pixel_at(&document_after, 2, 2), [0, 0, 0, 0], 4);
}

#[test]
fn rectangle_selection_view_pointer_e2e_dilates_projection_mask_across_uv_island_edge() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime.state.set_active_tool(ToolId::RectangleSelection);

    let viewport_size = [64, 64];
    let (_camera, view) = surface_test_view(viewport_size);
    let start = world_to_screen(view.view_proj, Vec3::new(0.40, 0.40, 0.0), viewport_size);
    let end = world_to_screen(view.view_proj, Vec3::new(0.52, 0.52, 0.0), viewport_size);
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, start, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, end, 0.1, view))
        .unwrap();

    let selection_artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("3D rectangle selection should produce renderer commit artifacts");
    assert_eq!(selection_artifacts.selection_after_commits.len(), 1);

    let selection = &selection_artifacts.selection_after_commits[0];
    assert_eq!(selection.texture_size, size);
    let seam_pixel = selection.r8[(16 * size[0] + 16) as usize];
    assert!(
        seam_pixel > 0,
        "3D rectangle selection should dilate the projected mask into UV seam padding, got {seam_pixel}"
    );
    let outside_pixel = selection.r8[(2 * size[0] + 2) as usize];
    assert_eq!(
        outside_pixel, 0,
        "dilation should not select distant UV pixels"
    );
}

#[test]
fn surface_smudge_view_pointer_e2e_commits_document_and_history() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial = vertical_stripes_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]);
    let initial_snapshot = initial.clone();
    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, initial);
    set_smudge_brush(&mut runtime, 0.75);

    let viewport_size = [64, 64];
    let (camera, view) = surface_test_view(viewport_size);
    runtime
        .dispatch(Command::SetCamera(camera))
        .expect("surface stroke renderer context should use the same camera as the view input");
    let stroke_world = [
        Vec3::new(0.30, 0.20, 0.0),
        Vec3::new(0.40, 0.20, 0.0),
        Vec3::new(0.50, 0.20, 0.0),
        Vec3::new(0.60, 0.20, 0.0),
        Vec3::new(0.70, 0.20, 0.0),
    ];
    let stroke_px: Vec<Vec2> = stroke_world
        .into_iter()
        .map(|world| world_to_screen(view.view_proj, world, viewport_size))
        .collect();

    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            stroke_px[0],
            0.0,
            view,
        ))
        .unwrap();
    for (index, position) in stroke_px.iter().copied().enumerate().skip(1) {
        runtime
            .dispatch(view_pointer_event(
                ViewPointerPhase::Move,
                position,
                index as f64 * 0.1,
                view,
            ))
            .unwrap();
    }
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            *stroke_px.last().unwrap(),
            0.6,
            view,
        ))
        .unwrap();

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("Surface smudge stroke should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    assert!(runtime.state.can_undo());
    assert_eq!(runtime.state.history.undo_stack.len(), 1);

    let document_after = document_surface(&runtime, surface);
    let renderer_after = read_surface(&mut engine, surface);
    assert_ne!(
        document_after.rgba8, initial_snapshot,
        "document should receive a visible surface smudge edit"
    );
    assert_eq!(document_after.rgba8, renderer_after.rgba8);
}

#[test]
fn surface_blur_view_pointer_e2e_supports_tiny_imported_mesh() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let world_scale = 0.01;
    let initial = vertical_stripes_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]);
    let initial_snapshot = initial.clone();
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        scaled_single_triangle_mesh(world_scale),
        size,
        initial,
    );
    set_blur_brush(&mut runtime, world_scale * 0.2);

    let viewport_size = [512, 512];
    runtime
        .dispatch(Command::SetCamera(surface_test_camera()))
        .unwrap();
    let camera = fit_camera_to_document(&runtime.state)
        .expect("a camera should be fitted after loading the tiny mesh");
    let view_proj = camera.view_projection_matrix(1.0);
    let view = ViewportInputContext {
        size: viewport_size,
        view_proj,
        inv_view_proj: view_proj.inverse(),
        camera_world: camera.position().into(),
    };
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let point = world_to_screen(
        view.view_proj,
        Vec3::new(world_scale * 0.2, world_scale * 0.2, 0.0),
        viewport_size,
    );

    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Down, point, 0.0, view))
        .unwrap();
    runtime
        .dispatch(view_pointer_event(ViewPointerPhase::Up, point, 0.1, view))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("tiny-mesh Surface blur should produce renderer commit artifacts");

    let blurred = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(
        blurred.rgba8 != initial_snapshot,
        "Surface blur should visibly edit a tiny imported mesh from 3D View"
    );
}

#[test]
fn surface_smudge_view_pointer_e2e_supports_tiny_imported_mesh() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let world_scale = 0.01;
    let initial = vertical_stripes_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]);
    let initial_snapshot = initial.clone();
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        scaled_single_triangle_mesh(world_scale),
        size,
        initial,
    );
    set_smudge_brush(&mut runtime, world_scale * 0.2);

    let viewport_size = [512, 512];
    runtime
        .dispatch(Command::SetCamera(surface_test_camera()))
        .unwrap();
    let camera = fit_camera_to_document(&runtime.state)
        .expect("a camera should be fitted after loading the tiny mesh");
    let view_proj = camera.view_projection_matrix(1.0);
    let view = ViewportInputContext {
        size: viewport_size,
        view_proj,
        inv_view_proj: view_proj.inverse(),
        camera_world: camera.position().into(),
    };
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let points: Vec<Vec2> = [0.15, 0.25, 0.35]
        .into_iter()
        .map(|x| {
            world_to_screen(
                view.view_proj,
                Vec3::new(world_scale * x, world_scale * 0.2, 0.0),
                viewport_size,
            )
        })
        .collect();

    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            points[0],
            0.0,
            view,
        ))
        .unwrap();
    for (index, point) in points.iter().copied().enumerate().skip(1) {
        runtime
            .dispatch(view_pointer_event(
                ViewPointerPhase::Move,
                point,
                index as f64 * 0.1,
                view,
            ))
            .unwrap();
    }
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            *points.last().unwrap(),
            0.3,
            view,
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("tiny-mesh Surface smudge should produce renderer commit artifacts");

    let smudged = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert!(
        smudged.rgba8 != initial_snapshot,
        "Surface smudge should visibly edit a tiny imported mesh from 3D View"
    );
}

fn run_tiny_mesh_backface_surface_brush(
    configure_brush: fn(&mut ApplicationRuntime, f32),
    stroke_world_xs: &[f32],
) -> Option<bool> {
    let size = [32, 32];
    let mut engine = gpu_engine(size)?;
    let world_scale = 0.01;
    let initial = vertical_stripes_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]);
    let initial_snapshot = initial.clone();
    let (mut runtime, surface) = load_runtime_with_mesh_pixels(
        &mut engine,
        scaled_single_triangle_mesh(world_scale),
        size,
        initial,
    );
    configure_brush(&mut runtime, world_scale * 0.2);

    let viewport_size = [512, 512];
    let mut camera = surface_test_camera();
    camera.orientation = Quat::from_rotation_y(std::f32::consts::PI);
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let camera = fit_camera_to_document(&runtime.state)
        .expect("a camera should be fitted after loading the tiny mesh");
    let view_proj = camera.view_projection_matrix(1.0);
    let view = ViewportInputContext {
        size: viewport_size,
        view_proj,
        inv_view_proj: view_proj.inverse(),
        camera_world: camera.position().into(),
    };
    runtime.dispatch(Command::SetCamera(camera)).unwrap();
    let points = stroke_world_xs
        .iter()
        .map(|x| {
            world_to_screen(
                view.view_proj,
                Vec3::new(world_scale * x, world_scale * 0.2, 0.0),
                viewport_size,
            )
        })
        .collect::<Vec<_>>();

    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Down,
            points[0],
            0.0,
            view,
        ))
        .unwrap();
    for (index, point) in points.iter().copied().enumerate().skip(1) {
        runtime
            .dispatch(view_pointer_event(
                ViewPointerPhase::Move,
                point,
                index as f64 * 0.1,
                view,
            ))
            .unwrap();
    }
    runtime
        .dispatch(view_pointer_event(
            ViewPointerPhase::Up,
            *points.last().unwrap(),
            points.len() as f64 * 0.1,
            view,
        ))
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("backface Surface brush should produce renderer commit artifacts");

    Some(read_surface(&mut engine, surface).rgba8 != initial_snapshot)
}

#[test]
fn surface_blur_view_pointer_e2e_edits_backface() {
    let Some(changed) = run_tiny_mesh_backface_surface_brush(set_blur_brush, &[0.2]) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    assert!(changed, "Surface blur should edit a visible backface");
}

#[test]
fn surface_smudge_view_pointer_e2e_edits_backface() {
    let Some(changed) = run_tiny_mesh_backface_surface_brush(set_smudge_brush, &[0.15, 0.25, 0.35])
    else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    assert!(changed, "Surface smudge should edit a visible backface");
}

#[test]
fn undo_redo_after_gpu_fill_restores_renderer_and_document() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    runtime
        .dispatch(Command::SetActiveTool(ToolId::FillMaterial))
        .unwrap();
    runtime
        .dispatch(Command::SetCurrentColor([0.0, 1.0, 0.0]))
        .unwrap();

    runtime
        .dispatch(Command::FillMaterial {
            material_index: 0,
            opacity: 1.0,
        })
        .unwrap();
    let fill_artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("fill material should produce renderer commit artifacts");
    assert_eq!(fill_artifacts.surface_commits.len(), 1);
    assert!(runtime.state.can_undo());
    assert_pixel_near(
        center_pixel(&document_surface(&runtime, surface)),
        [0, 255, 0, 255],
        3,
    );
    assert_pixel_near(
        center_pixel(&read_surface(&mut engine, surface)),
        [0, 255, 0, 255],
        3,
    );

    runtime.dispatch(Command::Undo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "undo uploads restored document tiles and should not require renderer readback finalization"
    );
    assert!(runtime.state.can_redo());
    assert_pixel_near(
        center_pixel(&document_surface(&runtime, surface)),
        [0, 0, 0, 0],
        0,
    );
    assert_pixel_near(
        center_pixel(&read_surface(&mut engine, surface)),
        [0, 0, 0, 0],
        0,
    );

    runtime.dispatch(Command::Redo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "redo uploads restored document tiles and should not require renderer readback finalization"
    );
    assert!(runtime.state.can_undo());
    assert!(!runtime.state.can_redo());
    assert_pixel_near(
        center_pixel(&document_surface(&runtime, surface)),
        [0, 255, 0, 255],
        3,
    );
    assert_pixel_near(
        center_pixel(&read_surface(&mut engine, surface)),
        [0, 255, 0, 255],
        3,
    );
}

#[test]
fn uv_smudge_tool_input_e2e_commits_document_history_and_renderer() {
    let size = [32, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial = vertical_stripes_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]);
    let initial_snapshot = initial.clone();
    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, initial);
    set_smudge_brush(&mut runtime, 0.20);

    stroke_uv(
        &mut runtime,
        &[
            Vec2::new(0.20, 0.50),
            Vec2::new(0.35, 0.50),
            Vec2::new(0.50, 0.50),
            Vec2::new(0.65, 0.50),
            Vec2::new(0.80, 0.50),
        ],
        [128, 64],
    );

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("UV smudge stroke should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    assert!(runtime.state.can_undo());
    assert_eq!(runtime.state.history.undo_stack.len(), 1);
    assert!(runtime.state.is_tool_idle());

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_ne!(
        document_after.rgba8, initial_snapshot,
        "UV smudge should create a visible edit when brush direction is determined by move samples"
    );
}

#[test]
fn add_layer_creates_each_material_surface_at_its_material_size() {
    let sizes = [[8, 12], [16, 10]];
    let Some(mut engine) = gpu_engine([16, 16]) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(0)], false),
        vec![
            MaterialSpec::new("A", sizes[0]),
            MaterialSpec::new("B", sizes[1]),
        ],
        vec![
            solid_rgba(sizes[0], [0, 0, 0, 0]),
            solid_rgba(sizes[1], [0, 0, 0, 0]),
        ],
    );

    runtime.dispatch(Command::AddLayer).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let layer_id = runtime.state.document.active_layer_id();

    for (material_index, expected_size) in sizes.into_iter().enumerate() {
        let snapshot = assert_snapshot_matches_document_and_renderer(
            &runtime,
            &mut engine,
            PaintSurfaceId::raster(material_index.into(), layer_id),
        );
        assert_eq!(snapshot.texture_size, expected_size);
    }
}

#[test]
fn layer_switch_gpu_fill_undo_redo_e2e_restores_renderer_document_and_composite() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, bottom_surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    let bottom_layer = runtime.state.document.active_layer_id();

    runtime.dispatch(Command::AddLayer).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "adding a layer should upload/sync renderer state without readback finalization"
    );
    let top_layer = runtime.state.document.active_layer_id();
    let top_surface = PaintSurfaceId::raster(0.into(), top_layer);
    assert_ne!(bottom_surface, top_surface);

    runtime
        .dispatch(Command::SelectLayer {
            layer_id: bottom_layer,
        })
        .unwrap();
    runtime
        .dispatch(Command::SetActiveTool(ToolId::FillMaterial))
        .unwrap();
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::FillMaterial {
            material_index: 0,
            opacity: 1.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("bottom layer fill should commit renderer artifacts");

    runtime
        .dispatch(Command::SelectLayer {
            layer_id: top_layer,
        })
        .unwrap();
    runtime
        .dispatch(Command::SetCurrentColor([0.0, 1.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::FillMaterial {
            material_index: 0,
            opacity: 1.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("top layer fill should commit renderer artifacts");

    assert_pixel_near(
        center_pixel(&document_surface(&runtime, bottom_surface)),
        [255, 0, 0, 255],
        3,
    );
    assert_pixel_near(
        center_pixel(&document_surface(&runtime, top_surface)),
        [0, 255, 0, 255],
        3,
    );
    assert_pixel_near(
        center_pixel(&read_composite(&mut engine)),
        [0, 255, 0, 255],
        3,
    );

    runtime.dispatch(Command::Undo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "undo of top layer fill should restore renderer texture without readback finalization"
    );
    assert!(runtime.state.can_redo());
    assert_pixel_near(
        center_pixel(&document_surface(&runtime, top_surface)),
        [0, 0, 0, 0],
        0,
    );
    assert_pixel_near(
        center_pixel(&read_surface(&mut engine, top_surface)),
        [0, 0, 0, 0],
        0,
    );
    assert_pixel_near(
        center_pixel(&read_composite(&mut engine)),
        [255, 0, 0, 255],
        3,
    );

    runtime.dispatch(Command::Redo).unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert!(!runtime.state.can_redo());
    assert_pixel_near(
        center_pixel(&assert_snapshot_matches_document_and_renderer(
            &runtime,
            &mut engine,
            top_surface,
        )),
        [0, 255, 0, 255],
        3,
    );
    assert_pixel_near(
        center_pixel(&read_composite(&mut engine)),
        [0, 255, 0, 255],
        3,
    );
}

#[test]
fn uv_paint_crossing_tile_boundary_e2e_commits_boundary_pixels() {
    let size = [300, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    set_paint_brush(&mut runtime, 0.08);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 1.0, 0.0]))
        .unwrap();

    stroke_uv(
        &mut runtime,
        &[Vec2::new(255.5 / 300.0, 0.5), Vec2::new(255.5 / 300.0, 0.5)],
        [300, 32],
    );

    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("tile-boundary UV paint stroke should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 1);
    let commit_rect = artifacts.surface_commits[0].rect;
    assert!(
        commit_rect.origin[0] <= 255
            && commit_rect.origin[0].saturating_add(commit_rect.size[0]) > 256,
        "renderer commit rect should span the document tile boundary at x=256, got {commit_rect:?}"
    );
    assert!(runtime.state.can_undo());

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 255, 16), [255, 255, 0, 255], 4);
    assert_pixel_near(pixel_at(&document_after, 256, 16), [255, 255, 0, 255], 4);
}

#[test]
fn tool_switch_during_pending_stroke_e2e_keeps_original_stroke_and_finalizes() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    set_paint_brush(&mut runtime, 0.20);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();

    let view_size = [64, 64];
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerDown(uv_sample(
            Vec2::new(0.35, 0.5),
            view_size,
            0.0,
        ))))
        .unwrap();
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerMove(uv_sample(
            Vec2::new(0.65, 0.5),
            view_size,
            0.1,
        ))))
        .unwrap();

    let smudge_tool = runtime
        .state
        .tool_id_by_config_id("builtin.brush.smudge.soft")
        .expect("smudge brush should exist");
    runtime
        .dispatch(Command::SetActiveTool(smudge_tool))
        .expect("tool switch commands are accepted but ignored while a stroke is active");
    assert_eq!(runtime.state.active_stroke_preset_index(), Some(0));
    assert!(runtime.state.is_stroking());

    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerUp(uv_sample(
            Vec2::new(0.65, 0.5),
            view_size,
            0.2,
        ))))
        .unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("pending paint stroke should finalize after ignored tool switch");
    assert!(runtime.state.can_undo());
    assert!(runtime.state.is_tool_idle());
    assert_pixel_near(
        center_pixel(&assert_snapshot_matches_document_and_renderer(
            &runtime,
            &mut engine,
            surface,
        )),
        [255, 0, 0, 255],
        4,
    );
}

#[test]
fn active_tool_switch_during_stroke_is_ignored_until_stroke_finalizes() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 0, 0]);
    set_paint_brush(&mut runtime, 0.20);
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    let selected_tool = runtime.state.active_tool_id();
    let view_size = [64, 64];
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerDown(uv_sample(
            Vec2::new(0.35, 0.5),
            view_size,
            0.0,
        ))))
        .unwrap();
    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerMove(uv_sample(
            Vec2::new(0.65, 0.5),
            view_size,
            0.1,
        ))))
        .unwrap();

    runtime
        .dispatch(Command::SetActiveTool(ToolId::FillMaterial))
        .expect("tool switch commands are accepted but ignored while a stroke is active");
    assert_eq!(runtime.state.active_tool_id(), selected_tool);
    assert!(runtime.state.is_stroking());

    runtime
        .dispatch(Command::ToolInput(ToolInputEvent::PointerUp(uv_sample(
            Vec2::new(0.65, 0.5),
            view_size,
            0.2,
        ))))
        .unwrap();

    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("pending paint stroke should finalize after ignored tool switch");
    assert!(runtime.state.can_undo());
    assert!(runtime.state.is_tool_idle());
    assert_eq!(runtime.state.active_tool_id(), selected_tool);
    assert_pixel_near(
        center_pixel(&assert_snapshot_matches_document_and_renderer(
            &runtime,
            &mut engine,
            surface,
        )),
        [255, 0, 0, 255],
        4,
    );
}

#[test]
fn lasso_selection_tool_input_e2e_constrains_gpu_fill_to_polygon() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 255, 255]);
    runtime.state.set_active_tool(ToolId::LassoSelection);

    let view_size = [64, 64];
    stroke_uv(
        &mut runtime,
        &[
            Vec2::new(0.05, 0.05),
            Vec2::new(0.55, 0.05),
            Vec2::new(0.05, 0.95),
            Vec2::new(0.05, 0.05),
        ],
        view_size,
    );

    let selection_artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("UV lasso selection should produce selection commit artifacts");
    assert!(selection_artifacts.surface_commits.is_empty());
    assert_eq!(selection_artifacts.selection_after_commits.len(), 1);

    runtime
        .dispatch(Command::SetActiveTool(ToolId::FillMaterial))
        .unwrap();
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::FillMaterial {
            material_index: 0,
            opacity: 1.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("lasso-selected fill should produce renderer commit artifacts");

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 4, 8), [255, 0, 0, 255], 4);
    assert_pixel_near(pixel_at(&document_after, 12, 8), [0, 0, 255, 255], 4);
}

#[test]
fn rectangle_selection_constrains_gpu_fill_e2e() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 255, 255]);

    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    let selection_artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("rectangle selection should produce selection commit artifacts");
    assert!(selection_artifacts.surface_commits.is_empty());
    assert_eq!(selection_artifacts.selection_after_commits.len(), 1);
    assert!(runtime.state.can_undo());

    runtime
        .dispatch(Command::SetActiveTool(ToolId::FillMaterial))
        .unwrap();
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::FillMaterial {
            material_index: 0,
            opacity: 1.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected fill should produce renderer commit artifacts");

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 4, 8), [255, 0, 0, 255], 4);
    assert_pixel_near(pixel_at(&document_after, 12, 8), [0, 0, 255, 255], 4);
    assert_pixel_near(
        pixel_at(&read_composite(&mut engine), 4, 8),
        [255, 0, 0, 255],
        4,
    );
    assert_pixel_near(
        pixel_at(&read_composite(&mut engine), 12, 8),
        [0, 0, 255, 255],
        4,
    );
}

#[test]
fn select_all_selects_every_material_and_supports_undo_redo() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(0)], false),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![
            solid_rgba(size, [0, 0, 255, 255]),
            solid_rgba(size, [255, 0, 0, 255]),
        ],
    );

    runtime.dispatch(Command::SelectAll).unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("select all should produce selection commit artifacts");
    assert_eq!(artifacts.selection_after_commits.len(), 2);
    for material_index in 0..2 {
        let commit = artifacts
            .selection_after_commits
            .iter()
            .find(|commit| commit.material_index.as_usize() == material_index)
            .expect("select all should commit every material selection mask");
        assert!(commit.r8.iter().all(|&coverage| coverage == 255));
    }
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("select all should record history")
            .label,
        "Select All"
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(!runtime.state.active_selection().unwrap().is_active());

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert!(runtime.state.active_selection().unwrap().is_active());
    for material_index in 0..2 {
        let snapshot = runtime
            .state
            .document()
            .unwrap()
            .selection_masks
            .snapshot(material_index.into())
            .expect("redo should restore every material selection mask");
        assert!(snapshot.r8.iter().all(|&coverage| coverage == 255));
    }
}

#[test]
fn invert_selection_complements_a_partial_selection_and_supports_undo_redo() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 255, 255]);

    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::ZERO,
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("rectangle selection should produce selection commit artifacts");

    runtime.dispatch(Command::InvertSelection).unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("invert selection should produce selection commit artifacts");
    let inverted = &artifacts.selection_after_commits[0].r8;
    assert_eq!(inverted[(8 * size[0] + 4) as usize], 0);
    assert_eq!(inverted[(8 * size[0] + 12) as usize], 255);
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("invert selection should record history")
            .label,
        "Invert Selection"
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let restored = runtime
        .state
        .document()
        .unwrap()
        .selection_masks
        .snapshot(0.into())
        .unwrap();
    assert_eq!(restored.r8[(8 * size[0] + 4) as usize], 255);
    assert_eq!(restored.r8[(8 * size[0] + 12) as usize], 0);

    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let redone = runtime
        .state
        .document()
        .unwrap()
        .selection_masks
        .snapshot(0.into())
        .unwrap();
    assert_eq!(redone.r8[(8 * size[0] + 4) as usize], 0);
    assert_eq!(redone.r8[(8 * size[0] + 12) as usize], 255);
}

#[test]
fn invert_selection_without_an_active_selection_selects_all() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime(&mut engine, size, [0, 0, 255, 255]);

    runtime.dispatch(Command::InvertSelection).unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("inverting no selection should produce selection commit artifacts");
    assert_eq!(artifacts.selection_after_commits.len(), 1);
    assert!(
        artifacts.selection_after_commits[0]
            .r8
            .iter()
            .all(|&coverage| coverage == 255)
    );
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("invert selection should record history")
            .label,
        "Invert Selection"
    );
}

#[test]
fn invert_selection_clears_stale_masks_before_enabling_new_materials() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, _) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(0)], false),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![
            solid_rgba(size, [0, 0, 255, 255]),
            solid_rgba(size, [255, 0, 0, 255]),
        ],
    );

    runtime.dispatch(Command::SelectAll).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("select all should initialize both GPU masks");
    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);

    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::ZERO,
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("partial selection should activate only the focused material");

    runtime.dispatch(Command::InvertSelection).unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("invert selection should commit both material masks");
    let material_b = artifacts
        .selection_after_commits
        .iter()
        .find(|commit| commit.material_index.as_usize() == 1)
        .expect("invert selection should enable the second material mask");
    assert!(material_b.r8.iter().all(|&coverage| coverage == 255));
}

#[test]
fn cut_raster_pixels_clears_surface_and_undo_restores_it() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial = solid_rgba(size, [96, 48, 24, 128]);
    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, initial.clone());

    runtime
        .dispatch(Command::CutSurfacePixels { target: surface })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("cut should produce renderer commit artifacts");

    let cut = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(cut.rgba8, solid_rgba(size, [0, 0, 0, 0]));
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("cut should record history")
            .label,
        "Cut"
    );

    runtime.dispatch(Command::Undo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "undo should restore cut pixels without renderer readback finalization"
    );
    let restored = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(restored.rgba8, initial);
    assert!(runtime.state.can_redo());

    runtime.dispatch(Command::Redo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "redo should restore the cut result without renderer readback finalization"
    );
    let redone = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(redone.rgba8, cut.rgba8);
}

#[test]
fn cut_layer_mask_pixels_clears_mask_and_undo_restores_it() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, raster_surface) = load_runtime(&mut engine, size, [255, 255, 255, 255]);
    let layer_id = raster_surface.layer_id;

    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);

    let mask_surface = PaintSurfaceId::layer_mask(0.into(), layer_id);
    let initial =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert_eq!(initial.rgba8, solid_rgba(size, [255, 255, 255, 255]));

    runtime
        .dispatch(Command::CutSurfacePixels {
            target: mask_surface,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("layer mask cut should produce renderer commit artifacts");

    let cut = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert_eq!(cut.rgba8, solid_rgba(size, [0, 0, 0, 0]));

    runtime.dispatch(Command::Undo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "undo should restore layer mask pixels without renderer readback finalization"
    );
    let restored =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert_eq!(restored.rgba8, initial.rgba8);
}

#[test]
fn layer_mask_invert_filter_updates_persistent_mask_and_supports_undo_redo() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, raster_surface) = load_runtime(&mut engine, size, [255, 255, 255, 255]);
    let layer_id = raster_surface.layer_id;
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::SelectLayerMask { layer_id })
        .unwrap();

    let mask_surface = PaintSurfaceId::layer_mask(0.into(), layer_id);
    runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id,
            adjustment: Adjustment::Invert,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("mask invert should commit the persistent mask");
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface).rgba8,
        solid_rgba(size, [0, 0, 0, 0])
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface).rgba8,
        solid_rgba(size, [255, 255, 255, 255])
    );
    runtime.dispatch(Command::Redo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface).rgba8,
        solid_rgba(size, [0, 0, 0, 0])
    );
}

#[test]
fn layer_mask_adjustment_preview_is_scalar_non_cumulative_and_cancel_restores() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, raster_surface) = load_runtime(&mut engine, size, [255, 255, 255, 255]);
    let layer_id = raster_surface.layer_id;
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::SelectLayerMask { layer_id })
        .unwrap();
    let mask_surface = PaintSurfaceId::layer_mask(0.into(), layer_id);
    let initial = document_surface(&runtime, mask_surface);
    let history_len = runtime.state.history.undo_stack.len();

    runtime
        .dispatch(Command::BeginAdjustmentFilterSession {
            layer_id,
            kind: AdjustmentKind::Levels,
        })
        .unwrap();
    let adjustment = Adjustment::Levels(LevelsAdjustment {
        master: LevelsChannel {
            output_white: 128,
            ..LevelsChannel::default()
        },
        ..LevelsAdjustment::default()
    });
    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id,
            adjustment: Some(adjustment.clone()),
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    let preview = read_surface(&mut engine, mask_surface);
    let pixel = pixel_at(&preview, 2, 2);
    assert!(pixel[3] > 0 && pixel[3] < 255);
    assert_eq!(pixel, [pixel[3]; 4]);
    assert_eq!(
        document_surface(&runtime, mask_surface).rgba8,
        initial.rgba8
    );

    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id,
            adjustment: None,
        })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert_eq!(read_surface(&mut engine, mask_surface).rgba8, initial.rgba8);

    runtime
        .dispatch(Command::PreviewAdjustmentFilter {
            layer_id,
            adjustment: Some(adjustment),
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::CancelAdjustmentFilterSession { layer_id })
        .unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert_eq!(read_surface(&mut engine, mask_surface).rgba8, initial.rgba8);
    assert_eq!(runtime.state.history.undo_stack.len(), history_len);
}

#[test]
fn layer_mask_levels_and_curves_apply_master_channel_to_scalar_value() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, raster_surface) = load_runtime(&mut engine, size, [255, 255, 255, 255]);
    let layer_id = raster_surface.layer_id;
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::SelectLayerMask { layer_id })
        .unwrap();
    let mask_surface = PaintSurfaceId::layer_mask(0.into(), layer_id);

    runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id,
            adjustment: Adjustment::Levels(LevelsAdjustment {
                master: LevelsChannel {
                    output_white: 128,
                    ..LevelsChannel::default()
                },
                ..LevelsAdjustment::default()
            }),
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine).expect("mask Levels should commit");
    let levels = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert_pixel_near(pixel_at(&levels, 2, 2), [128, 128, 128, 128], 1);

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let mut master = CurveChannel::default();
    assert!(master.set_point(
        1,
        CurvePoint {
            input: 255,
            output: 64
        }
    ));
    runtime
        .dispatch(Command::ApplyAdjustmentFilter {
            layer_id,
            adjustment: Adjustment::Curves(CurvesAdjustment {
                master,
                ..CurvesAdjustment::default()
            }),
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine).expect("mask Curves should commit");
    let curves = assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert_pixel_near(pixel_at(&curves, 2, 2), [64, 64, 64, 64], 1);
}

#[test]
fn layer_mask_surface_and_spatial_blur_soften_a_hard_mask_edge() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, raster_surface) = load_runtime(&mut engine, size, [255, 255, 255, 255]);
    let layer_id = raster_surface.layer_id;
    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::SelectLayerMask { layer_id })
        .unwrap();
    let mask_surface = PaintSurfaceId::layer_mask(0.into(), layer_id);
    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::CutSurfacePixels {
            target: mask_surface,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine).expect("mask cut should commit");
    runtime.dispatch(Command::DeselectSelection).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    let hard_edge = document_surface(&runtime, mask_surface);

    runtime
        .dispatch(Command::ApplySurfaceBlur { radius_px: 4.0 })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine).expect("mask Surface Blur should commit");
    let surface_blur =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert!(
        surface_blur
            .rgba8
            .chunks_exact(4)
            .any(|pixel| { pixel[3] > 0 && pixel[3] < 255 && pixel == [pixel[3]; 4] })
    );

    runtime.dispatch(Command::Undo).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    assert_eq!(
        document_surface(&runtime, mask_surface).rgba8,
        hard_edge.rgba8
    );
    runtime
        .dispatch(Command::ApplySpatialBlur {
            radius_px: 4.0,
            cross_meshes: false,
            orientation: SpatialBlurOrientation::Ignore,
            maximum_normal_angle_degrees: 180.0,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine).expect("mask Spatial Blur should commit");
    let spatial_blur =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert_ne!(spatial_blur.rgba8, hard_edge.rgba8);
    assert!(
        spatial_blur
            .rgba8
            .chunks_exact(4)
            .all(|pixel| pixel == [pixel[3]; 4])
    );
}

#[test]
fn rectangle_selection_constrains_cut_raster_pixels() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, surface) = load_runtime(&mut engine, size, [0, 0, 255, 255]);

    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("rectangle selection should produce selection commit artifacts");
    assert!(runtime.state.has_active_selection());

    runtime
        .dispatch(Command::CutSurfacePixels { target: surface })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected cut should produce renderer commit artifacts");

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_pixel_near(pixel_at(&document_after, 4, 8), [0, 0, 0, 0], 0);
    assert_pixel_near(pixel_at(&document_after, 12, 8), [0, 0, 255, 255], 4);
    assert!(runtime.state.has_active_selection());
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("selected cut should record history")
            .label,
        "Cut"
    );
}

#[test]
fn delete_selected_pixels_clears_layer_mask_without_deleting_the_mask() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let (mut runtime, raster_surface) = load_runtime(&mut engine, size, [255, 255, 255, 255]);
    let layer_id = raster_surface.layer_id;

    runtime
        .dispatch(Command::AddLayerMask {
            layer_id,
            mode: LayerMaskInitMode::RevealAll,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine);
    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("rectangle selection should produce selection commit artifacts");

    runtime.dispatch(Command::DeleteSelectedPixels).unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("selected layer mask delete should produce renderer commit artifacts");

    let mask_surface = PaintSurfaceId::layer_mask(0.into(), layer_id);
    let deleted =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, mask_surface);
    assert_pixel_near(pixel_at(&deleted, 4, 8), [0, 0, 0, 0], 0);
    assert_pixel_near(pixel_at(&deleted, 12, 8), [255, 255, 255, 255], 0);
    assert!(
        runtime
            .state
            .document()
            .unwrap()
            .layer_tree
            .has_layer_mask(layer_id)
    );
    assert!(runtime.state.has_active_selection());
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("delete should record history")
            .label,
        "Delete"
    );
}

#[test]
fn delete_selected_pixels_clears_selected_regions_across_materials_and_undoes_atomically() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial_a = solid_rgba(size, [0, 0, 255, 255]);
    let initial_b = solid_rgba(size, [255, 0, 0, 255]);
    let (mut runtime, surfaces) = load_runtime_with_material_pixels(
        &mut engine,
        parallel_material_pair_mesh([MeshId(0), MeshId(0)], false),
        vec![MaterialSpec::new("A", size), MaterialSpec::new("B", size)],
        vec![initial_a.clone(), initial_b.clone()],
    );

    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.0, 0.0),
            max_uv: Vec2::new(0.5, 1.0),
            operation: SelectionCompositeMode::Replace,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("first material selection should produce selection commit artifacts");
    runtime.dispatch(Command::SetFocusedMaterial(1)).unwrap();
    runtime
        .dispatch(Command::RectangleSelectUv {
            min_uv: Vec2::new(0.5, 0.0),
            max_uv: Vec2::new(1.0, 1.0),
            operation: SelectionCompositeMode::Add,
        })
        .unwrap();
    execute_runtime_effects(&mut runtime, &mut engine)
        .expect("second material selection should produce selection commit artifacts");

    runtime.dispatch(Command::DeleteSelectedPixels).unwrap();
    let artifacts = execute_runtime_effects(&mut runtime, &mut engine)
        .expect("multi-material delete should produce renderer commit artifacts");
    assert_eq!(artifacts.surface_commits.len(), 2);

    let deleted_a =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]);
    let deleted_b =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]);
    assert_pixel_near(pixel_at(&deleted_a, 4, 8), [0, 0, 0, 0], 0);
    assert_pixel_near(pixel_at(&deleted_a, 12, 8), [0, 0, 255, 255], 0);
    assert_pixel_near(pixel_at(&deleted_b, 4, 8), [255, 0, 0, 255], 0);
    assert_pixel_near(pixel_at(&deleted_b, 12, 8), [0, 0, 0, 0], 0);
    assert!(runtime.state.has_active_selection());
    assert_eq!(
        runtime
            .state
            .history
            .undo_stack
            .last()
            .expect("delete should record one history entry")
            .label,
        "Delete"
    );

    runtime.dispatch(Command::Undo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "undo should restore deleted pixels without renderer readback finalization"
    );
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]).rgba8,
        initial_a
    );
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]).rgba8,
        initial_b
    );
    assert!(runtime.state.has_active_selection());

    runtime.dispatch(Command::Redo).unwrap();
    assert!(
        execute_runtime_effects(&mut runtime, &mut engine).is_none(),
        "redo should restore the multi-material delete without renderer readback finalization"
    );
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[0]).rgba8,
        deleted_a.rgba8
    );
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surfaces[1]).rgba8,
        deleted_b.rgba8
    );
}

#[test]
fn delete_selected_pixels_without_selection_is_a_noop() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial = solid_rgba(size, [12, 34, 56, 255]);
    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, initial.clone());

    runtime.dispatch(Command::DeleteSelectedPixels).unwrap();
    assert!(execute_runtime_effects(&mut runtime, &mut engine).is_none());
    assert_eq!(
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface).rgba8,
        initial
    );
    assert!(!runtime.state.can_undo());
}

#[test]
fn zero_opacity_gpu_fill_e2e_finalizes_without_history_or_document_change() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU E2E test: no wgpu adapter/device available");
        return;
    };
    let initial = solid_rgba(size, [12, 34, 56, 78]);
    let (mut runtime, surface) = load_runtime_with_pixels(&mut engine, size, initial.clone());
    runtime
        .dispatch(Command::SetActiveTool(ToolId::FillMaterial))
        .unwrap();
    runtime
        .dispatch(Command::SetCurrentColor([1.0, 0.0, 0.0]))
        .unwrap();
    runtime
        .dispatch(Command::FillMaterial {
            material_index: 0,
            opacity: 0.0,
        })
        .unwrap();

    let (artifacts, project_changed) =
        execute_runtime_effects_with_project_change(&mut runtime, &mut engine)
            .expect("zero-opacity fill still exercises renderer readback finalization");
    assert!(!project_changed);
    assert_eq!(artifacts.surface_commits.len(), 1);
    assert!(!runtime.state.can_undo());
    assert_eq!(runtime.state.history.undo_stack.len(), 0);

    let document_after =
        assert_snapshot_matches_document_and_renderer(&runtime, &mut engine, surface);
    assert_eq!(document_after.rgba8, initial);
}

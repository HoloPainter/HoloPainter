use std::sync::Arc;

use eframe::egui_wgpu::wgpu;
use glam::{Mat4, Quat, Vec2, Vec3};
use holopainter::{
    core::{
        adjustment::{
            Adjustment, BrightnessContrastAdjustment, CurvePoint, CurvesAdjustment,
            GradientMapAdjustment, HueSaturationAdjustment, LevelsAdjustment, UvMirrorAdjustment,
            UvMirrorAxis, UvMirrorDirection,
        },
        brush_engine::{ParamValue, ResourceRefValue, SurfaceSourceMaterialScope},
        camera::OrbitCamera,
        composite::{GroupCompositeMode, LayerBlendMode},
        damage::DamageMap,
        decal::{DecalApplyPlan, DecalImageAsset, DecalImageId, DecalProjection, DecalTransform},
        document::{Document, MaterialSpec, MeshData, MeshId, MeshObject, SubMesh},
        geometry::RectU32,
        image::{LayerInitialPixels, MaterialPayload, Rgba8Snapshot},
        material::{MaterialRenderMode, MaterialRenderSettings},
        render_report::RenderMetrics,
        selection::ActiveSelection,
        stroke::{PaintSurfaceSet, StrokeContext, StrokeDab, SurfaceDab},
        stroke_preset::{PressureDynamics, StrokeOp},
        surface::PaintSurfaceId,
        tool::ColorSampleSource,
        transform::UvTransform,
        viewport_shading::ViewportShading,
        viewport_visibility::ViewportSceneVisibility,
        wireframe::WireframeStyle,
    },
    renderer::{
        ApplyCommand, ColorSampleAnchor, ColorSampleIntent, ColorSampleRequest, ColorSampleTarget,
        ColorSampleView, CompositeBakeTarget, CompositeCommand, DecalOverlayRequest, EditCommand,
        GpuDocumentCommand, MaterialUpload, ReadbackRequest, ReadbackResult, RenderEngine,
        RendererFramePlan, RendererStrokeOperation, RendererStrokeStyle, SelectionEditCommand,
        StrokeCommand, StrokeDabPayload, StrokeTarget, SurfaceProjectionDabBatch, TransformCommand,
        ViewCommand,
    },
};

fn brush_footprint_source_scope() -> SurfaceSourceMaterialScope {
    SurfaceSourceMaterialScope::BrushFootprint {
        extra_radius: Vec::new(),
    }
}

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
    MeshData::new(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
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

fn two_material_plane_mesh() -> MeshData {
    let positions = vec![
        Vec3::new(-0.9, -0.8, 0.0),
        Vec3::new(0.0, -0.8, 0.0),
        Vec3::new(-0.9, 0.8, 0.0),
        Vec3::new(0.0, 0.8, 0.0),
        Vec3::new(0.0, -0.8, 0.0),
        Vec3::new(0.9, -0.8, 0.0),
        Vec3::new(0.0, 0.8, 0.0),
        Vec3::new(0.9, 0.8, 0.0),
    ];
    let uvs = vec![Vec2::ZERO; positions.len()];
    let normals = vec![Vec3::Z; positions.len()];
    MeshData::new(
        positions,
        uvs,
        normals,
        vec![[0, 1, 2], [1, 3, 2], [4, 5, 6], [5, 7, 6]],
        vec![
            SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 6,
                material_index: 0,
                material_name: "Left".to_owned(),
                wireframe_edges: Vec::new(),
            },
            SubMesh {
                mesh_id: MeshId(0),
                start_index: 6,
                index_count: 6,
                material_index: 1,
                material_name: "Right".to_owned(),
                wireframe_edges: Vec::new(),
            },
        ],
        vec![MeshObject {
            id: MeshId(0),
            name: "Plane".to_owned(),
        }],
        vec![MeshId(0); 4],
    )
    .expect("test mesh should be valid")
}

fn overlapping_material_planes(z_by_material: &[f32]) -> MeshData {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut sub_meshes = Vec::new();
    for (material_index, z) in z_by_material.iter().copied().enumerate() {
        let vertex_start = positions.len() as u32;
        positions.extend([
            Vec3::new(-1.0, -1.0, z),
            Vec3::new(1.0, -1.0, z),
            Vec3::new(-1.0, 1.0, z),
            Vec3::new(1.0, 1.0, z),
        ]);
        uvs.extend([Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE]);
        normals.extend([Vec3::Z; 4]);
        let start_index = (indices.len() * 3) as u32;
        indices.extend([
            [vertex_start, vertex_start + 1, vertex_start + 2],
            [vertex_start + 1, vertex_start + 3, vertex_start + 2],
        ]);
        sub_meshes.push(SubMesh {
            mesh_id: MeshId(0),
            start_index,
            index_count: 6,
            material_index,
            material_name: format!("Material {material_index}"),
            wireframe_edges: Vec::new(),
        });
    }
    let triangle_count = indices.len();
    MeshData::new(
        positions,
        uvs,
        normals,
        indices,
        sub_meshes,
        vec![MeshObject {
            id: MeshId(0),
            name: "Overlapping planes".to_owned(),
        }],
        vec![MeshId(0); triangle_count],
    )
    .expect("overlapping material planes should be valid")
}

fn document(texture_size: [u32; 2]) -> Document {
    Document::new(
        single_triangle_mesh(),
        vec![MaterialSpec::new("Material", texture_size)],
    )
}

fn two_material_document(texture_size: [u32; 2]) -> Document {
    Document::new(
        two_material_plane_mesh(),
        vec![
            MaterialSpec::new("Left", texture_size),
            MaterialSpec::new("Right", texture_size),
        ],
    )
}

fn solid_rgba(texture_size: [u32; 2], rgba: [u8; 4]) -> Vec<u8> {
    let pixel_count = texture_size[0] as usize * texture_size[1] as usize;
    let mut pixels = Vec::with_capacity(pixel_count * 4);
    for _ in 0..pixel_count {
        pixels.extend_from_slice(&rgba);
    }
    pixels
}

#[derive(Debug, Clone)]
enum PlanCommand {
    GpuDocument(GpuDocumentCommand),
    Stroke(StrokeCommand),
    Apply(ApplyCommand),
    Composite(CompositeCommand),
    SelectionEdit(SelectionEditCommand),
    Transform(TransformCommand),
    View(ViewCommand),
}

impl From<GpuDocumentCommand> for PlanCommand {
    fn from(command: GpuDocumentCommand) -> Self {
        Self::GpuDocument(command)
    }
}

impl From<StrokeCommand> for PlanCommand {
    fn from(command: StrokeCommand) -> Self {
        Self::Stroke(command)
    }
}

impl From<ApplyCommand> for PlanCommand {
    fn from(command: ApplyCommand) -> Self {
        Self::Apply(command)
    }
}

impl From<CompositeCommand> for PlanCommand {
    fn from(command: CompositeCommand) -> Self {
        Self::Composite(command)
    }
}

impl From<SelectionEditCommand> for PlanCommand {
    fn from(command: SelectionEditCommand) -> Self {
        Self::SelectionEdit(command)
    }
}

impl From<TransformCommand> for PlanCommand {
    fn from(command: TransformCommand) -> Self {
        Self::Transform(command)
    }
}

impl From<ViewCommand> for PlanCommand {
    fn from(command: ViewCommand) -> Self {
        Self::View(command)
    }
}

fn upload_scene_command(document: &Document, rgba: [u8; 4]) -> PlanCommand {
    let size = document.materials[0].texture_size;
    GpuDocumentCommand::UploadScene {
        mesh: document.mesh.clone(),
        materials: vec![MaterialUpload {
            texture: MaterialPayload {
                width: size[0],
                height: size[1],
                rgba8: solid_rgba(size, rgba),
            },
            render_settings: document.materials[0].render_settings,
        }],
        surfaces: vec![PaintSurfaceId::raster(
            0.into(),
            document.layer_tree.default_raster_layer().unwrap(),
        )],
    }
    .into()
}

fn upload_scene_command_with_materials(
    document: &Document,
    rgba_by_material: &[[u8; 4]],
) -> PlanCommand {
    let default_layer = document.layer_tree.default_raster_layer().unwrap();
    GpuDocumentCommand::UploadScene {
        mesh: document.mesh.clone(),
        materials: document
            .materials
            .iter()
            .zip(rgba_by_material.iter().copied())
            .map(|(material, rgba)| MaterialUpload {
                texture: MaterialPayload {
                    width: material.texture_size[0],
                    height: material.texture_size[1],
                    rgba8: solid_rgba(material.texture_size, rgba),
                },
                render_settings: material.render_settings,
            })
            .collect(),
        surfaces: document
            .materials
            .iter()
            .enumerate()
            .map(|(material_index, _)| PaintSurfaceId::raster(material_index.into(), default_layer))
            .collect(),
    }
    .into()
}

fn upload_scene_with_materials_and_sync(
    document: &Document,
    rgba_by_material: &[[u8; 4]],
) -> Vec<PlanCommand> {
    let mut commands = vec![upload_scene_command_with_materials(
        document,
        rgba_by_material,
    )];
    commands.extend(
        (0..document.materials.len())
            .map(|material_index| sync_tree_command_for_material(document, material_index)),
    );
    commands
}

fn upload_surface_command(
    surface: PaintSurfaceId,
    texture_size: [u32; 2],
    rgba: [u8; 4],
) -> PlanCommand {
    GpuDocumentCommand::UploadSurfaceRgba8 {
        surface,
        rect: RectU32 {
            origin: [0, 0],
            size: texture_size,
        },
        rgba8: solid_rgba(texture_size, rgba),
        document_revision: None,
    }
    .into()
}

fn sync_tree_command(document: &Document) -> PlanCommand {
    GpuDocumentCommand::SyncMaterialTree {
        material_index: 0,
        tree: document.composite_tree(0).unwrap(),
    }
    .into()
}

fn sync_tree_command_for_material(document: &Document, material_index: usize) -> PlanCommand {
    GpuDocumentCommand::SyncMaterialTree {
        material_index,
        tree: document.composite_tree(material_index).unwrap(),
    }
    .into()
}

fn execute_ok(engine: &mut RenderEngine, commands: Vec<PlanCommand>) -> RenderMetrics {
    let report = engine.execute(frame_plan_from_commands(commands));
    assert!(
        report.is_ok(),
        "renderer batch failed: {:?}",
        report.error_message()
    );
    report.metrics().clone()
}

fn render_viewport_center_sample(engine: &mut RenderEngine, camera: OrbitCamera) -> [u8; 4] {
    render_viewport_center_sample_with_shading(engine, camera, ViewportShading::Unlit)
}

fn render_viewport_center_sample_with_shading(
    engine: &mut RenderEngine,
    camera: OrbitCamera,
    shading: ViewportShading,
) -> [u8; 4] {
    let viewport_size = [64, 64];
    let view_proj = camera.view_projection_matrix(1.0);
    let request = ColorSampleRequest {
        id: 1,
        intent: ColorSampleIntent::Preview,
        source: ColorSampleSource::View,
        anchor: ColorSampleAnchor {
            view: ColorSampleView::Viewport3d,
            position: [32, 32],
        },
        target: ColorSampleTarget::ViewOutput {
            view: ColorSampleView::Viewport3d,
            position: [32, 32],
        },
        document_generation: 1,
        active_layer_id: None,
    };
    let mut plan = frame_plan_from_commands(vec![
        ViewCommand::RenderViewport {
            camera: camera.clone(),
            view_proj,
            camera_world: camera.position().to_array(),
            viewport_size,
            brush_overlay_request: None,
            selection_overlay_request: None,
            mirror_plane_overlay_request: None,
            decal_overlay_request: None,
            scene_visibility: ViewportSceneVisibility::default(),
            show_wireframe: false,
            wireframe_style: WireframeStyle::new([1.0, 1.0, 1.0], 1.0),
            background_color: [0.0, 0.0, 0.0],
            shading,
        }
        .into(),
    ]);
    plan.color_sample_request = Some(request);
    let report = engine.execute(plan);
    assert!(
        report.is_ok(),
        "renderer batch failed: {:?}",
        report.error_message()
    );
    let completed = engine
        .wait_completed_color_samples()
        .into_iter()
        .next()
        .expect("viewport color sample did not complete");
    completed
        .result
        .expect("viewport color sample should succeed")
}

fn execute_report(
    engine: &mut RenderEngine,
    commands: Vec<PlanCommand>,
) -> holopainter::renderer::RenderExecutionReport {
    engine.execute(frame_plan_from_commands(commands))
}

fn frame_plan_from_commands(commands: Vec<PlanCommand>) -> RendererFramePlan {
    let mut plan = RendererFramePlan::default();
    for command in commands {
        match command {
            PlanCommand::GpuDocument(command) => {
                plan.document_commands.push(command);
            }
            PlanCommand::SelectionEdit(command) => {
                plan.edit_commands.push(EditCommand::Selection(command));
            }
            PlanCommand::Stroke(command) => {
                plan.edit_commands.push(EditCommand::Stroke(command));
            }
            PlanCommand::Apply(command) => {
                plan.edit_commands.push(EditCommand::Apply(command));
            }
            PlanCommand::Composite(command) => {
                plan.edit_commands.push(EditCommand::Composite(command));
            }
            PlanCommand::Transform(command) => {
                plan.edit_commands.push(EditCommand::Transform(command));
            }
            PlanCommand::View(command) => {
                plan.view_requests.push(command);
            }
        }
    }
    plan
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

fn first_pixel(snapshot: &Rgba8Snapshot) -> [u8; 4] {
    snapshot.rgba8[0..4].try_into().unwrap()
}

fn pixel_at(snapshot: &Rgba8Snapshot, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * snapshot.texture_size[0] + x) as usize) * 4;
    snapshot.rgba8[index..index + 4].try_into().unwrap()
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

fn assert_snapshot_near(actual: &Rgba8Snapshot, expected: &Rgba8Snapshot, tolerance: u8) {
    assert_eq!(
        actual.texture_size, expected.texture_size,
        "snapshot sizes differ"
    );
    for y in 0..actual.texture_size[1] {
        for x in 0..actual.texture_size[0] {
            let actual_pixel = pixel_at(actual, x, y);
            let expected_pixel = pixel_at(expected, x, y);
            for channel in 0..4 {
                let delta = actual_pixel[channel].abs_diff(expected_pixel[channel]);
                assert!(
                    delta <= tolerance,
                    "pixel mismatch at ({x}, {y}) channel {channel}: actual {actual_pixel:?}, expected {expected_pixel:?}, tolerance {tolerance}"
                );
            }
        }
    }
}

fn assert_active_preview_no_upload_or_general_fallback(metrics: &RenderMetrics) {
    assert_eq!(metrics.stroke_composite_upload_attempts, 0);
    assert_eq!(metrics.stroke_composite_general_fallbacks, 0);
    assert_eq!(metrics.stroke_composite_plan_misses, 0);
    assert_eq!(metrics.stroke_composite_checkpoint_budget_fallbacks, 0);
    assert_eq!(metrics.active_composite_source_cache_uploads, 0);
    assert_eq!(metrics.active_composite_checkpoint_rebuild_uploads, 0);
}

fn noop_uv_stroke_style() -> Arc<RendererStrokeStyle> {
    let params = external_source_over_paint_params(1.0, 1.0, 1.0);
    Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 1.0,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params,
        },
    })
}

fn prime_active_run_working_set(
    engine: &mut RenderEngine,
    surface: PaintSurfaceId,
    size: [u32; 2],
) {
    prime_active_surface_set(engine, PaintSurfaceSet::single(surface), size);
}

fn prime_active_surface_set(engine: &mut RenderEngine, surfaces: PaintSurfaceSet, _size: [u32; 2]) {
    let (_camera, _view_proj, context) = surface_test_context([64, 64]);
    execute_ok(
        engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface { surfaces, context },
                style: noop_uv_stroke_style(),
                active_selection: Arc::new(ActiveSelection::disabled_for_materials([0.into()])),
            }
            .into(),
        ],
    );
    assert!(
        engine.texture_metrics().checkpoint_texture_count > 0,
        "active-run priming should allocate checkpoint textures"
    );
}

fn prime_full_tree_active_surface_set(
    engine: &mut RenderEngine,
    surfaces: PaintSurfaceSet,
    _size: [u32; 2],
) -> RenderMetrics {
    let (_camera, _view_proj, context) = surface_test_context([64, 64]);
    execute_ok(
        engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface { surfaces, context },
                style: noop_uv_stroke_style(),
                active_selection: Arc::new(ActiveSelection::disabled_for_materials([0.into()])),
            }
            .into(),
            StrokeCommand::End { damage: None }.into(),
        ],
    )
}

fn begin_surface_stroke_command(surfaces: PaintSurfaceSet, _size: [u32; 2]) -> PlanCommand {
    let (_camera, _view_proj, context) = surface_test_context([64, 64]);
    StrokeCommand::Begin {
        target: StrokeTarget::Surface { surfaces, context },
        style: noop_uv_stroke_style(),
        active_selection: Arc::new(ActiveSelection::disabled_for_materials([0.into()])),
    }
    .into()
}

fn create_and_upload_surface_commands(
    surface: PaintSurfaceId,
    size: [u32; 2],
    rgba: [u8; 4],
) -> Vec<PlanCommand> {
    vec![
        GpuDocumentCommand::CreateSurface {
            target: surface,
            initial: LayerInitialPixels::Transparent,
        }
        .into(),
        upload_surface_command(surface, size, rgba),
    ]
}

fn run_scene_upload(
    engine: &mut RenderEngine,
    document: &Document,
    uploads: &[(PaintSurfaceId, [u8; 4])],
    background: [u8; 4],
) {
    let size = document.materials[0].texture_size;
    let mut commands = vec![upload_scene_command(document, background)];
    for (surface, rgba) in uploads {
        commands.extend(create_and_upload_surface_commands(*surface, size, *rgba));
    }
    commands.push(sync_tree_command(document));
    execute_ok(engine, commands);
}

#[test]
fn scene_upload_prewarms_brush_uv_island_masks() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = two_material_document(size);

    let metrics = execute_ok(
        &mut engine,
        vec![upload_scene_command_with_materials(
            &document,
            &[[0, 0, 0, 0], [0, 0, 0, 0]],
        )],
    );

    assert_eq!(metrics.uv_island_mask_generation_count, 2);
}

#[test]
fn completed_surface_stroke_reuses_material_source_texture() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 0]),
            sync_tree_command(&document),
        ],
    );

    execute_ok(
        &mut engine,
        vec![
            begin_surface_stroke_command(PaintSurfaceSet::single(surface), size),
            StrokeCommand::End { damage: None }.into(),
        ],
    );
    let metrics = execute_ok(
        &mut engine,
        vec![begin_surface_stroke_command(
            PaintSurfaceSet::single(surface),
            size,
        )],
    );

    assert_eq!(metrics.transient_texture_allocation_count, 0);
    assert!(metrics.transient_texture_reuse_count > 0);
    execute_ok(
        &mut engine,
        vec![StrokeCommand::End { damage: None }.into()],
    );
}

#[test]
fn composite_bake_rehydrates_transparent_target_before_copy() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let source_layer = document.layer_tree.default_raster_layer().unwrap();
    let target_layer = document
        .layer_tree
        .add_raster_layer_above(source_layer, "Bake Target")
        .unwrap();
    let target = PaintSurfaceId::raster(0.into(), target_layer);
    let tree = document.composite_tree_for_layers(0, &[source_layer]);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [20, 40, 60, 255]),
            GpuDocumentCommand::CreateSurface {
                target,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            CompositeCommand::BakeToSurfaces {
                targets: vec![CompositeBakeTarget { target, tree }],
                delete_sources_after_bake: Vec::new(),
                delete_embedded_images_after_bake: Vec::new(),
            }
            .into(),
        ],
    );

    assert_eq!(
        first_pixel(&read_surface(&mut engine, target)),
        [20, 40, 60, 255]
    );
}

#[test]
fn nonresident_transparent_raster_skips_transient_composite_upload() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let base = document.layer_tree.default_raster_layer().unwrap();
    let empty_layer = document
        .layer_tree
        .add_raster_layer_above(base, "Empty")
        .unwrap();
    let empty_surface = PaintSurfaceId::raster(0.into(), empty_layer);

    let metrics = execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [20, 40, 60, 255]),
            GpuDocumentCommand::CreateSurface {
                target: empty_surface,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );

    assert_eq!(first_pixel(&read_composite(&mut engine)), [20, 40, 60, 255]);
    // The visible base layer still uses the general-composite transient source
    // path. The known-empty layer must not add a second upload.
    assert_eq!(metrics.composite_transient_source_uploads, 1);
    assert_eq!(
        metrics.composite_transient_source_upload_bytes,
        size[0] as usize * size[1] as usize * 4
    );
    assert_eq!(metrics.composite_known_empty_raster_skips, 1);
}

#[test]
fn nonresident_hide_all_mask_skips_masked_raster_uploads() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let base = document.layer_tree.default_raster_layer().unwrap();
    let masked_layer = document
        .layer_tree
        .add_raster_layer_above(base, "Masked")
        .unwrap();
    assert!(document.layer_tree.add_layer_mask(masked_layer));
    let masked_surface = PaintSurfaceId::raster(0.into(), masked_layer);
    let mask_surface = PaintSurfaceId::layer_mask(0.into(), masked_layer);

    let metrics = execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [20, 40, 60, 255]),
            GpuDocumentCommand::CreateSurface {
                target: masked_surface,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            upload_surface_command(masked_surface, size, [200, 10, 20, 255]),
            GpuDocumentCommand::CreateSurface {
                target: mask_surface,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );

    assert_eq!(first_pixel(&read_composite(&mut engine)), [20, 40, 60, 255]);
    // Only the visible base layer should be uploaded. The masked raster and its
    // known-zero mask must both be removed before source resolution.
    assert_eq!(metrics.composite_transient_source_uploads, 1);
    assert_eq!(
        metrics.composite_transient_source_upload_bytes,
        size[0] as usize * size[1] as usize * 4
    );
    assert_eq!(metrics.composite_known_zero_mask_skips, 1);
}

#[test]
fn active_prime_plan_miss_fails_batch_and_records_prime_metric() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_background, "unsupported active group")
        .unwrap();
    let active_a = document
        .layer_tree
        .add_raster_layer_to_group(group, "active a")
        .unwrap();
    let active_b = document
        .layer_tree
        .add_raster_layer_to_group(group, "active b")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(group, LayerBlendMode::Screen);
    let active_a_surface = PaintSurfaceId::raster(0.into(), active_a);
    let active_b_surface = PaintSurfaceId::raster(0.into(), active_b);

    run_scene_upload(
        &mut engine,
        &document,
        &[
            (active_a_surface, [200, 20, 40, 255]),
            (active_b_surface, [40, 200, 20, 255]),
        ],
        [0, 0, 0, 255],
    );

    let report = execute_report(
        &mut engine,
        vec![begin_surface_stroke_command(
            PaintSurfaceSet::from_vec(vec![active_a_surface, active_b_surface]),
            size,
        )],
    );

    assert!(!report.is_ok());
    assert!(report.metrics().active_composite_prime_plan_misses > 0);
    assert_eq!(
        report
            .metrics()
            .stroke_composite_checkpoint_budget_fallbacks,
        0
    );

    let recovery_metrics = execute_ok(
        &mut engine,
        vec![upload_surface_command(
            active_a_surface,
            size,
            [20, 120, 220, 255],
        )],
    );
    assert_eq!(recovery_metrics.stroke_composite_plan_misses, 0);
    assert_eq!(recovery_metrics.stroke_composite_upload_attempts, 0);
}

#[test]
fn gpu_transform_preview_cancel_and_commit_restore_from_the_begin_snapshot() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let target = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let mut pixels = solid_rgba(size, [0, 0, 0, 0]);
    let source_offset = ((2 * size[0] + 2) * 4) as usize;
    pixels[source_offset..source_offset + 4].copy_from_slice(&[255, 0, 0, 255]);
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 0]),
            sync_tree_command(&document),
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface: target,
                rect: RectU32::full(size),
                rgba8: pixels,
                document_revision: None,
            }
            .into(),
        ],
    );

    let source_bounds = RectU32 {
        origin: [2, 2],
        size: [1, 1],
    };
    let transform =
        UvTransform::from_trs(Vec2::new(2.0, 1.0), 0.0, Vec2::ONE, Vec2::new(2.5, 2.5)).unwrap();
    let mut damage = DamageMap::default();
    damage.add_full_surface(target, size);
    let selection = ActiveSelection::disabled_for_materials([0.into()]);

    execute_ok(
        &mut engine,
        vec![
            TransformCommand::Begin {
                target,
                source_bounds,
                active_selection: selection.clone(),
            }
            .into(),
        ],
    );
    execute_ok(
        &mut engine,
        vec![
            TransformCommand::Preview {
                transform,
                result_damage: damage.clone(),
            }
            .into(),
        ],
    );
    let preview = read_composite(&mut engine);
    assert_eq!(pixel_at(&preview, 2, 2), [0, 0, 0, 0]);
    assert_eq!(pixel_at(&preview, 4, 3), [255, 0, 0, 255]);

    execute_ok(&mut engine, vec![TransformCommand::Cancel.into()]);
    let cancelled = read_composite(&mut engine);
    assert_eq!(pixel_at(&cancelled, 2, 2), [255, 0, 0, 255]);
    assert_eq!(pixel_at(&cancelled, 4, 3), [0, 0, 0, 0]);

    execute_ok(
        &mut engine,
        vec![
            TransformCommand::Begin {
                target,
                source_bounds,
                active_selection: selection,
            }
            .into(),
        ],
    );
    execute_ok(
        &mut engine,
        vec![
            TransformCommand::Commit {
                transform,
                result_damage: damage,
            }
            .into(),
        ],
    );
    let committed = read_surface(&mut engine, target);
    assert_eq!(pixel_at(&committed, 2, 2), [0, 0, 0, 0]);
    assert_eq!(pixel_at(&committed, 4, 3), [255, 0, 0, 255]);
}

#[test]
fn active_prime_checkpoint_budget_fallback_is_not_stroke_preview_fallback() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let active = document.layer_tree.default_raster_layer().unwrap();
    let mut previous = active;
    let mut above_layers = Vec::new();
    for index in 0..10 {
        let above = document
            .layer_tree
            .add_raster_layer_above(previous, format!("above {index}"))
            .unwrap();
        document
            .layer_tree
            .set_blend_mode(above, LayerBlendMode::Multiply);
        above_layers.push(above);
        previous = above;
    }
    let active_surface = PaintSurfaceId::raster(0.into(), active);
    let mut uploads = vec![(active_surface, [40, 200, 20, 255])];
    for layer in above_layers {
        let surface = PaintSurfaceId::raster(0.into(), layer);
        uploads.push((surface, [20, 40, 200, 255]));
    }

    run_scene_upload(&mut engine, &document, &uploads, [0, 0, 0, 255]);

    let report = execute_report(
        &mut engine,
        vec![begin_surface_stroke_command(
            PaintSurfaceSet::single(active_surface),
            size,
        )],
    );

    assert!(!report.is_ok());
    assert!(
        report
            .metrics()
            .active_composite_prime_checkpoint_budget_fallbacks
            > 0
    );
    assert_eq!(
        report
            .metrics()
            .stroke_composite_checkpoint_budget_fallbacks,
        0
    );

    let recovery_metrics = execute_ok(
        &mut engine,
        vec![upload_surface_command(
            active_surface,
            size,
            [220, 120, 20, 255],
        )],
    );
    assert_eq!(recovery_metrics.stroke_composite_plan_misses, 0);
    assert_eq!(recovery_metrics.stroke_composite_upload_attempts, 0);
}

#[test]
fn surface_stroke_add_dabs_primes_new_material_before_preview_composite() {
    let size = [8, 8];
    let viewport_size = [64, 64];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = two_material_document(size);
    let active_layer = document.layer_tree.default_raster_layer().unwrap();
    let left_surface = PaintSurfaceId::raster(0.into(), active_layer);
    let right_surface = PaintSurfaceId::raster(1.into(), active_layer);
    let (_camera, view_proj, context) = surface_test_context(viewport_size);
    let right_center = Vec3::new(0.45, 0.0, 0.0);
    let right_dab = SurfaceDab::with_scales(
        world_to_screen(view_proj, right_center, viewport_size),
        right_center,
        Vec3::Z,
        1,
        1.0,
        1.0,
    );

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command_with_materials(&document, &[[0, 0, 0, 0], [0, 0, 0, 0]]),
            sync_tree_command_for_material(&document, 0),
            sync_tree_command_for_material(&document, 1),
        ],
    );

    let params = external_source_over_paint_params(1.0, 1.0, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 0.25,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 0.0, 0.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([
        0.into(),
        1.into(),
    ]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface {
                    surfaces: PaintSurfaceSet::single(left_surface),
                    context,
                },
                style,
                active_selection: selection,
            }
            .into(),
        ],
    );

    let first_right_report = execute_report(
        &mut engine,
        vec![
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(right_surface),
                    target_surfaces: PaintSurfaceSet::single(right_surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(
                        vec![right_dab],
                        vec![vec![1]],
                    )],
                },
                preview_damage: None,
            }
            .into(),
        ],
    );
    assert!(
        first_right_report.is_ok(),
        "cross-material add dabs failed: {:?}",
        first_right_report.error_message()
    );
    let metrics = first_right_report.metrics();
    assert_eq!(metrics.active_composite_incremental_prime_calls, 1);
    assert_eq!(metrics.active_composite_incremental_prime_surfaces, 1);
    assert_eq!(metrics.active_composite_incremental_prime_materials, 1);
    assert_eq!(metrics.stroke_composite_upload_attempts, 0);
    assert_eq!(metrics.stroke_composite_general_fallbacks, 0);
    assert_eq!(metrics.stroke_composite_checkpoint_budget_fallbacks, 0);
    assert_eq!(metrics.active_composite_source_cache_uploads, 0);
    assert_eq!(metrics.active_composite_checkpoint_rebuild_uploads, 0);

    let repeated_right_report = execute_report(
        &mut engine,
        vec![
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(right_surface),
                    target_surfaces: PaintSurfaceSet::single(right_surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(
                        vec![right_dab],
                        vec![vec![1]],
                    )],
                },
                preview_damage: None,
            }
            .into(),
        ],
    );
    assert!(
        repeated_right_report.is_ok(),
        "repeated material add dabs failed: {:?}",
        repeated_right_report.error_message()
    );
    let metrics = repeated_right_report.metrics();
    assert_eq!(metrics.active_composite_incremental_prime_calls, 0);
    assert_eq!(metrics.stroke_composite_upload_attempts, 0);
    assert_eq!(metrics.stroke_composite_general_fallbacks, 0);
    assert_eq!(metrics.stroke_composite_checkpoint_budget_fallbacks, 0);
    assert_eq!(metrics.active_composite_source_cache_uploads, 0);
    assert_eq!(metrics.active_composite_checkpoint_rebuild_uploads, 0);

    execute_ok(
        &mut engine,
        vec![StrokeCommand::End { damage: None }.into()],
    );
}

fn external_paint_params(
    composite_mode: u32,
    opacity: f32,
    flow: f32,
    hardness: f32,
) -> Vec<(String, ParamValue)> {
    vec![
        (
            "composite_mode".to_owned(),
            ParamValue::Enum(composite_mode),
        ),
        ("opacity".to_owned(), ParamValue::F32(opacity)),
        ("flow".to_owned(), ParamValue::F32(flow)),
        ("hardness".to_owned(), ParamValue::F32(hardness)),
    ]
}

fn external_source_over_paint_params(
    opacity: f32,
    flow: f32,
    hardness: f32,
) -> Vec<(String, ParamValue)> {
    external_paint_params(0, opacity, flow, hardness)
}

fn external_multiply_paint_params(
    opacity: f32,
    flow: f32,
    hardness: f32,
) -> Vec<(String, ParamValue)> {
    external_paint_params(3, opacity, flow, hardness)
}

fn directional_paint_params() -> Vec<(String, ParamValue)> {
    let mut params = external_source_over_paint_params(1.0, 1.0, 1.0);
    params.extend([
        ("shape".to_owned(), ParamValue::Enum(1)),
        (
            "tip_mask".to_owned(),
            ParamValue::ResourceRef(ResourceRefValue::Resource(
                "texture.brush_tip.flat_64".to_owned(),
            )),
        ),
        ("tip_rotation".to_owned(), ParamValue::Enum(1)),
        ("anti_aliasing".to_owned(), ParamValue::Bool(true)),
    ]);
    params
}

fn directional_style(radius_world: f32) -> Arc<RendererStrokeStyle> {
    let params = directional_paint_params();
    Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 0.0, 0.0],
            params,
        },
    })
}

fn external_blur_params(
    opacity: f32,
    blur_radius_ratio: f32,
    hardness: f32,
) -> Vec<(String, ParamValue)> {
    vec![
        ("opacity".to_owned(), ParamValue::F32(opacity)),
        (
            "blur_radius_ratio".to_owned(),
            ParamValue::F32(blur_radius_ratio),
        ),
        ("hardness".to_owned(), ParamValue::F32(hardness)),
    ]
}

fn external_smudge_params(
    opacity: f32,
    flow: f32,
    hardness: f32,
    smudge_length_ratio: f32,
    sample_count: u32,
) -> Vec<(String, ParamValue)> {
    vec![
        ("opacity".to_owned(), ParamValue::F32(opacity)),
        ("flow".to_owned(), ParamValue::F32(flow)),
        ("hardness".to_owned(), ParamValue::F32(hardness)),
        (
            "smudge_length_ratio".to_owned(),
            ParamValue::F32(smudge_length_ratio),
        ),
        ("sample_count".to_owned(), ParamValue::U32(sample_count)),
    ]
}

fn horizontal_split_rgba(texture_size: [u32; 2], left: [u8; 4], right: [u8; 4]) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(texture_size[0] as usize * texture_size[1] as usize * 4);
    for _y in 0..texture_size[1] {
        for x in 0..texture_size[0] {
            let rgba = if x < texture_size[0] / 2 { left } else { right };
            pixels.extend_from_slice(&rgba);
        }
    }
    pixels
}

fn surface_test_camera() -> OrbitCamera {
    OrbitCamera {
        target: Vec3::new(0.5, 0.5, 0.0),
        orientation: Quat::IDENTITY,
        distance: 2.0,
        projection: holopainter::core::camera::CameraProjection::Perspective,
        fov_y_radians: 50.0_f32.to_radians(),
        orthographic_height: 4.0 * (25.0_f32.to_radians()).tan(),
        near: 0.1,
        far: 10.0,
    }
}

fn surface_test_context(viewport_size: [u32; 2]) -> (OrbitCamera, Mat4, Arc<StrokeContext>) {
    let camera = surface_test_camera();
    let view_proj =
        camera.view_projection_matrix(viewport_size[0] as f32 / viewport_size[1] as f32);
    let context = Arc::new(StrokeContext::surface(
        camera.clone(),
        view_proj,
        viewport_size,
        camera.position().into(),
        Default::default(),
    ));
    (camera, view_proj, context)
}

fn world_to_screen(view_proj: Mat4, world: Vec3, viewport_size: [u32; 2]) -> Vec2 {
    let ndc = view_proj.project_point3(world);
    Vec2::new(
        (ndc.x * 0.5 + 0.5) * viewport_size[0] as f32,
        (1.0 - (ndc.y * 0.5 + 0.5)) * viewport_size[1] as f32,
    )
}

fn uv_paint_stroke_first_pixel(opacity: f32, flow: f32, dab_count: usize) -> Option<[u8; 4]> {
    let size = [8, 8];
    let mut engine = gpu_engine(size)?;
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );

    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [0, 0, 0, 0])],
    );

    let params = external_source_over_paint_params(opacity, flow, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 2.0,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 0.0, 0.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));
    let dabs = (0..dab_count)
        .map(|_| StrokeDab::new(Vec2::new(0.5, 0.5), 1.0))
        .collect();

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(dabs),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        surface,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        ],
    );

    Some(first_pixel(&read_surface(&mut engine, surface)))
}

fn surface_paint_stroke_pixel(opacity: f32, flow: f32, batch_count: usize) -> Option<[u8; 4]> {
    let size = [16, 16];
    let viewport_size = [64, 64];
    let mut engine = gpu_engine(size)?;
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let (_camera, view_proj, context) = surface_test_context(viewport_size);
    let center = Vec3::new(0.35, 0.35, 0.0);

    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [0, 0, 0, 0])],
    );

    let params = external_source_over_paint_params(opacity, flow, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 0.25,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 0.0, 0.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));
    let surface_dab = SurfaceDab::with_scales(
        world_to_screen(view_proj, center, viewport_size),
        center,
        Vec3::Z,
        0,
        1.0,
        1.0,
    );

    let mut commands: Vec<PlanCommand> = Vec::with_capacity(batch_count + 2);
    commands.push(
        StrokeCommand::Begin {
            target: StrokeTarget::Surface {
                surfaces: PaintSurfaceSet::single(surface),
                context,
            },
            style,
            active_selection: selection,
        }
        .into(),
    );
    for _ in 0..batch_count {
        commands.push(
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(surface),
                    target_surfaces: PaintSurfaceSet::single(surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(
                        vec![surface_dab],
                        vec![vec![surface.material_index().as_usize()]],
                    )],
                },
                preview_damage: None,
            }
            .into(),
        );
    }
    commands.push(StrokeCommand::End { damage: None }.into());
    execute_ok(&mut engine, commands);

    Some(pixel_at(&read_surface(&mut engine, surface), 5, 5))
}

fn directional_uv_snapshot(size: [u32; 2], dabs: Vec<StrokeDab>) -> Option<Rgba8Snapshot> {
    let mut engine = gpu_engine(size)?;
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [0, 0, 0, 0])],
    );

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style: directional_style(0.18),
                active_selection: Arc::new(ActiveSelection::disabled_for_materials([0.into()])),
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(dabs),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        surface,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        ],
    );
    Some(read_surface(&mut engine, surface))
}

fn alpha_extent(snapshot: &Rgba8Snapshot) -> Option<[u32; 2]> {
    let [width, height] = snapshot.texture_size;
    let mut min = [width, height];
    let mut max = [0, 0];
    let mut found = false;
    for y in 0..height {
        for x in 0..width {
            if pixel_at(snapshot, x, y)[3] > 8 {
                min[0] = min[0].min(x);
                min[1] = min[1].min(y);
                max[0] = max[0].max(x);
                max[1] = max[1].max(y);
                found = true;
            }
        }
    }
    found.then_some([max[0] - min[0] + 1, max[1] - min[1] + 1])
}

fn directional_surface_snapshot(world_positions: &[Vec3]) -> Option<Rgba8Snapshot> {
    let size = [32, 32];
    let viewport_size = [96, 96];
    let mut engine = gpu_engine(size)?;
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let (_camera, view_proj, context) = surface_test_context(viewport_size);
    let dabs = world_positions
        .iter()
        .copied()
        .map(|position| {
            SurfaceDab::with_scales(
                world_to_screen(view_proj, position, viewport_size),
                position,
                Vec3::Z,
                0,
                1.0,
                1.0,
            )
        })
        .collect::<Vec<_>>();
    let materials = vec![vec![surface.material_index().as_usize()]; dabs.len()];
    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [0, 0, 0, 0])],
    );
    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface {
                    surfaces: PaintSurfaceSet::single(surface),
                    context,
                },
                style: directional_style(0.18),
                active_selection: Arc::new(ActiveSelection::disabled_for_materials([0.into()])),
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(surface),
                    target_surfaces: PaintSurfaceSet::single(surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(dabs, materials)],
                },
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End { damage: None }.into(),
        ],
    );
    Some(read_surface(&mut engine, surface))
}

#[test]
fn uv_paint_flow_builds_up_and_opacity_caps_stroke_alpha() {
    let Some(one_dab) = uv_paint_stroke_first_pixel(0.5, 0.25, 1) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let two_dabs = uv_paint_stroke_first_pixel(0.5, 0.25, 2)
        .expect("same GPU adapter should still be available");
    let capped = uv_paint_stroke_first_pixel(0.5, 1.0, 2)
        .expect("same GPU adapter should still be available");

    assert_pixel_near(one_dab, [32, 0, 0, 32], 3);
    assert_pixel_near(two_dabs, [56, 0, 0, 56], 3);
    assert_pixel_near(capped, [128, 0, 0, 128], 3);
    assert!(
        one_dab[3] < two_dabs[3] && two_dabs[3] < capped[3],
        "flow should build up toward the opacity cap: one={one_dab:?}, two={two_dabs:?}, capped={capped:?}"
    );
}

#[test]
fn external_paint_surface_opacity_caps_across_batches() {
    let Some(one_batch) = surface_paint_stroke_pixel(0.5, 1.0, 1) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let two_batches = surface_paint_stroke_pixel(0.5, 1.0, 2)
        .expect("same GPU adapter should still be available");

    assert_pixel_near(one_batch, [128, 0, 0, 128], 5);
    assert_pixel_near(two_batches, [128, 0, 0, 128], 5);
}

#[test]
fn external_paint_uv_stroke_updates_surface_pixels() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );

    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [0, 0, 0, 0])],
    );

    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 2.0,
            radius_pressure: PressureDynamics::default(),
            params: external_source_over_paint_params(1.0, 1.0, 1.0),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 0.0, 0.0],
            params: external_source_over_paint_params(1.0, 1.0, 1.0),
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)]),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        surface,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        ],
    );

    let surface_snapshot = read_surface(&mut engine, surface);
    assert_pixel_near(pixel_at(&surface_snapshot, 4, 4), [255, 0, 0, 255], 3);
}

#[test]
fn directional_paint_uv_zero_direction_writes_no_pixels() {
    let Some(snapshot) =
        directional_uv_snapshot([32, 32], vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)])
    else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };

    assert!(snapshot.rgba8.iter().all(|channel| *channel == 0));
}

#[test]
fn directional_paint_uv_rotates_flat_tip_with_stroke() {
    let Some(horizontal) = directional_uv_snapshot(
        [64, 64],
        vec![
            StrokeDab::new(Vec2::new(0.45, 0.5), 1.0),
            StrokeDab::new(Vec2::new(0.55, 0.5), 1.0),
        ],
    ) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let vertical = directional_uv_snapshot(
        [64, 64],
        vec![
            StrokeDab::new(Vec2::new(0.5, 0.45), 1.0),
            StrokeDab::new(Vec2::new(0.5, 0.55), 1.0),
        ],
    )
    .expect("same GPU adapter should still be available");

    let horizontal_extent = alpha_extent(&horizontal).expect("horizontal stroke should paint");
    let vertical_extent = alpha_extent(&vertical).expect("vertical stroke should paint");
    assert!(
        horizontal_extent[0] > horizontal_extent[1],
        "horizontal flat tip should be wider than tall: {horizontal_extent:?}"
    );
    assert!(
        vertical_extent[1] > vertical_extent[0],
        "vertical flat tip should be taller than wide: {vertical_extent:?}"
    );
}

#[test]
fn directional_paint_uv_uses_texture_pixel_space_on_non_square_surface() {
    let Some(snapshot) = directional_uv_snapshot(
        [80, 40],
        vec![
            StrokeDab::new(Vec2::new(39.0 / 80.0, 19.0 / 40.0), 1.0),
            StrokeDab::new(Vec2::new(41.0 / 80.0, 21.0 / 40.0), 1.0),
        ],
    ) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };

    let extent = alpha_extent(&snapshot).expect("diagonal stroke should paint");
    assert!(
        extent[0].abs_diff(extent[1]) <= 3,
        "equal pixel-space X/Y movement should orient the flat tip near 45 degrees: {extent:?}"
    );
}

#[test]
fn directional_paint_surface_zero_direction_writes_no_pixels() {
    let Some(snapshot) = directional_surface_snapshot(&[Vec3::new(0.35, 0.35, 0.0)]) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    assert!(snapshot.rgba8.iter().all(|channel| *channel == 0));
}

#[test]
fn directional_paint_surface_draws_once_direction_is_available() {
    let Some(snapshot) =
        directional_surface_snapshot(&[Vec3::new(0.30, 0.35, 0.0), Vec3::new(0.40, 0.35, 0.0)])
    else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };

    assert!(snapshot.rgba8.chunks_exact(4).any(|pixel| pixel[3] > 8));
}

#[test]
fn external_paint_uv_hardness_shapes_stroke_mask() {
    let size = [16, 16];
    let Some(mut hard_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let Some(mut soft_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    fn paint_with_hardness(
        engine: &mut RenderEngine,
        document: &Document,
        surface: PaintSurfaceId,
        selection: Arc<ActiveSelection>,
        hardness: f32,
    ) -> Rgba8Snapshot {
        let size = document.materials[0].texture_size;
        execute_ok(engine, vec![upload_scene_command(document, [0, 0, 0, 0])]);
        let style = Arc::new(RendererStrokeStyle {
            stroke_op: StrokeOp::BrushEngine {
                engine_id: "paint".to_owned(),
                radius_world: 0.25,
                radius_pressure: PressureDynamics::default(),
                params: external_source_over_paint_params(1.0, 1.0, hardness),
                param_dynamics: Vec::new(),
                surface_source_material_scope: brush_footprint_source_scope(),
            },
            operation: RendererStrokeOperation::BrushEngine {
                color: [1.0, 0.0, 0.0],
                params: external_source_over_paint_params(1.0, 1.0, hardness),
            },
        });

        execute_ok(
            engine,
            vec![
                StrokeCommand::Begin {
                    target: StrokeTarget::Uv { surface },
                    style,
                    active_selection: selection,
                }
                .into(),
                StrokeCommand::AddDabs {
                    dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)]),
                    preview_damage: None,
                }
                .into(),
                StrokeCommand::End {
                    damage: Some({
                        let mut damage = DamageMap::default();
                        damage.add_rect(
                            surface,
                            RectU32 {
                                origin: [0, 0],
                                size,
                            },
                        );
                        damage
                    }),
                }
                .into(),
            ],
        );
        read_surface(engine, surface)
    }

    let hard = paint_with_hardness(&mut hard_engine, &document, surface, selection.clone(), 1.0);
    let soft = paint_with_hardness(&mut soft_engine, &document, surface, selection, 0.0);
    let hard_edge = pixel_at(&hard, 11, 8)[3];
    let soft_edge = pixel_at(&soft, 11, 8)[3];

    assert!(
        hard_edge > soft_edge.saturating_add(64),
        "high hardness should keep the stroke mask edge denser than low hardness: hard={hard_edge}, soft={soft_edge}"
    );
}

#[test]
fn external_blur_uv_stroke_updates_surface_pixels() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );

    let mut rgba = solid_rgba(size, [255, 255, 255, 255]);
    let center_index = ((4 * size[0] + 4) as usize) * 4;
    rgba[center_index..center_index + 4].copy_from_slice(&[0, 0, 0, 255]);
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 255]),
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: RectU32 {
                    origin: [0, 0],
                    size,
                },
                rgba8: rgba,
                document_revision: None,
            }
            .into(),
        ],
    );

    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "blur".to_owned(),
            radius_world: 2.0,
            radius_pressure: PressureDynamics::default(),
            params: external_blur_params(1.0, 1.0, 1.0),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params: external_blur_params(1.0, 1.0, 1.0),
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)]),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        surface,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        ],
    );

    let blurred = pixel_at(&read_surface(&mut engine, surface), 4, 4);
    assert!(
        blurred[0] > 0 && blurred[3] == 255,
        "external blur should change the black first pixel toward neighboring white pixels: {blurred:?}"
    );
}

#[test]
fn external_blur_uv_stroke_accumulates_across_dab_batches() {
    let size = [8, 8];
    let Some(mut one_batch_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let Some(mut two_batch_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    fn blur_center(
        engine: &mut RenderEngine,
        document: &Document,
        surface: PaintSurfaceId,
        selection: Arc<ActiveSelection>,
        batch_count: usize,
    ) -> [u8; 4] {
        let size = document.materials[0].texture_size;
        let mut rgba = solid_rgba(size, [255, 255, 255, 255]);
        let center_index = ((4 * size[0] + 4) as usize) * 4;
        rgba[center_index..center_index + 4].copy_from_slice(&[0, 0, 0, 255]);
        execute_ok(
            engine,
            vec![
                upload_scene_command(document, [0, 0, 0, 255]),
                GpuDocumentCommand::UploadSurfaceRgba8 {
                    surface,
                    rect: RectU32 {
                        origin: [0, 0],
                        size,
                    },
                    rgba8: rgba,
                    document_revision: None,
                }
                .into(),
            ],
        );
        let params = external_blur_params(1.0, 1.0, 1.0);
        let style = Arc::new(RendererStrokeStyle {
            stroke_op: StrokeOp::BrushEngine {
                engine_id: "blur".to_owned(),
                radius_world: 0.125,
                radius_pressure: PressureDynamics::default(),
                params: params.clone(),
                param_dynamics: Vec::new(),
                surface_source_material_scope: brush_footprint_source_scope(),
            },
            operation: RendererStrokeOperation::BrushEngine {
                color: [1.0, 1.0, 1.0],
                params,
            },
        });
        let mut commands = vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: selection,
            }
            .into(),
        ];
        commands.extend((0..batch_count).map(|_| {
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)]),
                preview_damage: None,
            }
            .into()
        }));
        commands.push(
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        surface,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        );
        execute_ok(engine, commands);
        pixel_at(&read_surface(engine, surface), 4, 4)
    }

    let once = blur_center(
        &mut one_batch_engine,
        &document,
        surface,
        selection.clone(),
        1,
    );
    let twice = blur_center(&mut two_batch_engine, &document, surface, selection, 2);
    assert!(
        twice[0] > once[0].saturating_add(8),
        "second UV blur batch should blur the result of the first batch: once={once:?}, twice={twice:?}"
    );
}

#[test]
fn external_blur_uv_radius_follows_brush_size() {
    let size = [16, 16];
    let Some(mut small_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let Some(mut large_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    fn blur_center_with_radius(
        engine: &mut RenderEngine,
        document: &Document,
        surface: PaintSurfaceId,
        selection: Arc<ActiveSelection>,
        radius_world: f32,
    ) -> [u8; 4] {
        let size = document.materials[0].texture_size;
        let mut rgba = solid_rgba(size, [255, 255, 255, 255]);
        let center_index = ((8 * size[0] + 8) as usize) * 4;
        rgba[center_index..center_index + 4].copy_from_slice(&[0, 0, 0, 255]);
        execute_ok(
            engine,
            vec![
                upload_scene_command(document, [0, 0, 0, 255]),
                GpuDocumentCommand::UploadSurfaceRgba8 {
                    surface,
                    rect: RectU32 {
                        origin: [0, 0],
                        size,
                    },
                    rgba8: rgba,
                    document_revision: None,
                }
                .into(),
            ],
        );

        let style = Arc::new(RendererStrokeStyle {
            stroke_op: StrokeOp::BrushEngine {
                engine_id: "blur".to_owned(),
                radius_world: radius_world,
                radius_pressure: PressureDynamics::default(),
                params: external_blur_params(1.0, 1.0, 1.0),
                param_dynamics: Vec::new(),
                surface_source_material_scope: brush_footprint_source_scope(),
            },
            operation: RendererStrokeOperation::BrushEngine {
                color: [1.0, 1.0, 1.0],
                params: external_blur_params(1.0, 1.0, 1.0),
            },
        });

        execute_ok(
            engine,
            vec![
                StrokeCommand::Begin {
                    target: StrokeTarget::Uv { surface },
                    style,
                    active_selection: selection,
                }
                .into(),
                StrokeCommand::AddDabs {
                    dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)]),
                    preview_damage: None,
                }
                .into(),
                StrokeCommand::End {
                    damage: Some({
                        let mut damage = DamageMap::default();
                        damage.add_rect(
                            surface,
                            RectU32 {
                                origin: [0, 0],
                                size,
                            },
                        );
                        damage
                    }),
                }
                .into(),
            ],
        );

        pixel_at(&read_surface(engine, surface), 8, 8)
    }

    let small = blur_center_with_radius(
        &mut small_engine,
        &document,
        surface,
        selection.clone(),
        0.0625,
    );
    let large = blur_center_with_radius(&mut large_engine, &document, surface, selection, 0.25);

    assert!(
        large[0] > small[0].saturating_add(16),
        "larger external blur brush radius should increase blur radius in pixels: small={small:?}, large={large:?}"
    );
}

#[test]
fn external_smudge_uv_stroke_updates_surface_pixels_after_end() {
    let size = [16, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 255, 255]),
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: RectU32 {
                    origin: [0, 0],
                    size,
                },
                rgba8: horizontal_split_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]),
                document_revision: None,
            }
            .into(),
        ],
    );

    let params = external_smudge_params(1.0, 1.0, 1.0, 3.0, 16);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "smudge".to_owned(),
            radius_world: 0.25,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.50, 0.25), 1.0)]),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.66, 0.25), 1.0)]),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.82, 0.25), 1.0)]),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        surface,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        ],
    );

    let pixels = read_surface(&mut engine, surface);
    let smeared = pixel_at(&pixels, 10, 4);
    assert!(
        smeared[0] > 32 && smeared[2] < 224,
        "external smudge UV stroke should persist a red smear into the blue side after End: {smeared:?}"
    );
    let accumulated = pixel_at(&pixels, 13, 4);
    assert!(
        accumulated[0] > 32 && accumulated[2] < 224,
        "later UV smudge batches should carry forward earlier batch output: {accumulated:?}"
    );
}

#[test]
fn external_paint_surface_stroke_updates_surface_pixels() {
    let size = [16, 16];
    let viewport_size = [64, 64];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let (_camera, view_proj, context) = surface_test_context(viewport_size);
    let center = Vec3::new(0.35, 0.35, 0.0);

    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [0, 0, 0, 0])],
    );

    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 0.25,
            radius_pressure: PressureDynamics::default(),
            params: external_source_over_paint_params(1.0, 1.0, 1.0),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 0.0, 0.0],
            params: external_source_over_paint_params(1.0, 1.0, 1.0),
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface {
                    surfaces: PaintSurfaceSet::single(surface),
                    context,
                },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(surface),
                    target_surfaces: PaintSurfaceSet::single(surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(
                        vec![SurfaceDab::with_scales(
                            world_to_screen(view_proj, center, viewport_size),
                            center,
                            Vec3::Z,
                            0,
                            1.0,
                            1.0,
                        )],
                        vec![vec![surface.material_index().as_usize()]],
                    )],
                },
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End { damage: None }.into(),
        ],
    );

    let painted = pixel_at(&read_surface(&mut engine, surface), 5, 5);
    assert_pixel_near(painted, [255, 0, 0, 255], 4);
}

#[test]
fn external_blur_surface_stroke_updates_surface_pixels() {
    let size = [16, 16];
    let viewport_size = [64, 64];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let (_camera, view_proj, context) = surface_test_context(viewport_size);
    let center = Vec3::new(0.35, 0.35, 0.0);

    let mut rgba = solid_rgba(size, [255, 255, 255, 255]);
    let center_index = ((5 * size[0] + 5) as usize) * 4;
    rgba[center_index..center_index + 4].copy_from_slice(&[0, 0, 0, 255]);
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [255, 255, 255, 255]),
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: RectU32 {
                    origin: [0, 0],
                    size,
                },
                rgba8: rgba,
                document_revision: None,
            }
            .into(),
        ],
    );

    let params = external_blur_params(1.0, 2.0, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "blur".to_owned(),
            radius_world: 0.25,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface {
                    surfaces: PaintSurfaceSet::single(surface),
                    context,
                },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(surface),
                    target_surfaces: PaintSurfaceSet::single(surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(
                        vec![SurfaceDab::with_scales(
                            world_to_screen(view_proj, center, viewport_size),
                            center,
                            Vec3::Z,
                            0,
                            1.0,
                            1.0,
                        )],
                        vec![vec![surface.material_index().as_usize()]],
                    )],
                },
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End { damage: None }.into(),
        ],
    );

    let blurred = pixel_at(&read_surface(&mut engine, surface), 5, 5);
    assert!(
        blurred[0] > 0 && blurred[3] == 255,
        "external blur Surface stroke should commit blurred pixels after End: {blurred:?}"
    );
}

fn blur_front_material_with_background(background: [u8; 4]) -> Option<Rgba8Snapshot> {
    let size = [16, 16];
    let viewport_size = [64, 64];
    let mut engine = gpu_engine(size)?;
    let mut front = MaterialSpec::new("Blue glass", size);
    front.render_settings.render_mode = MaterialRenderMode::Blend;
    let document = Document::new(
        overlapping_material_planes(&[0.0, -0.2]),
        vec![front, MaterialSpec::new("Background", size)],
    );
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let (_camera, view_proj, context) = surface_test_context(viewport_size);
    let center = Vec3::new(0.35, 0.35, 0.0);
    let mut front_pixels = solid_rgba(size, [0, 0, 128, 128]);
    let center_index = ((5 * size[0] + 5) as usize) * 4;
    front_pixels[center_index..center_index + 4].copy_from_slice(&[0, 0, 0, 128]);
    let mut commands =
        upload_scene_with_materials_and_sync(&document, &[[0, 0, 128, 128], background]);
    commands.push(
        GpuDocumentCommand::UploadSurfaceRgba8 {
            surface,
            rect: RectU32 {
                origin: [0, 0],
                size,
            },
            rgba8: front_pixels,
            document_revision: None,
        }
        .into(),
    );
    execute_ok(&mut engine, commands);

    let params = external_blur_params(1.0, 2.0, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "blur".to_owned(),
            radius_world: 0.25,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params,
        },
    });
    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface {
                    surfaces: PaintSurfaceSet::single(surface),
                    context,
                },
                style,
                active_selection: Arc::new(ActiveSelection::disabled_for_materials([0.into()])),
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(surface),
                    target_surfaces: PaintSurfaceSet::single(surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(
                        vec![SurfaceDab::with_scales(
                            world_to_screen(view_proj, center, viewport_size),
                            center,
                            Vec3::Z,
                            0,
                            1.0,
                            1.0,
                        )],
                        vec![vec![surface.material_index().as_usize()]],
                    )],
                },
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End { damage: None }.into(),
        ],
    );
    Some(read_surface(&mut engine, surface))
}

#[test]
fn surface_source_blur_result_does_not_include_background_material_color() {
    let Some(red_background) = blur_front_material_with_background([255, 0, 0, 255]) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let green_background = blur_front_material_with_background([0, 255, 0, 255])
        .expect("same GPU adapter should still be available");

    assert_eq!(red_background.rgba8, green_background.rgba8);
}

#[test]
fn external_smudge_surface_stroke_updates_surface_pixels_after_end() {
    let size = [16, 16];
    let viewport_size = [64, 64];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );
    let (_camera, view_proj, context) = surface_test_context(viewport_size);
    let from = Vec3::new(0.50, 0.20, 0.0);
    let to = Vec3::new(0.66, 0.20, 0.0);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 255, 255]),
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: RectU32 {
                    origin: [0, 0],
                    size,
                },
                rgba8: horizontal_split_rgba(size, [255, 0, 0, 255], [0, 0, 255, 255]),
                document_revision: None,
            }
            .into(),
        ],
    );

    let params = external_smudge_params(1.0, 1.0, 1.0, 3.0, 16);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "smudge".to_owned(),
            radius_world: 0.25,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Surface {
                    surfaces: PaintSurfaceSet::single(surface),
                    context,
                },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Surface {
                    source_surfaces: PaintSurfaceSet::single(surface),
                    target_surfaces: PaintSurfaceSet::single(surface),
                    projection_batches: vec![SurfaceProjectionDabBatch::primary(
                        vec![
                            SurfaceDab::with_scales(
                                world_to_screen(view_proj, from, viewport_size),
                                from,
                                Vec3::Z,
                                0,
                                1.0,
                                1.0,
                            ),
                            SurfaceDab::with_scales(
                                world_to_screen(view_proj, to, viewport_size),
                                to,
                                Vec3::Z,
                                0,
                                1.0,
                                1.0,
                            ),
                        ],
                        vec![vec![surface.material_index().as_usize()]; 2],
                    )],
                },
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End { damage: None }.into(),
        ],
    );

    let smeared = pixel_at(&read_surface(&mut engine, surface), 10, 3);
    assert!(
        smeared[0] > 32 && smeared[2] < 224,
        "external smudge Surface stroke should persist a red smear into the blue side after End: {smeared:?}"
    );
}

#[test]
fn uv_paint_multiply_darkens_existing_surface() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let document = document(size);
    let surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );

    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [128, 128, 128, 255])],
    );

    let params = external_multiply_paint_params(1.0, 1.0, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 2.0,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [0.5, 0.25, 1.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)]),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        surface,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_surface(&mut engine, surface)),
        [64, 32, 128, 255],
        3,
    );
}

#[test]
fn solid_fill_gpu_composite_uses_constant_color_without_surface_upload() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    document
        .layer_tree
        .add_solid_fill_layer_above(raster, "Fill", [0.25, 0.5, 1.0])
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 0]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [64, 128, 255, 255],
        1,
    );
}

#[test]
fn solid_fill_shader_blend_uses_constant_color_source() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let fill = document
        .layer_tree
        .add_solid_fill_layer_above(raster, "Fill", [0.5, 0.25, 1.0])
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(fill, LayerBlendMode::Multiply);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [128, 200, 80, 255]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [64, 50, 80, 255],
        2,
    );
}

#[test]
fn brightness_adjustment_preserves_alpha_and_uses_opacity_and_mask() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let adjustment = document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "Brightness",
            Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
                brightness: 100,
                contrast: 0,
            }),
        )
        .unwrap();
    document.layer_tree.set_opacity(adjustment, 0.5);
    assert!(document.layer_tree.add_layer_mask(adjustment));
    let mask = PaintSurfaceId::layer_mask(0.into(), adjustment);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [32, 64, 96, 128]),
            GpuDocumentCommand::CreateSurface {
                target: mask,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            upload_surface_command(mask, size, [0, 0, 0, 128]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [39, 74, 103, 128],
        3,
    );
}

#[test]
fn levels_adjustment_applies_master_gamma() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let mut levels = LevelsAdjustment::default();
    levels.master.gamma = 2.0;
    document
        .layer_tree
        .add_adjustment_layer_above(raster, "Levels", Adjustment::Levels(levels))
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [64, 64, 64, 255]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [128, 128, 128, 255],
        2,
    );
}

#[test]
fn hue_saturation_adjustment_rotates_red_to_green() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "Hue / Saturation",
            Adjustment::HueSaturation(HueSaturationAdjustment {
                hue: 120,
                saturation: 0,
                lightness: 0,
            }),
        )
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [255, 0, 0, 255]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [0, 255, 0, 255],
        2,
    );
}

#[test]
fn invert_adjustment_inverts_rgb_and_preserves_alpha() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    document
        .layer_tree
        .add_adjustment_layer_above(raster, "Invert", Adjustment::Invert)
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [32, 64, 96, 128]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [96, 64, 32, 128],
        2,
    );
}

#[test]
fn curves_adjustment_uses_component_lut_channels() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let mut curves = CurvesAdjustment::default();
    assert!(curves.red.set_point(
        0,
        CurvePoint {
            input: 0,
            output: 255
        }
    ));
    document
        .layer_tree
        .add_adjustment_layer_above(raster, "Curves", Adjustment::Curves(curves))
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [64, 128, 192, 255]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [255, 128, 192, 255],
        2,
    );
}

#[test]
fn gradient_map_uses_photoshop_grayscale_for_lookup() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "Gradient Map",
            Adjustment::GradientMap(GradientMapAdjustment::default()),
        )
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [255, 0, 0, 255]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [77, 77, 77, 255],
        2,
    );
}

#[test]
fn uv_mirror_x_center_copies_positive_side_to_negative_side() {
    let size = [8, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let adjustment = document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "UV Mirror",
            Adjustment::UvMirror(UvMirrorAdjustment::default()),
        )
        .unwrap();

    let row = [
        [16, 0, 0, 255],
        [32, 0, 0, 255],
        [48, 0, 0, 255],
        [64, 0, 0, 255],
        [80, 0, 0, 255],
        [96, 0, 0, 255],
        [112, 0, 0, 255],
        [0, 0, 0, 0],
    ];
    let mut rgba = Vec::with_capacity((size[0] * size[1] * 4) as usize);
    for _ in 0..size[1] {
        for pixel in row {
            rgba.extend_from_slice(&pixel);
        }
    }
    let surface = PaintSurfaceId::raster(0.into(), raster);
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 0]),
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: RectU32::full(size),
                rgba8: rgba,
                document_revision: None,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );

    let composite = read_composite(&mut engine);
    let expected = [
        row[7], row[6], row[5], row[4], row[4], row[5], row[6], row[7],
    ];
    for y in 0..size[1] {
        for x in 0..size[0] {
            assert_pixel_near(pixel_at(&composite, x, y), expected[x as usize], 1);
        }
    }

    let updated = Adjustment::UvMirror(UvMirrorAdjustment {
        axis: UvMirrorAxis::X,
        position: 0.75,
        direction: UvMirrorDirection::PositiveToNegative,
    });
    execute_ok(
        &mut engine,
        vec![
            GpuDocumentCommand::UpdateAdjustment {
                layer_id: adjustment,
                adjustment: updated.clone(),
            }
            .into(),
        ],
    );

    let composite = read_composite(&mut engine);
    let expected = [
        row[0], row[1], row[2], row[3], row[7], row[6], row[6], row[7],
    ];
    for y in 0..size[1] {
        for x in 0..size[0] {
            assert_pixel_near(pixel_at(&composite, x, y), expected[x as usize], 1);
        }
    }
}

#[test]
fn uv_mirror_partial_and_active_preview_match_full_for_remote_source_dirty() {
    let size = [16, 4];
    let Some(mut partial_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_engine = gpu_engine(size).expect("same GPU adapter should still be available");
    let mut full_engine = gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "UV Mirror",
            Adjustment::UvMirror(UvMirrorAdjustment::default()),
        )
        .unwrap();
    let surface = PaintSurfaceId::raster(0.into(), raster);

    for engine in [&mut partial_engine, &mut active_engine, &mut full_engine] {
        execute_ok(
            engine,
            vec![
                upload_scene_command(&document, [0, 0, 0, 0]),
                sync_tree_command(&document),
            ],
        );
    }
    prime_full_tree_active_surface_set(&mut active_engine, PaintSurfaceSet::single(surface), size);

    let dirty_rect = RectU32 {
        origin: [12, 1],
        size: [1, 1],
    };
    let partial_metrics = execute_ok(
        &mut partial_engine,
        vec![
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: dirty_rect,
                rgba8: vec![255, 0, 0, 255],
                document_revision: None,
            }
            .into(),
        ],
    );
    assert!(partial_metrics.partial_composite_executions > 0);
    assert_eq!(partial_metrics.full_composite_executions, 0);

    let active_metrics = execute_ok(
        &mut active_engine,
        vec![
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: dirty_rect,
                rgba8: vec![255, 0, 0, 255],
                document_revision: None,
            }
            .into(),
        ],
    );
    assert!(active_metrics.partial_composite_executions > 0);
    assert_eq!(active_metrics.full_composite_executions, 0);
    assert_active_preview_no_upload_or_general_fallback(&active_metrics);

    let full_metrics = execute_ok(
        &mut full_engine,
        vec![
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: dirty_rect,
                rgba8: vec![255, 0, 0, 255],
                document_revision: None,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );
    assert!(full_metrics.full_composite_executions > 0);

    let partial = read_composite(&mut partial_engine);
    let active = read_composite(&mut active_engine);
    let full = read_composite(&mut full_engine);
    assert_snapshot_near(&partial, &full, 1);
    assert_snapshot_near(&active, &full, 1);
    assert_pixel_near(pixel_at(&partial, 12, 1), [255, 0, 0, 255], 1);
    assert_pixel_near(pixel_at(&partial, 3, 1), [255, 0, 0, 255], 1);
}

fn assert_uv_mirror_inside_isolated_group_stroke_commit_matches_full(same_batch: bool) {
    let size = [32, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let root_raster = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_raster, "Isolated")
        .unwrap();
    let raster = document
        .layer_tree
        .add_raster_layer_to_group(group, "Paint")
        .unwrap();
    document
        .layer_tree
        .add_adjustment_layer_to_group(
            group,
            "UV Mirror",
            Adjustment::UvMirror(UvMirrorAdjustment::default()),
        )
        .unwrap();
    let surface = PaintSurfaceId::raster(0.into(), raster);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 0]),
            GpuDocumentCommand::CreateSurface {
                target: surface,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );

    let params = external_source_over_paint_params(1.0, 1.0, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 0.08,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 0.0, 0.0],
            params,
        },
    });
    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: Arc::new(ActiveSelection::disabled_for_materials([0.into()])),
            }
            .into(),
        ],
    );

    let source_damage = RectU32 {
        origin: [21, 1],
        size: [7, 6],
    };
    let mut damage = DamageMap::default();
    damage.add_rect(surface, source_damage);
    let add_dabs: PlanCommand = StrokeCommand::AddDabs {
        dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.75, 0.5), 1.0)]),
        preview_damage: Some(damage.clone()),
    }
    .into();
    let end: PlanCommand = StrokeCommand::End {
        damage: Some(damage),
    }
    .into();
    let active_preview = if same_batch {
        execute_ok(&mut engine, vec![add_dabs, end]);
        None
    } else {
        execute_ok(&mut engine, vec![add_dabs]);
        let active_preview = read_composite(&mut engine);
        execute_ok(&mut engine, vec![end]);
        Some(active_preview)
    };
    let committed = read_composite(&mut engine);

    execute_ok(&mut engine, vec![sync_tree_command(&document)]);
    let forced_full = read_composite(&mut engine);
    assert!(
        pixel_at(&forced_full, 7, 4)[3] > 0,
        "forced full composite must contain the remote mirrored dab"
    );
    if let Some(active_preview) = active_preview {
        assert_snapshot_near(&active_preview, &forced_full, 1);
    }
    assert_snapshot_near(&committed, &forced_full, 1);
}

#[test]
fn uv_mirror_inside_isolated_group_stays_correct_after_stroke_commit() {
    assert_uv_mirror_inside_isolated_group_stroke_commit_matches_full(false);
}

#[test]
fn uv_mirror_inside_isolated_group_same_batch_stroke_commit_matches_full() {
    assert_uv_mirror_inside_isolated_group_stroke_commit_matches_full(true);
}

fn assert_uv_mirror_group_partial_update_matches_forced_full(mode: GroupCompositeMode) {
    let size = [16, 4];
    let Some(mut cached_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut full_engine = gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_raster = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_raster, "UV Mirror Group")
        .unwrap();
    if mode == GroupCompositeMode::PassThrough {
        assert!(document.layer_tree.set_group_composite_mode(group, mode));
    }
    let raster = document
        .layer_tree
        .add_raster_layer_to_group(group, "Paint")
        .unwrap();
    document
        .layer_tree
        .add_adjustment_layer_to_group(
            group,
            "UV Mirror",
            Adjustment::UvMirror(UvMirrorAdjustment::default()),
        )
        .unwrap();
    let surface = PaintSurfaceId::raster(0.into(), raster);

    for engine in [&mut cached_engine, &mut full_engine] {
        execute_ok(
            engine,
            vec![
                upload_scene_command(&document, [0, 0, 0, 0]),
                GpuDocumentCommand::CreateSurface {
                    target: surface,
                    initial: LayerInitialPixels::Transparent,
                }
                .into(),
                sync_tree_command(&document),
            ],
        );
    }

    let source_rect = RectU32 {
        origin: [12, 1],
        size: [1, 1],
    };
    execute_ok(
        &mut cached_engine,
        vec![
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: source_rect,
                rgba8: vec![255, 0, 0, 255],
                document_revision: None,
            }
            .into(),
        ],
    );
    execute_ok(
        &mut full_engine,
        vec![
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: source_rect,
                rgba8: vec![255, 0, 0, 255],
                document_revision: None,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );

    let cached = read_composite(&mut cached_engine);
    let forced_full = read_composite(&mut full_engine);
    assert_pixel_near(pixel_at(&cached, 12, 1), [255, 0, 0, 255], 1);
    assert_pixel_near(pixel_at(&forced_full, 3, 1), [255, 0, 0, 255], 1);
    assert_snapshot_near(&cached, &forced_full, 1);
}

#[test]
fn uv_mirror_inside_pass_through_group_partial_update_matches_forced_full() {
    assert_uv_mirror_group_partial_update_matches_forced_full(GroupCompositeMode::PassThrough);
}

#[test]
fn uv_mirror_inside_isolated_group_partial_update_matches_forced_full() {
    assert_uv_mirror_group_partial_update_matches_forced_full(GroupCompositeMode::Isolated);
}

#[test]
fn uv_mirror_off_center_partial_matches_full_at_dirty_edge() {
    let size = [16, 4];
    let Some(mut partial_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut full_engine = gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "UV Mirror",
            Adjustment::UvMirror(UvMirrorAdjustment {
                axis: UvMirrorAxis::X,
                position: 0.53,
                direction: UvMirrorDirection::PositiveToNegative,
            }),
        )
        .unwrap();
    let surface = PaintSurfaceId::raster(0.into(), raster);
    let initial = solid_rgba(size, [180, 180, 180, 255]);
    for engine in [&mut partial_engine, &mut full_engine] {
        execute_ok(
            engine,
            vec![
                upload_scene_command(&document, [0, 0, 0, 0]),
                GpuDocumentCommand::UploadSurfaceRgba8 {
                    surface,
                    rect: RectU32::full(size),
                    rgba8: initial.clone(),
                    document_revision: None,
                }
                .into(),
                sync_tree_command(&document),
            ],
        );
    }

    let dirty_rect = RectU32 {
        origin: [12, 1],
        size: [1, 1],
    };
    let partial_metrics = execute_ok(
        &mut partial_engine,
        vec![
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: dirty_rect,
                rgba8: vec![255, 0, 0, 255],
                document_revision: None,
            }
            .into(),
        ],
    );
    assert!(partial_metrics.partial_composite_executions > 0);

    execute_ok(
        &mut full_engine,
        vec![
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: dirty_rect,
                rgba8: vec![255, 0, 0, 255],
                document_revision: None,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );

    let partial = read_composite(&mut partial_engine);
    let full = read_composite(&mut full_engine);
    assert_snapshot_near(&partial, &full, 1);
}

#[test]
fn uv_mirror_blur_preview_matches_forced_full_composite() {
    let size = [32, 16];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "UV Mirror",
            Adjustment::UvMirror(UvMirrorAdjustment::default()),
        )
        .unwrap();
    let surface = PaintSurfaceId::raster(0.into(), raster);

    let mut rgba = solid_rgba(size, [224, 224, 224, 255]);
    for y in 0..size[1] {
        for x in 22..26 {
            let index = ((y * size[0] + x) as usize) * 4;
            rgba[index..index + 4].copy_from_slice(&[16, 48, 192, 255]);
        }
    }
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 255]),
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect: RectU32 {
                    origin: [0, 0],
                    size,
                },
                rgba8: rgba,
                document_revision: None,
            }
            .into(),
            sync_tree_command(&document),
        ],
    );

    let params = external_blur_params(1.0, 1.0, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "blur".to_owned(),
            radius_world: 0.12,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));
    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface },
                style,
                active_selection: selection,
            }
            .into(),
        ],
    );

    let mut preview_damage = DamageMap::default();
    preview_damage.add_rect(
        surface,
        RectU32 {
            origin: [20, 4],
            size: [9, 8],
        },
    );
    let metrics = execute_ok(
        &mut engine,
        vec![
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.75, 0.5), 1.0)]),
                preview_damage: Some(preview_damage.clone()),
            }
            .into(),
        ],
    );
    assert!(metrics.partial_composite_executions > 0);
    let partial = read_composite(&mut engine);

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::End {
                damage: Some(preview_damage),
            }
            .into(),
        ],
    );
    let committed = read_composite(&mut engine);
    assert_eq!(
        engine.texture_metrics().checkpoint_texture_count,
        0,
        "committing the stroke should release active-preview checkpoints"
    );

    execute_ok(&mut engine, vec![sync_tree_command(&document)]);
    let full = read_composite(&mut engine);
    assert_snapshot_near(&partial, &full, 1);
    assert_snapshot_near(&committed, &full, 1);
}

#[test]
fn adjustment_gpu_patch_updates_existing_composite_tree() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let adjustment = document
        .layer_tree
        .add_adjustment_layer_above(
            raster,
            "Brightness",
            Adjustment::BrightnessContrast(Default::default()),
        )
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [64, 128, 192, 255]),
            sync_tree_command(&document),
        ],
    );
    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [64, 128, 192, 255],
        1,
    );

    execute_ok(
        &mut engine,
        vec![
            GpuDocumentCommand::UpdateAdjustment {
                layer_id: adjustment,
                adjustment: Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
                    brightness: 100,
                    contrast: 0,
                }),
            }
            .into(),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [123, 210, 245, 255],
        1,
    );
}

#[test]
fn adjustment_value_change_reuses_unchanged_root_prefix_checkpoint() {
    let size = [4, 4];
    let Some(mut cached_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut reference_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let mut anchor = document.layer_tree.default_raster_layer().unwrap();
    let mut uploads = Vec::new();
    for index in 0..4 {
        anchor = document
            .layer_tree
            .add_raster_layer_above(anchor, format!("prefix layer {index}"))
            .unwrap();
        uploads.push((
            PaintSurfaceId::raster(0.into(), anchor),
            [
                40 + index as u8 * 30,
                170 - index as u8 * 20,
                60 + index as u8 * 25,
                70 + index as u8 * 25,
            ],
        ));
    }
    let adjustment = document
        .layer_tree
        .add_adjustment_layer_above(
            anchor,
            "Brightness",
            Adjustment::BrightnessContrast(Default::default()),
        )
        .unwrap();

    run_scene_upload(&mut cached_engine, &document, &uploads, [20, 30, 40, 255]);
    run_scene_upload(
        &mut reference_engine,
        &document,
        &uploads,
        [20, 30, 40, 255],
    );

    let updated = Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
        brightness: 35,
        contrast: 20,
    });
    let metrics = execute_ok(
        &mut cached_engine,
        vec![
            GpuDocumentCommand::UpdateAdjustment {
                layer_id: adjustment,
                adjustment: updated.clone(),
            }
            .into(),
        ],
    );
    assert!(document.layer_tree.set_adjustment(adjustment, updated));
    execute_ok(&mut reference_engine, vec![sync_tree_command(&document)]);

    assert!(
        metrics.composite_checkpoint_reuses > 0,
        "adjustment changes above the prefix should reuse the unchanged root checkpoint"
    );
    assert_snapshot_near(
        &read_composite(&mut cached_engine),
        &read_composite(&mut reference_engine),
        2,
    );
}

#[test]
fn adjustment_inside_isolated_and_pass_through_groups_uses_expected_backdrop() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(raster, "Group")
        .unwrap();
    document
        .layer_tree
        .add_adjustment_layer_to_group(
            group,
            "Brightness",
            Adjustment::BrightnessContrast(BrightnessContrastAdjustment {
                brightness: 100,
                contrast: 0,
            }),
        )
        .unwrap();

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [64, 128, 192, 255]),
            sync_tree_command(&document),
        ],
    );
    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [64, 128, 192, 255],
        1,
    );

    assert!(
        document
            .layer_tree
            .set_group_composite_mode(group, GroupCompositeMode::PassThrough)
    );
    execute_ok(&mut engine, vec![sync_tree_command(&document)]);

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [123, 210, 245, 255],
        1,
    );
}

#[test]
fn solid_fill_r8_layer_mask_attenuates_gpu_composite() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let raster = document.layer_tree.default_raster_layer().unwrap();
    let fill = document
        .layer_tree
        .add_solid_fill_layer_above(raster, "Fill", [200.0 / 255.0, 80.0 / 255.0, 40.0 / 255.0])
        .unwrap();
    assert!(document.layer_tree.add_layer_mask(fill));
    let mask = PaintSurfaceId::layer_mask(0.into(), fill);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 0]),
            GpuDocumentCommand::CreateSurface {
                target: mask,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            upload_surface_command(mask, size, [0, 0, 0, 128]),
            sync_tree_command(&document),
        ],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [100, 40, 20, 128],
        2,
    );
}

#[test]
fn r8_layer_mask_attenuates_normal_gpu_composite() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let layer = document.layer_tree.default_raster_layer().unwrap();
    assert!(document.layer_tree.add_layer_mask(layer));
    let mask = PaintSurfaceId::layer_mask(0.into(), layer);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [200, 80, 40, 255]),
            GpuDocumentCommand::CreateSurface {
                target: mask,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            upload_surface_command(mask, size, [0, 0, 0, 128]),
            sync_tree_command(&document),
        ],
    );

    let mask_pixel = first_pixel(&read_surface(&mut engine, mask));
    assert_eq!(mask_pixel[3], 128);

    let composite = read_composite(&mut engine);
    assert_pixel_near(first_pixel(&composite), [100, 40, 20, 128], 2);
}

#[test]
fn disabled_layer_mask_is_ignored_by_gpu_composite() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let layer = document.layer_tree.default_raster_layer().unwrap();
    assert!(document.layer_tree.add_layer_mask(layer));
    document.layer_tree.set_layer_mask_enabled(layer, false);
    let mask = PaintSurfaceId::layer_mask(0.into(), layer);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [200, 80, 40, 255]),
            GpuDocumentCommand::CreateSurface {
                target: mask,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            upload_surface_command(mask, size, [0, 0, 0, 0]),
            sync_tree_command(&document),
        ],
    );

    let composite = read_composite(&mut engine);
    assert_pixel_near(first_pixel(&composite), [200, 80, 40, 255], 1);
}

#[test]
fn r8_layer_mask_attenuates_shader_blend_gpu_composite() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let bottom = document.layer_tree.default_raster_layer().unwrap();
    let top = document
        .layer_tree
        .add_raster_layer_above(bottom, "screen top")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(top, LayerBlendMode::Screen);
    assert!(document.layer_tree.add_layer_mask(top));

    let top_surface = PaintSurfaceId::raster(0.into(), top);
    let top_mask = PaintSurfaceId::layer_mask(0.into(), top);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 200, 0, 255]),
            GpuDocumentCommand::CreateSurface {
                target: top_surface,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            upload_surface_command(top_surface, size, [200, 0, 0, 255]),
            GpuDocumentCommand::CreateSurface {
                target: top_mask,
                initial: LayerInitialPixels::Transparent,
            }
            .into(),
            upload_surface_command(top_mask, size, [0, 0, 0, 128]),
            sync_tree_command(&document),
        ],
    );

    let composite = read_composite(&mut engine);
    assert_pixel_near(first_pixel(&composite), [100, 200, 0, 255], 3);
}

#[test]
fn uv_stroke_on_layer_mask_proxy_resolves_to_r8_and_updates_composite() {
    let size = [8, 8];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let layer = document.layer_tree.default_raster_layer().unwrap();
    assert!(document.layer_tree.add_layer_mask(layer));
    let mask = PaintSurfaceId::layer_mask(0.into(), layer);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [180, 60, 20, 255]),
            GpuDocumentCommand::CreateSurface {
                target: mask,
                initial: LayerInitialPixels::SolidRgba8([0, 0, 0, 0]),
            }
            .into(),
            sync_tree_command(&document),
        ],
    );
    assert_pixel_near(first_pixel(&read_composite(&mut engine)), [0, 0, 0, 0], 1);

    let params = external_source_over_paint_params(1.0, 1.0, 1.0);
    let style = Arc::new(RendererStrokeStyle {
        stroke_op: StrokeOp::BrushEngine {
            engine_id: "paint".to_owned(),
            radius_world: 2.0,
            radius_pressure: PressureDynamics::default(),
            params: params.clone(),
            param_dynamics: Vec::new(),
            surface_source_material_scope: brush_footprint_source_scope(),
        },
        operation: RendererStrokeOperation::BrushEngine {
            color: [1.0, 1.0, 1.0],
            params,
        },
    });
    let selection = Arc::new(ActiveSelection::disabled_for_materials([0.into()]));

    execute_ok(
        &mut engine,
        vec![
            StrokeCommand::Begin {
                target: StrokeTarget::Uv { surface: mask },
                style,
                active_selection: selection,
            }
            .into(),
            StrokeCommand::AddDabs {
                dabs: StrokeDabPayload::Stroke(vec![StrokeDab::new(Vec2::new(0.5, 0.5), 1.0)]),
                preview_damage: None,
            }
            .into(),
            StrokeCommand::End {
                damage: Some({
                    let mut damage = DamageMap::default();
                    damage.add_rect(
                        mask,
                        RectU32 {
                            origin: [0, 0],
                            size,
                        },
                    );
                    damage
                }),
            }
            .into(),
        ],
    );

    let mask_snapshot = read_surface(&mut engine, mask);
    assert_pixel_near(first_pixel(&mask_snapshot), [255, 255, 255, 255], 2);

    let composite = read_composite(&mut engine);
    assert_pixel_near(first_pixel(&composite), [180, 60, 20, 255], 3);
}

#[test]
fn decal_on_layer_mask_resolves_only_the_planned_proxy_region() {
    let size = [32, 32];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let layer = document.layer_tree.default_raster_layer().unwrap();
    assert!(document.layer_tree.add_layer_mask(layer));
    let mask = PaintSurfaceId::layer_mask(0.into(), layer);
    let transform = DecalTransform {
        center_world: Vec3::new(0.35, 0.35, 0.0),
        axis_x_world: Vec3::X,
        axis_y_world: Vec3::Y,
        normal_world: Vec3::Z,
        size_world: Vec2::splat(0.2),
        projection_depth_world: 1.0,
    };
    let plan = DecalApplyPlan::build(&document, &[mask], transform)
        .expect("valid layer mask Decal should produce an apply plan");
    assert_eq!(plan.targets.len(), 1);
    assert_ne!(plan.targets[0].damage_rects, vec![RectU32::full(size)]);

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [180, 60, 20, 255]),
            GpuDocumentCommand::CreateSurface {
                target: mask,
                initial: LayerInitialPixels::SolidRgba8([0, 0, 0, 0]),
            }
            .into(),
            sync_tree_command(&document),
        ],
    );
    execute_ok(
        &mut engine,
        vec![
            ApplyCommand::Decal {
                plan,
                image: Arc::new(DecalImageAsset {
                    id: DecalImageId(17),
                    file_name: "layer-mask-decal.png".to_owned(),
                    size: [2, 2],
                    rgba8: Arc::from(solid_rgba([2, 2], [255, 255, 255, 255])),
                }),
                projection: holopainter::core::decal::DecalProjection::Surface(transform),
                opacity: 1.0,
                active_selection: ActiveSelection::disabled_for_materials([0.into()]),
            }
            .into(),
        ],
    );

    let mask_snapshot = read_surface(&mut engine, mask);
    assert_pixel_near(pixel_at(&mask_snapshot, 11, 11), [255, 255, 255, 255], 2);
    assert_pixel_near(pixel_at(&mask_snapshot, 1, 1), [0, 0, 0, 0], 0);

    let composite = read_composite(&mut engine);
    assert_pixel_near(pixel_at(&composite, 11, 11), [180, 60, 20, 255], 3);
    assert_pixel_near(pixel_at(&composite, 1, 1), [0, 0, 0, 0], 1);
}

#[test]
fn surface_decal_preview_keeps_viewport_depth_attachment_size() {
    let texture_size = [16, 16];
    let Some(mut engine) = gpu_engine(texture_size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let document = document(texture_size);
    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [255, 255, 255, 255])],
    );

    let viewport_size = [545, 932];
    let camera = OrbitCamera::default();
    let view_proj =
        camera.view_projection_matrix(viewport_size[0] as f32 / viewport_size[1] as f32);
    let transform = DecalTransform {
        center_world: Vec3::new(0.5, 0.5, 0.0),
        axis_x_world: Vec3::X,
        axis_y_world: Vec3::Y,
        normal_world: Vec3::Z,
        size_world: Vec2::ONE,
        projection_depth_world: 1.0,
    };
    execute_ok(
        &mut engine,
        vec![
            ViewCommand::RenderViewport {
                camera: camera.clone(),
                view_proj,
                camera_world: camera.position().to_array(),
                viewport_size,
                brush_overlay_request: None,
                selection_overlay_request: None,
                mirror_plane_overlay_request: None,
                decal_overlay_request: Some(DecalOverlayRequest {
                    image: Arc::new(DecalImageAsset {
                        id: DecalImageId(99),
                        file_name: "large-preview.png".to_owned(),
                        size: [800, 800],
                        rgba8: Arc::from(solid_rgba([800, 800], [255, 0, 0, 255])),
                    }),
                    projection: DecalProjection::Surface(transform),
                    opacity: 1.0,
                    scene_visibility: ViewportSceneVisibility::default(),
                }),
                scene_visibility: ViewportSceneVisibility::default(),
                show_wireframe: false,
                wireframe_style: WireframeStyle::new([1.0, 1.0, 1.0], 1.0),
                background_color: [0.0, 0.0, 0.0],
                shading: ViewportShading::Unlit,
            }
            .into(),
        ],
    );
}

#[test]
fn view_projection_decal_preview_reuses_main_depth() {
    let texture_size = [16, 16];
    let Some(mut engine) = gpu_engine(texture_size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let document = document(texture_size);
    execute_ok(
        &mut engine,
        vec![upload_scene_command(&document, [255, 255, 255, 255])],
    );

    let viewport_size = [320, 180];
    let camera = OrbitCamera::default();
    let view_proj =
        camera.view_projection_matrix(viewport_size[0] as f32 / viewport_size[1] as f32);
    let texture_count_before = engine.texture_metrics().view_texture_count;
    execute_ok(
        &mut engine,
        vec![
            ViewCommand::RenderViewport {
                camera: camera.clone(),
                view_proj,
                camera_world: camera.position().to_array(),
                viewport_size,
                brush_overlay_request: None,
                selection_overlay_request: None,
                mirror_plane_overlay_request: None,
                decal_overlay_request: Some(DecalOverlayRequest {
                    image: Arc::new(DecalImageAsset {
                        id: DecalImageId(100),
                        file_name: "view-projection-preview.png".to_owned(),
                        size: [2, 2],
                        rgba8: Arc::from(solid_rgba([2, 2], [255, 0, 0, 255])),
                    }),
                    projection: DecalProjection::ViewProjection {
                        projector_view_proj: view_proj,
                        viewport_view_proj: view_proj,
                        depth_size: viewport_size,
                    },
                    opacity: 1.0,
                    scene_visibility: ViewportSceneVisibility::default(),
                }),
                scene_visibility: ViewportSceneVisibility::default(),
                show_wireframe: false,
                wireframe_style: WireframeStyle::new([1.0, 1.0, 1.0], 1.0),
                background_color: [0.0, 0.0, 0.0],
                shading: ViewportShading::Unlit,
            }
            .into(),
        ],
    );

    // Only the shared Decal image texture is new. A dedicated two-texture
    // Decal depth target would increase this count by three instead.
    assert_eq!(
        engine.texture_metrics().view_texture_count,
        texture_count_before + 1
    );
}

#[test]
fn viewport_presentation_applies_hemisphere_shade_only_when_enabled() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let document = Document::new(
        overlapping_material_planes(&[0.0]),
        vec![MaterialSpec::new("Material", size)],
    );
    execute_ok(
        &mut engine,
        upload_scene_with_materials_and_sync(&document, &[[200, 100, 40, 255]]),
    );

    let mut camera = OrbitCamera::default();
    camera.target = Vec3::ZERO;
    camera.orientation = Quat::IDENTITY;
    camera.distance = 3.0;

    assert_pixel_near(
        render_viewport_center_sample_with_shading(
            &mut engine,
            camera.clone(),
            ViewportShading::Unlit,
        ),
        [200, 100, 40, 255],
        2,
    );
    assert_pixel_near(
        render_viewport_center_sample_with_shading(&mut engine, camera, ViewportShading::Shade),
        [155, 78, 31, 255],
        2,
    );
}

#[test]
fn viewport_presentation_blends_transparent_material_over_opaque_background() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let mut front = MaterialSpec::new("Blue glass", size);
    front.render_settings = MaterialRenderSettings {
        render_mode: MaterialRenderMode::Blend,
        ..MaterialRenderSettings::default()
    };
    let back = MaterialSpec::new("Red back", size);
    let document = Document::new(overlapping_material_planes(&[0.0, -0.2]), vec![front, back]);
    execute_ok(
        &mut engine,
        upload_scene_with_materials_and_sync(&document, &[[0, 0, 128, 128], [255, 0, 0, 255]]),
    );

    let mut camera = OrbitCamera::default();
    camera.target = Vec3::ZERO;
    camera.orientation = Quat::IDENTITY;
    camera.distance = 3.0;
    let pixel = render_viewport_center_sample(&mut engine, camera);

    assert_pixel_near(pixel, [127, 0, 128, 255], 3);
}

#[test]
fn viewport_presentation_sorts_multiple_blend_sub_meshes_back_to_front() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let blend = |name| {
        let mut material = MaterialSpec::new(name, size);
        material.render_settings.render_mode = MaterialRenderMode::Blend;
        material
    };
    let document = Document::new(
        overlapping_material_planes(&[0.0, -0.1, -0.2]),
        vec![
            blend("Blue front"),
            blend("Green middle"),
            MaterialSpec::new("Red back", size),
        ],
    );
    execute_ok(
        &mut engine,
        upload_scene_with_materials_and_sync(
            &document,
            &[[0, 0, 128, 128], [0, 128, 0, 128], [255, 0, 0, 255]],
        ),
    );

    let mut camera = OrbitCamera::default();
    camera.target = Vec3::ZERO;
    camera.orientation = Quat::IDENTITY;
    camera.distance = 3.0;
    let pixel = render_viewport_center_sample(&mut engine, camera);

    assert_pixel_near(pixel, [63, 64, 128, 255], 4);
}

#[test]
fn viewport_presentation_cutoff_updates_immediately_and_opaque_unpremultiplies() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let mut front = MaterialSpec::new("Blue front", size);
    front.render_settings.render_mode = MaterialRenderMode::Opaque;
    let document = Document::new(
        overlapping_material_planes(&[0.0, -0.2]),
        vec![front, MaterialSpec::new("Red back", size)],
    );
    execute_ok(
        &mut engine,
        upload_scene_with_materials_and_sync(&document, &[[0, 0, 100, 100], [255, 0, 0, 255]]),
    );
    let mut camera = OrbitCamera::default();
    camera.target = Vec3::ZERO;
    camera.orientation = Quat::IDENTITY;
    camera.distance = 3.0;
    assert_pixel_near(
        render_viewport_center_sample(&mut engine, camera.clone()),
        [0, 0, 255, 255],
        2,
    );

    execute_ok(
        &mut engine,
        vec![
            GpuDocumentCommand::SetMaterialRenderSettings {
                material_index: 0,
                settings: MaterialRenderSettings {
                    render_mode: MaterialRenderMode::Cutoff,
                    alpha_cutoff: 128,
                    ..MaterialRenderSettings::default()
                },
            }
            .into(),
        ],
    );
    assert_pixel_near(
        render_viewport_center_sample(&mut engine, camera),
        [255, 0, 0, 255],
        2,
    );
}

#[test]
fn cutoff_zero_keeps_zero_alpha_texel_as_opaque_black() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let mut material = MaterialSpec::new("Zero alpha cutoff", size);
    material.render_settings = MaterialRenderSettings {
        render_mode: MaterialRenderMode::Cutoff,
        alpha_cutoff: 0,
        ..MaterialRenderSettings::default()
    };
    let document = Document::new(overlapping_material_planes(&[0.0]), vec![material]);
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&document, [0, 0, 0, 0]),
            sync_tree_command(&document),
        ],
    );
    let mut camera = OrbitCamera::default();
    camera.target = Vec3::ZERO;
    camera.orientation = Quat::IDENTITY;
    camera.distance = 3.0;

    assert_pixel_near(
        render_viewport_center_sample(&mut engine, camera),
        [0, 0, 0, 255],
        1,
    );
}

#[test]
fn material_presentation_resources_follow_scene_reuploads() {
    let size = [2, 2];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU test: no wgpu adapter/device available");
        return;
    };
    let mut camera = OrbitCamera::default();
    camera.target = Vec3::ZERO;
    camera.orientation = Quat::IDENTITY;
    camera.distance = 3.0;

    let one_material = Document::new(
        overlapping_material_planes(&[0.0]),
        vec![MaterialSpec::new("One", size)],
    );
    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&one_material, [255, 0, 0, 255]),
            sync_tree_command(&one_material),
        ],
    );
    assert_pixel_near(
        render_viewport_center_sample(&mut engine, camera.clone()),
        [255, 0, 0, 255],
        1,
    );

    let mut cutoff_front = MaterialSpec::new("Cutoff", size);
    cutoff_front.render_settings = MaterialRenderSettings {
        render_mode: MaterialRenderMode::Cutoff,
        alpha_cutoff: 200,
        ..MaterialRenderSettings::default()
    };
    let three_materials = Document::new(
        overlapping_material_planes(&[0.0, -0.1, -0.2]),
        vec![
            cutoff_front,
            MaterialSpec::new("Green", size),
            MaterialSpec::new("Red", size),
        ],
    );
    execute_ok(
        &mut engine,
        upload_scene_with_materials_and_sync(
            &three_materials,
            &[[0, 0, 64, 64], [0, 255, 0, 255], [255, 0, 0, 255]],
        ),
    );
    assert_pixel_near(
        render_viewport_center_sample(&mut engine, camera.clone()),
        [0, 255, 0, 255],
        1,
    );

    execute_ok(
        &mut engine,
        vec![
            upload_scene_command(&one_material, [0, 0, 255, 255]),
            sync_tree_command(&one_material),
        ],
    );
    assert_pixel_near(
        render_viewport_center_sample(&mut engine, camera),
        [0, 0, 255, 255],
        1,
    );
}

#[test]
fn active_preview_single_active_raster_uses_no_upload_path() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let document = document(size);
    let active_surface = PaintSurfaceId::raster(
        0.into(),
        document.layer_tree.default_raster_layer().unwrap(),
    );

    run_scene_upload(&mut full_engine, &document, &[], [40, 80, 120, 255]);
    run_scene_upload(&mut active_run_engine, &document, &[], [40, 80, 120, 255]);
    prime_active_run_working_set(&mut active_run_engine, active_surface, size);

    let updated = [210, 30, 70, 255];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        2,
    );
}

#[test]
fn active_preview_active_layer_below_inactive_normal_layer_uses_no_upload_path() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let background = document.layer_tree.default_raster_layer().unwrap();
    let active = document
        .layer_tree
        .add_raster_layer_above(background, "active normal")
        .unwrap();
    let top = document
        .layer_tree
        .add_raster_layer_above(active, "inactive normal")
        .unwrap();
    document.layer_tree.set_opacity(top, 0.5);

    let active_surface = PaintSurfaceId::raster(0.into(), active);
    let uploads = [
        (active_surface, [180, 40, 80, 255]),
        (PaintSurfaceId::raster(0.into(), top), [20, 200, 60, 128]),
    ];
    run_scene_upload(&mut full_engine, &document, &uploads, [10, 20, 30, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [10, 20, 30, 255],
    );
    prime_active_run_working_set(&mut active_run_engine, active_surface, size);

    let updated = [40, 100, 220, 255];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        2,
    );
}

#[test]
fn active_preview_nested_multi_active_matches_full_composite() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_background, "normal group")
        .unwrap();
    let active_a = document
        .layer_tree
        .add_raster_layer_to_group(group, "active a")
        .unwrap();
    let static_mid = document
        .layer_tree
        .add_raster_layer_to_group(group, "static mid")
        .unwrap();
    let active_b = document
        .layer_tree
        .add_raster_layer_to_group(group, "active b")
        .unwrap();
    document.layer_tree.set_opacity(group, 0.8);
    document.layer_tree.set_opacity(static_mid, 0.5);

    let active_a_surface = PaintSurfaceId::raster(0.into(), active_a);
    let active_b_surface = PaintSurfaceId::raster(0.into(), active_b);
    let uploads = [
        (active_a_surface, [200, 20, 40, 255]),
        (
            PaintSurfaceId::raster(0.into(), static_mid),
            [20, 200, 40, 128],
        ),
        (active_b_surface, [40, 20, 220, 192]),
    ];
    run_scene_upload(&mut full_engine, &document, &uploads, [30, 60, 90, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [30, 60, 90, 255],
    );
    prime_active_surface_set(
        &mut active_run_engine,
        PaintSurfaceSet::from_vec(vec![active_a_surface, active_b_surface]),
        size,
    );

    let update_a = [20, 220, 180, 255];
    let update_b = [240, 180, 20, 192];
    execute_ok(
        &mut full_engine,
        vec![
            upload_surface_command(active_a_surface, size, update_a),
            upload_surface_command(active_b_surface, size, update_b),
        ],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![
            upload_surface_command(active_a_surface, size, update_a),
            upload_surface_command(active_b_surface, size, update_b),
        ],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn active_preview_sibling_group_multi_active_matches_full_composite() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let left_group = document
        .layer_tree
        .add_group_above(root_background, "left group")
        .unwrap();
    let left_active = document
        .layer_tree
        .add_raster_layer_to_group(left_group, "left active")
        .unwrap();
    let left_static = document
        .layer_tree
        .add_raster_layer_to_group(left_group, "left static")
        .unwrap();
    let right_group = document
        .layer_tree
        .add_group_above(left_group, "right group")
        .unwrap();
    let right_static = document
        .layer_tree
        .add_raster_layer_to_group(right_group, "right static")
        .unwrap();
    let right_active = document
        .layer_tree
        .add_raster_layer_to_group(right_group, "right active")
        .unwrap();
    document.layer_tree.set_opacity(left_static, 0.5);
    document.layer_tree.set_opacity(right_static, 0.5);

    let left_active_surface = PaintSurfaceId::raster(0.into(), left_active);
    let right_active_surface = PaintSurfaceId::raster(0.into(), right_active);
    let uploads = [
        (left_active_surface, [220, 20, 20, 220]),
        (
            PaintSurfaceId::raster(0.into(), left_static),
            [20, 120, 220, 128],
        ),
        (
            PaintSurfaceId::raster(0.into(), right_static),
            [120, 220, 20, 128],
        ),
        (right_active_surface, [20, 20, 220, 220]),
    ];
    run_scene_upload(&mut full_engine, &document, &uploads, [40, 50, 60, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [40, 50, 60, 255],
    );
    prime_active_surface_set(
        &mut active_run_engine,
        PaintSurfaceSet::from_vec(vec![left_active_surface, right_active_surface]),
        size,
    );

    let update_left = [20, 220, 180, 220];
    let update_right = [240, 160, 20, 220];
    execute_ok(
        &mut full_engine,
        vec![
            upload_surface_command(left_active_surface, size, update_left),
            upload_surface_command(right_active_surface, size, update_right),
        ],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![
            upload_surface_command(left_active_surface, size, update_left),
            upload_surface_command(right_active_surface, size, update_right),
        ],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn active_run_non_normal_leaf_matches_full_composite_after_active_upload() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_background, "normal group")
        .unwrap();
    let group_below = document
        .layer_tree
        .add_raster_layer_to_group(group, "group below")
        .unwrap();
    let active = document
        .layer_tree
        .add_raster_layer_to_group(group, "active multiply")
        .unwrap();
    let group_above = document
        .layer_tree
        .add_raster_layer_to_group(group, "group above")
        .unwrap();
    let root_above = document
        .layer_tree
        .add_raster_layer_above(group, "root above")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(active, LayerBlendMode::Multiply);
    document.layer_tree.set_opacity(group_above, 0.5);
    document.layer_tree.set_opacity(root_above, 0.25);

    let active_surface = PaintSurfaceId::raster(0.into(), active);
    let uploads = [
        (
            PaintSurfaceId::raster(0.into(), group_below),
            [180, 100, 40, 255],
        ),
        (active_surface, [80, 220, 160, 255]),
        (
            PaintSurfaceId::raster(0.into(), group_above),
            [20, 40, 200, 255],
        ),
        (
            PaintSurfaceId::raster(0.into(), root_above),
            [240, 10, 30, 255],
        ),
    ];

    run_scene_upload(&mut full_engine, &document, &uploads, [50, 120, 200, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [50, 120, 200, 255],
    );
    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        2,
    );

    prime_active_run_working_set(&mut active_run_engine, active_surface, size);
    let updated = [220, 40, 120, 255];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert!(
        active_run_engine.texture_metrics().checkpoint_texture_count > 0,
        "active-run update should keep checkpoint textures alive"
    );
    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn active_run_non_normal_ancestor_group_matches_full_composite_after_active_upload() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_background, "screen group")
        .unwrap();
    let group_below = document
        .layer_tree
        .add_raster_layer_to_group(group, "group below")
        .unwrap();
    let active = document
        .layer_tree
        .add_raster_layer_to_group(group, "active normal")
        .unwrap();
    let group_above = document
        .layer_tree
        .add_raster_layer_to_group(group, "group above multiply")
        .unwrap();
    let root_above = document
        .layer_tree
        .add_raster_layer_above(group, "root above")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(group, LayerBlendMode::Screen);
    document.layer_tree.set_opacity(group, 0.7);
    document
        .layer_tree
        .set_blend_mode(group_above, LayerBlendMode::Multiply);
    document.layer_tree.set_opacity(root_above, 0.35);

    let active_surface = PaintSurfaceId::raster(0.into(), active);
    let uploads = [
        (
            PaintSurfaceId::raster(0.into(), group_below),
            [30, 180, 70, 255],
        ),
        (active_surface, [210, 40, 140, 255]),
        (
            PaintSurfaceId::raster(0.into(), group_above),
            [120, 80, 220, 255],
        ),
        (
            PaintSurfaceId::raster(0.into(), root_above),
            [10, 230, 40, 255],
        ),
    ];

    run_scene_upload(&mut full_engine, &document, &uploads, [70, 120, 190, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [70, 120, 190, 255],
    );
    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        2,
    );

    prime_active_run_working_set(&mut active_run_engine, active_surface, size);
    let updated = [60, 210, 200, 255];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn active_run_non_normal_leaf_preserves_isolated_group_backdrop() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_background, "isolated group")
        .unwrap();
    let active = document
        .layer_tree
        .add_raster_layer_to_group(group, "active multiply")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(active, LayerBlendMode::Multiply);

    let active_surface = PaintSurfaceId::raster(0.into(), active);
    let uploads = [(active_surface, [200, 20, 40, 255])];
    run_scene_upload(&mut full_engine, &document, &uploads, [0, 200, 0, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [0, 200, 0, 255],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut full_engine)),
        [200, 20, 40, 255],
        2,
    );
    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        2,
    );

    prime_active_run_working_set(&mut active_run_engine, active_surface, size);
    let updated = [40, 10, 230, 255];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    let expected = [40, 10, 230, 255];
    assert_pixel_near(first_pixel(&read_composite(&mut full_engine)), expected, 2);
    assert_pixel_near(
        first_pixel(&read_composite(&mut active_run_engine)),
        expected,
        2,
    );
}

#[test]
fn active_run_raster_inside_group_mask_matches_full_composite_after_active_upload() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_background, "masked group")
        .unwrap();
    let active = document
        .layer_tree
        .add_raster_layer_to_group(group, "active child")
        .unwrap();
    document.layer_tree.set_opacity(group, 0.9);
    assert!(document.layer_tree.add_layer_mask(group));

    let active_surface = PaintSurfaceId::raster(0.into(), active);
    let group_mask = PaintSurfaceId::layer_mask(0.into(), group);
    let uploads = [
        (active_surface, [220, 60, 30, 255]),
        (group_mask, [0, 0, 0, 128]),
    ];

    run_scene_upload(&mut full_engine, &document, &uploads, [30, 90, 150, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [30, 90, 150, 255],
    );
    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );

    prime_active_run_working_set(&mut active_run_engine, active_surface, size);
    let updated = [40, 210, 180, 255];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn active_run_mask_edit_inside_non_normal_group_matches_full_composite() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_run_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let root_background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(root_background, "multiply group")
        .unwrap();
    let group_below = document
        .layer_tree
        .add_raster_layer_to_group(group, "group below")
        .unwrap();
    let masked = document
        .layer_tree
        .add_raster_layer_to_group(group, "masked active")
        .unwrap();
    let root_above = document
        .layer_tree
        .add_raster_layer_above(group, "root above")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(group, LayerBlendMode::Multiply);
    document.layer_tree.set_opacity(group, 0.8);
    assert!(document.layer_tree.add_layer_mask(masked));

    let masked_surface = PaintSurfaceId::raster(0.into(), masked);
    let active_mask = PaintSurfaceId::layer_mask(0.into(), masked);
    let uploads = [
        (
            PaintSurfaceId::raster(0.into(), group_below),
            [180, 180, 30, 255],
        ),
        (masked_surface, [20, 120, 240, 255]),
        (active_mask, [0, 0, 0, 96]),
        (
            PaintSurfaceId::raster(0.into(), root_above),
            [240, 40, 20, 255],
        ),
    ];

    run_scene_upload(&mut full_engine, &document, &uploads, [80, 200, 160, 255]);
    run_scene_upload(
        &mut active_run_engine,
        &document,
        &uploads,
        [80, 200, 160, 255],
    );
    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );

    prime_active_run_working_set(&mut active_run_engine, active_mask, size);
    let updated_mask = [0, 0, 0, 210];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_mask, size, updated_mask)],
    );
    let active_update_metrics = execute_ok(
        &mut active_run_engine,
        vec![upload_surface_command(active_mask, size, updated_mask)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);

    assert_snapshot_near(
        &read_composite(&mut active_run_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn gated_pass_through_opacity_and_mask_blend_against_parent_backdrop() {
    let size = [4, 4];
    let Some(mut engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut document = document(size);
    let background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(background, "gated pass through")
        .unwrap();
    let child = document
        .layer_tree
        .add_raster_layer_to_group(group, "multiply child")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(child, LayerBlendMode::Multiply);
    document.layer_tree.set_opacity(group, 0.5);
    assert!(document.layer_tree.add_layer_mask(group));
    assert!(
        document
            .layer_tree
            .set_group_composite_mode(group, GroupCompositeMode::PassThrough)
    );

    run_scene_upload(
        &mut engine,
        &document,
        &[
            (PaintSurfaceId::raster(0.into(), child), [200, 100, 50, 255]),
            (PaintSurfaceId::layer_mask(0.into(), group), [0, 0, 0, 128]),
        ],
        [100, 100, 100, 255],
    );

    assert_pixel_near(
        first_pixel(&read_composite(&mut engine)),
        [95, 85, 80, 255],
        3,
    );
}

#[test]
fn active_preview_gated_pass_through_matches_full_composite_without_upload() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_engine = gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(background, "gated pass through")
        .unwrap();
    let static_child = document
        .layer_tree
        .add_raster_layer_to_group(group, "static child")
        .unwrap();
    let active = document
        .layer_tree
        .add_raster_layer_to_group(group, "active multiply")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(active, LayerBlendMode::Multiply);
    document.layer_tree.set_opacity(group, 0.7);
    assert!(document.layer_tree.add_layer_mask(group));
    assert!(
        document
            .layer_tree
            .set_group_composite_mode(group, GroupCompositeMode::PassThrough)
    );

    let active_surface = PaintSurfaceId::raster(0.into(), active);
    let uploads = [
        (
            PaintSurfaceId::raster(0.into(), static_child),
            [170, 40, 80, 180],
        ),
        (active_surface, [40, 210, 90, 255]),
        (PaintSurfaceId::layer_mask(0.into(), group), [0, 0, 0, 190]),
    ];
    run_scene_upload(&mut full_engine, &document, &uploads, [80, 120, 160, 255]);
    run_scene_upload(&mut active_engine, &document, &uploads, [80, 120, 160, 255]);
    assert_snapshot_near(
        &read_composite(&mut active_engine),
        &read_composite(&mut full_engine),
        3,
    );

    let prime_metrics = prime_full_tree_active_surface_set(
        &mut active_engine,
        PaintSurfaceSet::single(active_surface),
        size,
    );
    assert!(prime_metrics.active_composite_source_cache_uploads > 0);

    let updated = [210, 70, 200, 255];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_engine,
        vec![upload_surface_command(active_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);
    assert_snapshot_near(
        &read_composite(&mut active_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn active_preview_gated_pass_through_mask_matches_full_composite_without_upload() {
    let size = [4, 4];
    let Some(mut full_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut active_engine = gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let background = document.layer_tree.default_raster_layer().unwrap();
    let group = document
        .layer_tree
        .add_group_above(background, "gated pass through")
        .unwrap();
    let child = document
        .layer_tree
        .add_raster_layer_to_group(group, "multiply child")
        .unwrap();
    document
        .layer_tree
        .set_blend_mode(child, LayerBlendMode::Multiply);
    document.layer_tree.set_opacity(group, 0.8);
    assert!(document.layer_tree.add_layer_mask(group));
    assert!(
        document
            .layer_tree
            .set_group_composite_mode(group, GroupCompositeMode::PassThrough)
    );

    let mask_surface = PaintSurfaceId::layer_mask(0.into(), group);
    let uploads = [
        (PaintSurfaceId::raster(0.into(), child), [190, 80, 220, 255]),
        (mask_surface, [0, 0, 0, 96]),
    ];
    run_scene_upload(&mut full_engine, &document, &uploads, [70, 130, 90, 255]);
    run_scene_upload(&mut active_engine, &document, &uploads, [70, 130, 90, 255]);

    let prime_metrics = prime_full_tree_active_surface_set(
        &mut active_engine,
        PaintSurfaceSet::single(mask_surface),
        size,
    );
    assert!(prime_metrics.active_composite_source_cache_uploads > 0);

    let updated = [0, 0, 0, 224];
    execute_ok(
        &mut full_engine,
        vec![upload_surface_command(mask_surface, size, updated)],
    );
    let active_update_metrics = execute_ok(
        &mut active_engine,
        vec![upload_surface_command(mask_surface, size, updated)],
    );
    assert_active_preview_no_upload_or_general_fallback(&active_update_metrics);
    assert_snapshot_near(
        &read_composite(&mut active_engine),
        &read_composite(&mut full_engine),
        3,
    );
}

#[test]
fn general_composite_reuses_unchanged_isolated_group_cache() {
    let size = [4, 4];
    let Some(mut cached_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut reference_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let background = document.layer_tree.default_raster_layer().unwrap();
    let lower_group = document
        .layer_tree
        .add_group_above(background, "lower isolated")
        .unwrap();
    let lower_child = document
        .layer_tree
        .add_raster_layer_to_group(lower_group, "lower child")
        .unwrap();
    let upper_group = document
        .layer_tree
        .add_group_above(lower_group, "upper isolated")
        .unwrap();
    let upper_child = document
        .layer_tree
        .add_raster_layer_to_group(upper_group, "upper child")
        .unwrap();

    let lower_surface = PaintSurfaceId::raster(0.into(), lower_child);
    let upper_surface = PaintSurfaceId::raster(0.into(), upper_child);
    let uploads = [
        (lower_surface, [190, 40, 80, 170]),
        (upper_surface, [30, 180, 220, 150]),
    ];
    run_scene_upload(&mut cached_engine, &document, &uploads, [60, 90, 120, 255]);
    run_scene_upload(
        &mut reference_engine,
        &document,
        &uploads,
        [60, 90, 120, 255],
    );

    let updated = [220, 130, 20, 190];
    let metrics = execute_ok(
        &mut cached_engine,
        vec![upload_surface_command(upper_surface, size, updated)],
    );
    execute_ok(
        &mut reference_engine,
        vec![
            upload_surface_command(upper_surface, size, updated),
            sync_tree_command(&document),
        ],
    );

    assert!(
        metrics.composite_checkpoint_reuses > 0,
        "unchanged isolated sibling should reuse its subtree cache"
    );
    assert_snapshot_near(
        &read_composite(&mut cached_engine),
        &read_composite(&mut reference_engine),
        3,
    );
}

#[test]
fn general_composite_reuses_root_prefix_checkpoint_for_upper_layer_change() {
    let size = [4, 4];
    let Some(mut cached_engine) = gpu_engine(size) else {
        eprintln!("skipping GPU integration test: no wgpu adapter/device available");
        return;
    };
    let mut reference_engine =
        gpu_engine(size).expect("same GPU adapter should still be available");
    let mut document = document(size);
    let mut anchor = document.layer_tree.default_raster_layer().unwrap();
    let mut added_layers = Vec::new();
    for index in 0..4 {
        anchor = document
            .layer_tree
            .add_raster_layer_above(anchor, format!("root layer {index}"))
            .unwrap();
        added_layers.push(anchor);
    }
    let uploads = added_layers
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            (
                PaintSurfaceId::raster(0.into(), *layer),
                [
                    30 + index as u8 * 35,
                    180 - index as u8 * 25,
                    70 + index as u8 * 30,
                    80 + index as u8 * 30,
                ],
            )
        })
        .collect::<Vec<_>>();
    run_scene_upload(&mut cached_engine, &document, &uploads, [40, 60, 100, 255]);
    run_scene_upload(
        &mut reference_engine,
        &document,
        &uploads,
        [40, 60, 100, 255],
    );

    let upper_surface = PaintSurfaceId::raster(0.into(), *added_layers.last().unwrap());
    let updated = [230, 50, 170, 210];
    let metrics = execute_ok(
        &mut cached_engine,
        vec![upload_surface_command(upper_surface, size, updated)],
    );
    execute_ok(
        &mut reference_engine,
        vec![
            upload_surface_command(upper_surface, size, updated),
            sync_tree_command(&document),
        ],
    );

    assert!(
        metrics.composite_checkpoint_reuses > 0,
        "upper-layer changes should reuse the unchanged root prefix checkpoint"
    );
    assert_snapshot_near(
        &read_composite(&mut cached_engine),
        &read_composite(&mut reference_engine),
        3,
    );
}

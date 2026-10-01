use std::sync::Arc;

use anyhow::{Context, Result};
use eframe::egui_wgpu::wgpu;

use crate::{
    core::{
        brush_engine::BrushEngineRegistry,
        render_report::GpuTextureMetrics,
        selection::ActiveSelection,
        stroke::PaintSurfaceSet,
        surface::{LayerTree, PaintSurfaceId},
        texture::TextureCatalog,
    },
    renderer::{
        command::{StrokeDabPayload, StrokeTarget},
        document::{
            materials::MaterialRegistry, scene::SceneStore, selection::SelectionMasks,
            surfaces::SurfaceRepository,
        },
        engine::gpu_state::RendererGpuState,
        features::brush::{
            BrushEngineRunner, engine::BrushEngineDeps, resources::BrushGpuResources,
        },
        gpu::{frame::GpuFrame, texture::create_render_scratch_texture},
        scene_capture::{SceneCapture, SceneCapturePipelines},
        transient::TransientTextures,
        view::{ToolBrushPreview, ToolPreviewItem, ToolPreviewKind, ToolPreviewRequest},
    },
};

const DEFAULT_PREVIEW_SIZE: [u32; 2] = [1, 1];
const BRIGHT_BACKGROUND: [u8; 4] = [255, 255, 255, 255];
const DARK_BACKGROUND: [u8; 4] = [48, 48, 48, 255];
const DEFAULT_TARGET_MATERIAL: usize = 0;

pub(crate) struct ToolPreviewFeature {
    brush: BrushEngineRunner,
    resources: BrushGpuResources,
    materials: MaterialRegistry,
    surfaces: SurfaceRepository,
    scene: SceneStore,
    selections: SelectionMasks,
    target_surface: PaintSurfaceId,
    output_size: [u32; 2],
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    cached_request: Option<ToolPreviewRequest>,
}

impl ToolPreviewFeature {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        registry: &BrushEngineRegistry,
        textures: &TextureCatalog,
    ) -> Result<Self> {
        let brush = BrushEngineRunner::new(device, queue, registry, textures)?;
        let layer_tree = LayerTree::new_default_raster();
        let target_layer = layer_tree
            .default_raster_layer()
            .context("preview layer tree does not contain a raster layer")?;
        let target_surface = PaintSurfaceId::raster(DEFAULT_TARGET_MATERIAL.into(), target_layer);
        let (output, output_view) =
            create_render_scratch_texture(device, DEFAULT_PREVIEW_SIZE, "tool_preview_output");
        Ok(Self {
            brush,
            resources: BrushGpuResources::new(device),
            materials: MaterialRegistry::default(),
            surfaces: SurfaceRepository::new(),
            scene: SceneStore::new(device),
            selections: SelectionMasks::new(),
            target_surface,
            output_size: DEFAULT_PREVIEW_SIZE,
            output,
            output_view,
            cached_request: None,
        })
    }

    pub(crate) fn register_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        definition: &crate::core::texture::TextureResourceDefinition,
    ) -> Result<()> {
        self.brush.register_texture(device, queue, definition)?;
        self.cached_request = None;
        Ok(())
    }

    pub(crate) fn unregister_texture(&mut self, id: &str) {
        self.brush.unregister_texture(id);
        self.cached_request = None;
    }

    pub(crate) fn register_engine(
        &mut self,
        device: &wgpu::Device,
        engine: &crate::core::brush_engine::BrushEngineDefinition,
    ) -> Result<()> {
        self.brush.register_engine(device, engine)?;
        self.cached_request = None;
        Ok(())
    }

    pub(crate) fn unregister_engine(&mut self, id: &str) {
        self.brush.unregister_engine(id);
        self.cached_request = None;
    }

    pub(crate) fn texture_view(&self) -> &wgpu::TextureView {
        &self.output_view
    }

    pub(crate) fn texture_metrics(&self) -> GpuTextureMetrics {
        let mut metrics = GpuTextureMetrics::default();
        metrics.add_total_bytes(rgba8_texture_bytes(self.output_size));
        metrics.view_bytes = metrics
            .view_bytes
            .saturating_add(rgba8_texture_bytes(self.output_size));
        metrics.view_texture_count = metrics.view_texture_count.saturating_add(1);
        metrics.merge(self.surfaces.texture_metrics());
        metrics
    }

    pub(crate) fn render(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        scratch: &mut TransientTextures,
        scene_capture: &mut SceneCapture,
        scene_capture_pipelines: &SceneCapturePipelines,
        request: ToolPreviewRequest,
    ) -> Result<()> {
        let item_size = normalized_item_size(request.item_size);
        let atlas_size = atlas_size(item_size, request.items.len());
        let texture_changed = self.ensure_output_size(gpu.device(), atlas_size);
        if !texture_changed && self.cached_request.as_ref() == Some(&request) {
            return Ok(());
        }

        self.materials
            .replace_with_sizes(gpu.device(), frame, [item_size]);
        let atlas = build_preview_atlas(item_size, &request.items);
        frame.write_texture_rgba8(gpu.device(), &self.output, [0, 0], atlas_size, &atlas);

        for (index, item) in request.items.iter().enumerate() {
            if let ToolPreviewKind::Brush(brush_preview) = &item.kind {
                self.render_brush_item(
                    frame,
                    gpu,
                    scratch,
                    scene_capture,
                    scene_capture_pipelines,
                    item_size,
                    index,
                    brush_preview,
                )?;
            }
        }

        self.cached_request = Some(request);
        Ok(())
    }

    fn ensure_output_size(&mut self, device: &wgpu::Device, size: [u32; 2]) -> bool {
        if self.output_size == size {
            return false;
        }
        self.output_size = size;
        let (output, output_view) =
            create_render_scratch_texture(device, size, "tool_preview_output");
        self.output = output;
        self.output_view = output_view;
        self.cached_request = None;
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn render_brush_item(
        &mut self,
        frame: &mut GpuFrame,
        gpu: &mut RendererGpuState,
        scratch: &mut TransientTextures,
        scene_capture: &mut SceneCapture,
        scene_capture_pipelines: &SceneCapturePipelines,
        item_size: [u32; 2],
        item_index: usize,
        brush_preview: &ToolBrushPreview,
    ) -> Result<()> {
        let background = split_background_rgba8(item_size);
        self.surfaces.create_surface_record_from_rgba8_into_frame(
            gpu,
            frame,
            self.target_surface,
            item_size,
            &background,
        )?;

        let active_selection = Arc::new(ActiveSelection::disabled_for_materials([
            DEFAULT_TARGET_MATERIAL.into(),
        ]));
        let mut deps = BrushEngineDeps {
            gpu,
            stroke_resources: &mut self.resources,
            materials: &self.materials,
            surfaces: &mut self.surfaces,
            scene: &mut self.scene,
            selections: &mut self.selections,
            scratch,
            scene_capture,
            scene_capture_pipelines,
        };
        let (mut session, _begin_result) = self.brush.begin_stroke(
            frame,
            &mut deps,
            StrokeTarget::Uv {
                surface: self.target_surface,
            },
            &brush_preview.style,
            active_selection,
        )?;
        self.brush.stamp_stroke_payload(
            frame,
            &mut deps,
            &mut session,
            StrokeDabPayload::Stroke(brush_preview.dabs.clone()),
            PaintSurfaceSet::single(self.target_surface),
            None,
        )?;
        self.brush.finalize_stroke(
            frame,
            &mut deps,
            &session,
            None,
            PaintSurfaceSet::single(self.target_surface),
            None,
        )?;

        let source = self
            .surfaces
            .surface_record_texture(self.target_surface)
            .context("preview brush target texture was not created")?;
        frame.encoder().copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.output,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: item_index as u32 * item_size[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: item_size[0],
                height: item_size[1],
                depth_or_array_layers: 1,
            },
        );
        Ok(())
    }
}

fn normalized_item_size(size: [u32; 2]) -> [u32; 2] {
    [size[0].max(1), size[1].max(1)]
}

fn atlas_size(item_size: [u32; 2], item_count: usize) -> [u32; 2] {
    [
        item_size[0],
        item_size[1].saturating_mul(item_count.max(1) as u32).max(1),
    ]
}

fn build_preview_atlas(item_size: [u32; 2], items: &[ToolPreviewItem]) -> Vec<u8> {
    let atlas_size = atlas_size(item_size, items.len());
    let mut atlas = vec![0; rgba8_texture_bytes(atlas_size)];
    if items.is_empty() {
        write_split_background(&mut atlas, atlas_size, 0, atlas_size[1]);
        return atlas;
    }
    for (index, item) in items.iter().enumerate() {
        let row_origin_y = index as u32 * item_size[1];
        write_split_background(&mut atlas, atlas_size, row_origin_y, item_size[1]);
        paint_dummy_preview(&mut atlas, atlas_size, row_origin_y, item_size, &item.kind);
    }
    atlas
}

fn split_background_rgba8(size: [u32; 2]) -> Vec<u8> {
    let mut pixels = vec![0; rgba8_texture_bytes(size)];
    write_split_background(&mut pixels, size, 0, size[1]);
    pixels
}

fn write_split_background(pixels: &mut [u8], texture_size: [u32; 2], origin_y: u32, height: u32) {
    let width = texture_size[0] as usize;
    let center_x = width / 2;
    for y in origin_y..origin_y.saturating_add(height).min(texture_size[1]) {
        let row = y as usize * width * 4;
        for x in 0..width {
            let color = if x < center_x {
                BRIGHT_BACKGROUND
            } else {
                DARK_BACKGROUND
            };
            let offset = row + x * 4;
            pixels[offset..offset + 4].copy_from_slice(&color);
        }
    }
}

fn paint_dummy_preview(
    pixels: &mut [u8],
    texture_size: [u32; 2],
    row_origin_y: u32,
    item_size: [u32; 2],
    kind: &ToolPreviewKind,
) {
    let rect = PreviewRect {
        left: item_size[0] * 18 / 100,
        top: item_size[1] * 22 / 100,
        right: item_size[0] * 82 / 100,
        bottom: item_size[1] * 78 / 100,
    };
    match kind {
        ToolPreviewKind::RectangleErase | ToolPreviewKind::LassoErase => {
            fill_rect(
                pixels,
                texture_size,
                row_origin_y,
                rect,
                [235, 235, 235, 230],
            );
            stroke_rect(
                pixels,
                texture_size,
                row_origin_y,
                rect,
                [70, 70, 70, 255],
                2,
            );
        }
        ToolPreviewKind::Brush(_) | ToolPreviewKind::Empty => {}
    }
}

#[derive(Clone, Copy)]
struct PreviewRect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl PreviewRect {
    fn shrink(self, amount: u32) -> Self {
        Self {
            left: self.left.saturating_add(amount).min(self.right),
            top: self.top.saturating_add(amount).min(self.bottom),
            right: self.right.saturating_sub(amount).max(self.left),
            bottom: self.bottom.saturating_sub(amount).max(self.top),
        }
    }
}

fn fill_rect(
    pixels: &mut [u8],
    texture_size: [u32; 2],
    row_origin_y: u32,
    rect: PreviewRect,
    color: [u8; 4],
) {
    for y in rect.top..rect.bottom {
        for x in rect.left..rect.right {
            blend_pixel(pixels, texture_size, x, row_origin_y + y, color);
        }
    }
}

fn stroke_rect(
    pixels: &mut [u8],
    texture_size: [u32; 2],
    row_origin_y: u32,
    rect: PreviewRect,
    color: [u8; 4],
    thickness: u32,
) {
    for i in 0..thickness {
        let r = rect.shrink(i);
        for x in r.left..r.right {
            blend_pixel(pixels, texture_size, x, row_origin_y + r.top, color);
            blend_pixel(
                pixels,
                texture_size,
                x,
                row_origin_y + r.bottom.saturating_sub(1),
                color,
            );
        }
        for y in r.top..r.bottom {
            blend_pixel(pixels, texture_size, r.left, row_origin_y + y, color);
            blend_pixel(
                pixels,
                texture_size,
                r.right.saturating_sub(1),
                row_origin_y + y,
                color,
            );
        }
    }
}

fn blend_pixel(pixels: &mut [u8], texture_size: [u32; 2], x: u32, y: u32, color: [u8; 4]) {
    if x >= texture_size[0] || y >= texture_size[1] {
        return;
    }
    let index = ((y as usize * texture_size[0] as usize) + x as usize) * 4;
    let alpha = color[3] as f32 / 255.0;
    for channel in 0..3 {
        let dst = pixels[index + channel] as f32;
        let src = color[channel] as f32;
        pixels[index + channel] = (src * alpha + dst * (1.0 - alpha)).round() as u8;
    }
    pixels[index + 3] = 255;
}

fn rgba8_texture_bytes(size: [u32; 2]) -> usize {
    (size[0] as usize)
        .saturating_mul(size[1] as usize)
        .saturating_mul(4)
}

use eframe::egui_wgpu::wgpu;

use anyhow::Result;

use crate::{
    core::{brush_engine::BrushEngineRegistry, texture::TextureCatalog},
    renderer::{
        decal_image::DecalImageCache,
        features::{
            apply::ApplyFeature,
            brush::{BrushEngineRunner, BrushFeature},
            color_sampler::ColorSampler,
            composite::CompositeFeature,
            filter::FilterFeature,
            selection::SelectionFeature,
            tool_preview::ToolPreviewFeature,
            transform::TransformFeature,
            view::ViewFeature,
        },
        scene_capture::SceneCapturePipelines,
    },
};

pub(crate) struct RenderFeatures {
    pub(crate) brush_engines: BrushEngineRegistry,
    pub(crate) brush: BrushFeature,
    pub(crate) decal_images: DecalImageCache,
    pub(crate) color_sampler: ColorSampler,
    pub(crate) apply: ApplyFeature,
    pub(crate) filter: FilterFeature,
    pub(crate) composite: CompositeFeature,
    pub(crate) selection: SelectionFeature,
    pub(crate) view: ViewFeature,
    pub(crate) tool_preview: ToolPreviewFeature,
    pub(crate) transform: TransformFeature,
    pub(crate) scene_capture_pipelines: SceneCapturePipelines,
}

impl RenderFeatures {
    pub(crate) fn register_brush_engine(
        &mut self,
        device: &wgpu::Device,
        definition: &crate::core::brush_engine::BrushEngineDefinition,
    ) -> Result<()> {
        let previous = self.brush_engines.get(&definition.id).cloned();
        self.brush.register_engine(device, definition)?;
        if let Err(error) = self.tool_preview.register_engine(device, definition) {
            if let Some(previous) = previous.as_ref() {
                let _ = self.brush.register_engine(device, previous);
            } else {
                self.brush.unregister_engine(&definition.id);
            }
            return Err(error);
        }
        self.brush_engines.set_validated_user(definition.clone());
        Ok(())
    }

    pub(crate) fn unregister_brush_engine(&mut self, id: &str) {
        self.brush.unregister_engine(id);
        self.tool_preview.unregister_engine(id);
        let _ = self.brush_engines.remove_user(id);
    }

    pub(crate) fn register_brush_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        definition: &crate::core::texture::TextureResourceDefinition,
    ) -> Result<()> {
        self.brush.register_texture(device, queue, definition)?;
        if let Err(error) = self
            .tool_preview
            .register_texture(device, queue, definition)
        {
            self.brush.unregister_texture(&definition.id);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn unregister_brush_texture(&mut self, id: &str) {
        self.brush.unregister_texture(id);
        self.tool_preview.unregister_texture(id);
    }

    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        tex_size: [u32; 2],
        brush_engines: &BrushEngineRegistry,
        textures: &TextureCatalog,
    ) -> Result<Self> {
        let brush = BrushEngineRunner::new(device, queue, brush_engines, textures)?;
        let tool_preview = ToolPreviewFeature::new(device, queue, brush_engines, textures)?;
        Ok(Self {
            brush_engines: brush_engines.clone(),
            brush: BrushFeature::new(device, brush),
            decal_images: DecalImageCache::new(),
            color_sampler: ColorSampler::new(device),
            apply: ApplyFeature::new(device),
            filter: FilterFeature::new(device),
            composite: CompositeFeature::new(device, tex_size),
            selection: SelectionFeature::new(device),
            view: ViewFeature::new(device),
            tool_preview,
            transform: TransformFeature::new(device),
            scene_capture_pipelines: SceneCapturePipelines::new(device),
        })
    }
}

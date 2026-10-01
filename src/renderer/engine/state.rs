use anyhow::Result;
use eframe::egui_wgpu::wgpu;

use crate::renderer::{
    document::GpuDocument, engine::gpu_state::RendererGpuState, presentation::PresentationState,
    transient::TransientTextures,
};

use super::submission::SubmissionState;

pub(crate) struct RendererState {
    pub(crate) gpu: RendererGpuState,
    pub(crate) transient: TransientTextures,
    pub(crate) document: GpuDocument,
    pub(crate) presentation: PresentationState,
    pub(crate) submissions: SubmissionState,
}

impl RendererState {
    pub(crate) fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        tex_size: [u32; 2],
    ) -> Result<Self> {
        let gpu = RendererGpuState::new(device, queue, tex_size)?;
        let transient = TransientTextures::new(gpu.ctx.device());
        let document = GpuDocument::new(gpu.ctx.device());
        let submissions = SubmissionState::new(gpu.ctx.device());
        Ok(Self {
            gpu,
            transient,
            document,
            presentation: PresentationState::default(),
            submissions,
        })
    }
}

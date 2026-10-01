use eframe::egui_wgpu::wgpu;

use crate::renderer::gpu::frame::{GpuFrame, SharedStagingBelt};

#[derive(Debug)]
pub(crate) struct SubmissionState {
    staging_belt: SharedStagingBelt,
}

impl SubmissionState {
    const STAGING_BELT_CHUNK_SIZE: u64 = 64 * 1024;

    pub(crate) fn new(device: &wgpu::Device) -> Self {
        Self {
            staging_belt: std::sync::Arc::new(std::sync::Mutex::new(wgpu::util::StagingBelt::new(
                device.clone(),
                Self::STAGING_BELT_CHUNK_SIZE,
            ))),
        }
    }

    pub(crate) fn create_frame(&mut self, device: &wgpu::Device, label: &'static str) -> GpuFrame {
        GpuFrame::new_with_staging_belt(device, label, self.staging_belt.clone())
    }

    pub(crate) fn submit_frame(
        &mut self,
        queue: &wgpu::Queue,
        frame: GpuFrame,
    ) -> wgpu::SubmissionIndex {
        frame.finish().submit(queue)
    }
}

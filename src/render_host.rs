use anyhow::Result;
use eframe::{egui::TextureId, egui_wgpu::RenderState};

use crate::{
    core::{
        brush_engine::BrushEngineRegistry,
        image::Rgba8Snapshot,
        render_report::{GpuTextureMetrics, RenderMetrics, RenderReport},
        texture::TextureCatalog,
    },
    renderer::{
        CompletedColorSample, CompletedRenderCommit, ReadbackRequest, ReadbackResult,
        RenderCommitArtifacts, RenderCommitId, RenderEngine, RenderExecutionReport,
        RendererFramePlan, StartedRenderCommit, mutation::PresentDirty,
    },
    ui::egui_renderer_presenter::EguiRendererPresenter,
};

#[derive(Debug)]
pub struct FrameRenderResult {
    pub report: RenderReport,
    pub started_commit: std::result::Result<Option<StartedRenderCommit>, String>,
}

pub struct RenderHost {
    backend: RenderEngine,
    presenter: EguiRendererPresenter,
}

impl RenderHost {
    pub fn new(
        rs: &RenderState,
        tex_size: [u32; 2],
        brush_engines: &BrushEngineRegistry,
        brush_textures: &TextureCatalog,
    ) -> Result<Self> {
        let backend = RenderEngine::new_with_brush_resources(
            rs.device.clone(),
            rs.queue.clone(),
            tex_size,
            brush_engines,
            brush_textures,
        )?;
        let presenter = EguiRendererPresenter::new(rs, backend.texture_outputs());
        Ok(Self { backend, presenter })
    }

    fn apply_present_dirty(&mut self, dirty: PresentDirty) {
        self.presenter.sync(self.backend.texture_outputs(), dirty);
    }

    fn execute_backend_plan(&mut self, plan: RendererFramePlan) -> RenderExecutionReport {
        let execution = self.backend.execute(plan);
        self.apply_present_dirty(execution.changes.present_dirty);
        execution
    }

    pub fn texture_id(&self, material_index: usize) -> TextureId {
        self.presenter.texture_id(material_index)
    }

    pub fn viewport_texture_id(&self) -> TextureId {
        self.presenter.viewport_texture_id()
    }

    pub fn uv_view_texture_id(&self) -> TextureId {
        self.presenter.uv_view_texture_id()
    }

    pub fn tool_preview_texture_id(&self) -> TextureId {
        self.presenter.tool_preview_texture_id()
    }

    pub fn texture_size(&self, material_index: usize) -> [usize; 2] {
        self.backend
            .texture_outputs()
            .materials()
            .iter()
            .find(|output| output.material_index() == material_index)
            .map(|output| output.size())
            .unwrap_or([0, 0])
    }

    pub(crate) fn register_brush_texture(
        &mut self,
        definition: &crate::core::texture::TextureResourceDefinition,
    ) -> Result<()> {
        self.backend.register_brush_texture(definition)
    }

    pub(crate) fn unregister_brush_texture(&mut self, id: &str) {
        self.backend.unregister_brush_texture(id);
    }

    pub(crate) fn register_brush_engine(
        &mut self,
        definition: &crate::core::brush_engine::BrushEngineDefinition,
    ) -> Result<()> {
        self.backend.register_brush_engine(definition)
    }

    pub(crate) fn unregister_brush_engine(&mut self, id: &str) {
        self.backend.unregister_brush_engine(id);
    }

    pub fn max_texture_dimension_2d(&self) -> u32 {
        self.backend.max_texture_dimension_2d()
    }

    pub fn read_composite_rgba8(&mut self, material_index: usize) -> Result<Rgba8Snapshot> {
        match self.backend.readback(ReadbackRequest::CompositeRgba8 {
            material_index,
            rect: None,
        })? {
            ReadbackResult::Rgba8(snapshot) => Ok(snapshot),
            ReadbackResult::SelectionMaskR8(_) => {
                anyhow::bail!("composite readback returned a selection mask")
            }
        }
    }

    pub fn take_metrics(&mut self) -> RenderMetrics {
        self.backend.take_metrics()
    }

    pub fn texture_metrics(&self) -> GpuTextureMetrics {
        self.backend.texture_metrics()
    }

    pub fn execute_frame(&mut self, plan: RendererFramePlan) -> FrameRenderResult {
        let (report, started_commit) = if plan.is_empty() {
            (RenderReport::ok(RenderMetrics::default()), Ok(None))
        } else {
            let execution = self.execute_backend_plan(plan);
            let report = execution.report.clone();
            let started_commit = render_commit_started_from_execution(execution);
            (report, started_commit)
        };
        if !report.is_ok() {
            return FrameRenderResult {
                report,
                started_commit: Err("render batch failed before commit readback".to_owned()),
            };
        }

        FrameRenderResult {
            report,
            started_commit,
        }
    }

    pub fn poll_completed_color_samples(&mut self) -> Vec<CompletedColorSample> {
        self.backend.poll_completed_color_samples()
    }

    pub fn poll_completed_commits(&mut self) -> Vec<CompletedRenderCommit> {
        self.backend.poll_completed_commits()
    }

    pub fn accept_commit(
        &mut self,
        commit_id: RenderCommitId,
        artifacts: &RenderCommitArtifacts,
    ) -> std::result::Result<(), String> {
        self.backend.accept_commit(commit_id, artifacts)
    }
}

fn render_commit_started_from_execution(
    execution: RenderExecutionReport,
) -> std::result::Result<Option<StartedRenderCommit>, String> {
    execution.started_commit
}

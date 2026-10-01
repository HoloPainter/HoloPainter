use anyhow::{Result, anyhow};

use crate::{
    core::{
        document::MeshData, image::Rgba8Snapshot, surface::PaintSurfaceId,
        tile_payload::TilePayload,
    },
    renderer::{
        GpuDocumentCommand,
        document::{
            materials::DocumentMaterialUploads,
            selection::SelectionSyncContext,
            surfaces::{SurfaceInitialPixels, SurfaceMutationContext, SurfaceUploadCommit},
        },
        gpu::frame::GpuFrame,
        mutation::MutationLog,
        report::{CommandResult, record_surface_prepare_metrics},
    },
};

use super::{core::RenderEngine, metrics::record_tile_cache_update_metrics};

impl RenderEngine {
    pub(super) fn execute_document_command(
        &mut self,
        frame: &mut GpuFrame,
        command: GpuDocumentCommand,
    ) -> Result<CommandResult> {
        let mut mutations = MutationLog::default();

        match command {
            GpuDocumentCommand::SyncEmbeddedImages { .. }
            | GpuDocumentCommand::UpsertEmbeddedImage { .. }
            | GpuDocumentCommand::RemoveEmbeddedImage { .. }
            | GpuDocumentCommand::SetEmbeddedImagePreview { .. } => {
                unreachable!("embedded image sync is handled by the document plan executor")
            }
            GpuDocumentCommand::UploadScene {
                mesh,
                materials,
                surfaces,
            } => {
                let resources = DocumentMaterialUploads::from_uploads(materials);
                self.upload_scene(frame, &mesh, &resources, &surfaces, &mut mutations)
            }
            GpuDocumentCommand::AppendMaterials { materials } => {
                self.append_materials(frame, &materials, &mut mutations)
            }
            GpuDocumentCommand::ReplaceMesh { mesh } => {
                self.replace_mesh(frame, &mesh, &mut mutations)
            }
            GpuDocumentCommand::SetMaterialRenderSettings {
                material_index,
                settings,
            } => self.set_material_render_settings(frame, material_index, settings, &mut mutations),
            GpuDocumentCommand::ResizeMaterialTexture {
                snapshot,
                active_selection,
            } => self.resize_material_texture(frame, snapshot, active_selection, &mut mutations),
            GpuDocumentCommand::CreateSurface { target, initial } => {
                self.create_surface_texture(frame, target, initial, &mut mutations)
            }
            GpuDocumentCommand::DeleteSurface { target } => {
                self.delete_surface_texture(target, &mut mutations);
                Ok(())
            }
            GpuDocumentCommand::DuplicateSurface { from, to } => {
                self.duplicate_surface_texture(frame, from, to, &mut mutations)
            }
            GpuDocumentCommand::UploadSurfaceRgba8 {
                surface,
                rect,
                rgba8,
                document_revision,
            } => {
                let texture_size = self
                    .state
                    .document
                    .surfaces
                    .surface_record_texture_size(surface)
                    .ok_or_else(|| anyhow!("History upload surface is missing: {surface:?}"))?;
                let snapshot = Rgba8Snapshot::new(texture_size, rect.origin, rect.size, rgba8)
                    .map_err(|err| anyhow!("History snapshot invalid: {err:#}"))?;
                self.upload_surface_rgba8_with_revision(
                    frame,
                    surface,
                    &snapshot,
                    document_revision,
                    &mut mutations,
                )
                .map_err(|err| anyhow!("History upload failed: {err:#}"))
            }
            GpuDocumentCommand::UploadSurfaceTiles {
                surface,
                surface_revision,
                tiles,
            } => self.upload_surface_tiles_with_revision(
                frame,
                surface,
                surface_revision,
                tiles,
                &mut mutations,
            ),
            GpuDocumentCommand::UploadSelectionTiles { .. }
            | GpuDocumentCommand::SyncMaterialTree { .. }
            | GpuDocumentCommand::UpdateLayerProps { .. }
            | GpuDocumentCommand::UpdateAdjustment { .. } => unreachable!(
                "non-surface document command must be handled by execute_document_plan_command"
            ),
        }?;

        Ok(CommandResult::from_mutations(mutations))
    }

    fn set_material_render_settings(
        &mut self,
        frame: &mut GpuFrame,
        material_index: usize,
        settings: crate::core::material::MaterialRenderSettings,
        mutations: &mut MutationLog,
    ) -> Result<()> {
        if !self.state.document.materials.set_render_settings(
            self.state.gpu.device(),
            frame,
            material_index,
            settings,
        )? {
            return Ok(());
        }
        self.state
            .document
            .scene
            .set_material_render_settings(material_index, settings);
        self.state.document.scene.invalidate_all_depth_slots();
        self.features.apply.clear_scene_caches();
        self.features.brush.clear_scene_caches();
        mutations.view.viewport();
        Ok(())
    }

    fn append_materials(
        &mut self,
        frame: &mut GpuFrame,
        materials: &[crate::renderer::MaterialRegistration],
        mutations: &mut MutationLog,
    ) -> Result<()> {
        let maximum = self.state.gpu.device().limits().max_texture_dimension_2d;
        for material in materials {
            anyhow::ensure!(
                material.texture_size[0] > 0 && material.texture_size[1] > 0,
                "material texture size must be > 0"
            );
            anyhow::ensure!(
                material.texture_size[0] <= maximum && material.texture_size[1] <= maximum,
                "material texture size {}x{} exceeds GPU limit {}",
                material.texture_size[0],
                material.texture_size[1],
                maximum
            );
        }
        let first_material_index = self.state.document.materials.material_count();
        self.state
            .document
            .materials
            .append(self.state.gpu.device(), frame, materials);
        for (offset, material) in materials.iter().enumerate() {
            let material_index = first_material_index + offset;
            self.features.composite.ensure_material(
                self.state.gpu.device(),
                material_index,
                material.texture_size,
            );
            mutations.composites.material(material_index);
        }
        Ok(())
    }

    fn replace_mesh(
        &mut self,
        frame: &mut GpuFrame,
        mesh: &MeshData,
        mutations: &mut MutationLog,
    ) -> Result<()> {
        self.state
            .document
            .scene
            .upload_mesh(&self.state.gpu, frame, mesh);
        self.features
            .filter
            .upload_scene(self.state.gpu.device(), frame, mesh)?;
        self.features.apply.clear_scene_caches();
        self.features.brush.clear_scene_caches();
        for material_index in 0..self.state.document.materials.material_count() {
            if let Some(settings) = self
                .state
                .document
                .materials
                .render_settings(material_index)
            {
                self.state
                    .document
                    .scene
                    .set_material_render_settings(material_index, settings);
            }
        }
        let material_sizes =
            (0..self.state.document.materials.material_count()).filter_map(|material_index| {
                self.state
                    .document
                    .materials
                    .texture_size(material_index)
                    .map(|size| (material_index, size))
            });
        self.features.brush.prewarm_uv_island_masks(
            &self.state.gpu,
            &self.state.document.scene,
            frame,
            material_sizes,
        );
        mutations.scene.uploaded();
        mutations.view.viewport();
        mutations.view.uv_view();
        Ok(())
    }

    fn resize_material_texture(
        &mut self,
        frame: &mut GpuFrame,
        snapshot: crate::core::document::MaterialTextureStateSnapshot,
        active_selection: crate::core::selection::ActiveSelection,
        mutations: &mut MutationLog,
    ) -> Result<()> {
        let material_index = snapshot.material_index.as_usize();
        let size = snapshot.texture_size;
        let maximum = self.state.gpu.device().limits().max_texture_dimension_2d;
        anyhow::ensure!(
            size[0] > 0 && size[1] > 0,
            "material texture size must be > 0"
        );
        anyhow::ensure!(
            size[0] <= maximum && size[1] <= maximum,
            "material texture size {}x{} exceeds GPU limit {}",
            size[0],
            size[1],
            maximum
        );
        anyhow::ensure!(
            self.state
                .document
                .materials
                .texture_size(material_index)
                .is_some(),
            "material index {} is out of range",
            material_index
        );
        for surface in &snapshot.surfaces {
            anyhow::ensure!(
                surface.surface.material_index() == snapshot.material_index,
                "material resize surface belongs to another material"
            );
            anyhow::ensure!(
                surface.texture_size == size,
                "material resize surface size mismatch"
            );
            anyhow::ensure!(
                self.state
                    .document
                    .surfaces
                    .surface_record_texture_size(surface.surface)
                    .is_some(),
                "material resize surface is missing: {:?}",
                surface.surface
            );
            anyhow::ensure!(
                surface.rgba8.len() == crate::core::image::rgba8_len(size)?,
                "material resize surface payload size mismatch"
            );
        }
        if let Some(selection) = &snapshot.selection_mask {
            anyhow::ensure!(
                selection.texture_size == size,
                "material resize selection size mismatch"
            );
            anyhow::ensure!(
                selection.r8.len() == crate::core::selection::selection_mask_len(size)?,
                "material resize selection payload size mismatch"
            );
        }

        self.state
            .document
            .materials
            .set_texture_size(material_index, size)?;
        for surface in &snapshot.surfaces {
            SurfaceMutationContext::new(&mut self.state.document.surfaces, mutations)
                .replace_surface_from_rgba8_into_frame(
                    &self.state.gpu,
                    frame,
                    surface.surface,
                    size,
                    &surface.rgba8,
                )?;
        }
        SelectionSyncContext::new(&mut self.state.document.selections, mutations)
            .replace_material_mask(
                self.state.gpu.device(),
                frame,
                material_index,
                size,
                snapshot.selection_mask.as_ref(),
                &active_selection,
            )?;
        self.features
            .composite
            .ensure_material(self.state.gpu.device(), material_index, size);
        self.features.apply.clear_scene_caches();
        self.features.brush.clear_scene_caches();
        mutations.composites.material(material_index);
        mutations.view.viewport();
        mutations.view.uv_view();
        Ok(())
    }

    fn upload_scene(
        &mut self,
        frame: &mut GpuFrame,
        mesh: &MeshData,
        resources: &DocumentMaterialUploads,
        surfaces: &[PaintSurfaceId],
        mutations: &mut MutationLog,
    ) -> Result<()> {
        self.state
            .document
            .scene
            .upload_mesh(&self.state.gpu, frame, mesh);
        self.features
            .filter
            .upload_scene(self.state.gpu.device(), frame, mesh)?;
        self.features.apply.clear_scene_caches();
        self.features.brush.clear_scene_caches();
        self.state.document.materials.replace_from_uploads(
            self.state.gpu.device(),
            frame,
            resources,
        );
        for material_index in 0..self.state.document.materials.material_count() {
            if let Some(settings) = self
                .state
                .document
                .materials
                .render_settings(material_index)
            {
                self.state
                    .document
                    .scene
                    .set_material_render_settings(material_index, settings);
            }
        }
        mutations.scene.uploaded();
        SurfaceMutationContext::new(&mut self.state.document.surfaces, mutations)
            .upload_scene_materials_into_frame(
                &self.state.gpu,
                frame,
                &resources.materials,
                surfaces,
            )?;
        for material_index in 0..self.state.document.materials.material_count() {
            let Some(size) = self.state.document.materials.texture_size(material_index) else {
                continue;
            };
            self.features
                .composite
                .ensure_material(self.state.gpu.device(), material_index, size);
        }
        let material_count = self.state.document.materials.material_count();
        self.features
            .composite
            .retain_material_count(material_count);
        self.features.apply.retain_material_count(material_count);
        self.features.brush.retain_material_count(material_count);
        let material_sizes = (0..material_count).filter_map(|material_index| {
            self.state
                .document
                .materials
                .texture_size(material_index)
                .map(|size| (material_index, size))
        });
        self.features.brush.prewarm_uv_island_masks(
            &self.state.gpu,
            &self.state.document.scene,
            frame,
            material_sizes,
        );
        Ok(())
    }

    fn create_surface_texture(
        &mut self,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        initial: SurfaceInitialPixels,
        mutations: &mut MutationLog,
    ) -> Result<()> {
        let size = self
            .state
            .document
            .materials
            .texture_size(target.material_index().as_usize())
            .ok_or_else(|| anyhow!("material does not exist: {}", target.material_index))?;
        SurfaceMutationContext::new(&mut self.state.document.surfaces, mutations)
            .create_surface_texture_into_frame(&self.state.gpu, frame, target, size, initial)?;
        Ok(())
    }

    fn delete_surface_texture(&mut self, target: PaintSurfaceId, mutations: &mut MutationLog) {
        SurfaceMutationContext::new(&mut self.state.document.surfaces, mutations)
            .delete_surface_texture(target);
    }

    fn duplicate_surface_texture(
        &mut self,
        frame: &mut GpuFrame,
        from: PaintSurfaceId,
        to: PaintSurfaceId,
        mutations: &mut MutationLog,
    ) -> Result<()> {
        SurfaceMutationContext::new(&mut self.state.document.surfaces, mutations)
            .duplicate_surface_texture_into_frame(&self.state.gpu, frame, from, to)?;
        Ok(())
    }

    fn upload_surface_rgba8_with_revision(
        &mut self,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        snapshot: &Rgba8Snapshot,
        document_revision: Option<u64>,
        mutations: &mut MutationLog,
    ) -> Result<()> {
        let was_resident = self
            .state
            .document
            .surfaces
            .is_surface_texture_resident(target);
        let upload: SurfaceUploadCommit =
            SurfaceMutationContext::new(&mut self.state.document.surfaces, mutations)
                .upload_rgba8_with_revision(
                    &self.state.gpu,
                    frame,
                    target,
                    snapshot,
                    document_revision,
                )?;
        let metrics = self.metrics.get_mut();
        record_surface_prepare_metrics(metrics, &upload.prepare);
        if was_resident {
            metrics.record_upload(snapshot.texture_size, snapshot.origin, snapshot.size);
        }
        record_tile_cache_update_metrics(metrics, &upload.tile_cache_update);
        Ok(())
    }

    fn upload_surface_tiles_with_revision(
        &mut self,
        frame: &mut GpuFrame,
        target: PaintSurfaceId,
        surface_revision: u64,
        tiles: Vec<TilePayload>,
        mutations: &mut MutationLog,
    ) -> Result<()> {
        let was_resident = self
            .state
            .document
            .surfaces
            .is_surface_texture_resident(target);
        let texture_size = self
            .state
            .document
            .surfaces
            .surface_record_texture_size(target)
            .ok_or_else(|| anyhow!("Document tile upload surface is missing: {target:?}"))?;
        let upload: SurfaceUploadCommit =
            SurfaceMutationContext::new(&mut self.state.document.surfaces, mutations)
                .upload_rgba8_tiles_with_revision(
                    &self.state.gpu,
                    frame,
                    target,
                    &tiles,
                    Some(surface_revision),
                )
                .map_err(|err| anyhow!("Document tile upload failed: {err:#}"))?;
        if was_resident {
            for tile in &tiles {
                self.metrics.get_mut().record_upload(
                    texture_size,
                    tile.rect.origin,
                    tile.rect.size,
                );
            }
        }
        record_tile_cache_update_metrics(self.metrics.get_mut(), &upload.tile_cache_update);
        Ok(())
    }
}

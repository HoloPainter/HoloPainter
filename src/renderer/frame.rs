use crate::{
    core::{
        document::{MaterialTextureStateSnapshot, MeshData},
        geometry::RectU32,
        image::{LayerInitialPixels, MaterialPayload},
        selection::{ActiveSelection, SelectionTilePayload},
        surface::PaintSurfaceId,
        surface::{CompositeProps, CompositeTree, LayerId},
        tile_payload::TilePayload,
    },
    renderer::{
        command::{StrokeCommand, TransformCommand},
        features::{
            apply::ApplyCommand, composite::CompositeCommand, filter::FilterCommand,
            selection::SelectionEditCommand, view::ViewCommand,
        },
        view::ColorSampleRequest,
    },
};

#[derive(Debug, Clone)]
pub struct MaterialUpload {
    pub texture: MaterialPayload,
    pub render_settings: crate::core::material::MaterialRenderSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialRegistration {
    pub texture_size: [u32; 2],
    pub render_settings: crate::core::material::MaterialRenderSettings,
}

#[derive(Debug, Clone, Default)]
pub struct RendererFramePlan {
    pub document_commands: Vec<GpuDocumentCommand>,
    pub edit_commands: Vec<EditCommand>,
    pub view_requests: Vec<ViewRequest>,
    pub color_sample_request: Option<ColorSampleRequest>,
    pub commit_request: Option<CommitRequest>,
}

impl RendererFramePlan {
    pub fn is_empty(&self) -> bool {
        self.document_commands.is_empty()
            && self.edit_commands.is_empty()
            && self.view_requests.is_empty()
            && self.color_sample_request.is_none()
            && self.commit_request.is_none()
    }

    pub(crate) fn command_count(&self) -> usize {
        self.document_commands.len()
            + self.edit_commands.len()
            + self.view_requests.len()
            + usize::from(self.color_sample_request.is_some())
    }
}

#[derive(Debug, Clone)]
pub enum GpuDocumentCommand {
    SyncEmbeddedImages {
        images: Vec<std::sync::Arc<crate::core::embedded_image::EmbeddedImageAsset>>,
    },
    UpsertEmbeddedImage {
        image: std::sync::Arc<crate::core::embedded_image::EmbeddedImageAsset>,
    },
    RemoveEmbeddedImage {
        image_id: crate::core::embedded_image::EmbeddedImageId,
    },
    SetEmbeddedImagePreview {
        layer_id: LayerId,
        material_index: usize,
        transform: Option<crate::core::embedded_image::EmbeddedImageTransform>,
        damage: Option<RectU32>,
    },
    UploadScene {
        mesh: MeshData,
        materials: Vec<MaterialUpload>,
        surfaces: Vec<PaintSurfaceId>,
    },
    AppendMaterials {
        materials: Vec<MaterialRegistration>,
    },
    ReplaceMesh {
        mesh: MeshData,
    },
    SetMaterialRenderSettings {
        material_index: usize,
        settings: crate::core::material::MaterialRenderSettings,
    },
    ResizeMaterialTexture {
        snapshot: MaterialTextureStateSnapshot,
        active_selection: ActiveSelection,
    },
    CreateSurface {
        target: PaintSurfaceId,
        initial: LayerInitialPixels,
    },
    DeleteSurface {
        target: PaintSurfaceId,
    },
    DuplicateSurface {
        from: PaintSurfaceId,
        to: PaintSurfaceId,
    },
    UploadSurfaceRgba8 {
        surface: PaintSurfaceId,
        rect: RectU32,
        rgba8: Vec<u8>,
        document_revision: Option<u64>,
    },
    UploadSurfaceTiles {
        surface: PaintSurfaceId,
        surface_revision: u64,
        tiles: Vec<TilePayload>,
    },
    UploadSelectionTiles {
        active_selection: ActiveSelection,
        tiles: Vec<SelectionTilePayload>,
    },
    SyncMaterialTree {
        material_index: usize,
        tree: CompositeTree,
    },
    UpdateLayerProps {
        layer_id: LayerId,
        props: CompositeProps,
    },
    UpdateAdjustment {
        layer_id: LayerId,
        adjustment: crate::core::adjustment::Adjustment,
    },
}

#[derive(Debug, Clone)]
pub enum EditCommand {
    Stroke(StrokeCommand),
    Apply(ApplyCommand),
    Composite(CompositeCommand),
    Filter(FilterCommand),
    Selection(SelectionEditCommand),
    Transform(TransformCommand),
}

pub type ViewRequest = ViewCommand;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommitRequest {
    pub finalized_surfaces: Vec<PaintSurfaceId>,
    pub selection: Option<SelectionCommitRequest>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectionCommitRequest {
    pub before: ActiveSelection,
    pub after: ActiveSelection,
}

impl CommitRequest {
    pub fn is_empty(&self) -> bool {
        self.finalized_surfaces.is_empty() && self.selection.is_none()
    }
}

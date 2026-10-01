use crate::core::{
    embedded_image::EmbeddedImageId,
    surface::{CompositeTree, PaintSurfaceId},
};

#[derive(Debug, Clone)]
pub struct CompositeBakeTarget {
    pub target: PaintSurfaceId,
    pub tree: CompositeTree,
}

#[derive(Debug, Clone)]
pub enum CompositeCommand {
    BakeToSurfaces {
        targets: Vec<CompositeBakeTarget>,
        delete_sources_after_bake: Vec<PaintSurfaceId>,
        delete_embedded_images_after_bake: Vec<EmbeddedImageId>,
    },
}

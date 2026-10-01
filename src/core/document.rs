use anyhow::{Context, Result, bail};
use glam::{Vec2, Vec3};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock},
};
use uuid::Uuid;

use crate::core::composite::{GroupCompositeMode, LayerBlendMode};
use crate::core::selection::{
    ActiveSelection, SelectionMaskSnapshot, SelectionMaskTileStore, selection_mask_len,
};
use crate::core::surface::{
    CompositeGroup, CompositeNode, CompositeProps, CompositeTree, LayerContent, LayerId,
    LayerMergePlan, LayerTree, PaintSurfaceId, RemoveLayerResult,
};
use crate::core::{
    document_tile_store::{AtomicSurfaceChanges, ChangedTiles, DocumentTileStore, InitialPixels},
    embedded_image::{EmbeddedImageAsset, EmbeddedImageId, EmbeddedImageTransform},
    geometry::RectU32,
    image::LayerImageSnapshot,
    image_resize::{resize_mask_rgba8, resize_r8, resize_rgba8},
    material::{append_materials, materialize_materials, restore_materials},
    viewport_visibility::ViewportSceneVisibility,
};

pub use crate::core::material::{
    MaterialData, MaterialId, MaterialIndex, MaterialRenderSettings, MaterialSpec, MaterialUiColor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveLayerTarget {
    Raster,
    EmbeddedImage,
    SolidFill,
    Adjustment,
    LayerMask,
    Structure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveLayerPart {
    Content,
    LayerMask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialTextureStateSnapshot {
    pub material_index: MaterialIndex,
    pub texture_size: [u32; 2],
    pub surfaces: Vec<LayerImageSnapshot>,
    pub selection_mask: Option<SelectionMaskSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialTextureResize {
    pub before: MaterialTextureStateSnapshot,
    pub after: MaterialTextureStateSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubMesh {
    pub mesh_id: MeshId,
    pub start_index: u32,
    pub index_count: u32,
    pub material_index: usize,
    pub material_name: String,
    pub wireframe_edges: Vec<[u32; 2]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub Uuid);

impl ProjectId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MeshObjectId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshObject {
    pub id: MeshId,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHit {
    pub world_pos: Vec3,
    pub world_normal: Vec3,
    pub uv: Vec2,
    pub uv_edge_distance: f32,
    pub uv_paint_boundary_distance: f32,
    pub triangle_index: usize,
    pub material_index: MaterialIndex,
    pub mesh_id: MeshId,
    pub t: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvHit {
    pub uv: Vec2,
    pub triangle_index: usize,
    pub material_index: MaterialIndex,
    pub mesh_id: MeshId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UvPaintEdgeKind {
    Continuous,
    OpenBoundary,
    MaterialBoundary,
    UvDiscontinuity,
    NonManifold,
}

impl UvPaintEdgeKind {
    fn blocks_partial_surface_rect(self) -> bool {
        matches!(
            self,
            Self::MaterialBoundary | Self::UvDiscontinuity | Self::NonManifold
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct PositionKey {
    x: i64,
    y: i64,
    z: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GeometryEdgeKey(PositionKey, PositionKey);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SurfaceFilterTopology {
    pub(crate) triangles: Vec<SurfaceFilterTriangle>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SurfaceFilterTriangle {
    pub(crate) positions: [Vec3; 3],
    pub(crate) uvs: [Vec2; 3],
    pub(crate) material_index: usize,
    pub(crate) mesh_id: MeshId,
    pub(crate) edges: [SurfaceFilterEdge; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurfaceFilterEdge {
    pub(crate) neighbor_triangle: Option<usize>,
    pub(crate) neighbor_edge: usize,
    pub(crate) reversed: bool,
}

impl SurfaceFilterEdge {
    const BOUNDARY: Self = Self {
        neighbor_triangle: None,
        neighbor_edge: 0,
        reversed: false,
    };
}

#[derive(Debug, Clone)]
struct SurfaceFilterEdgeOccurrence {
    triangle_index: usize,
    local_edge: usize,
    position_keys: [PositionKey; 2],
}

#[derive(Debug, Clone)]
struct EdgeOccurrence {
    triangle_index: usize,
    local_edge: usize,
    material_index: usize,
    position_keys: [PositionKey; 2],
    uvs: [Vec2; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct UvPaintEdgeNeighbor {
    material_index: usize,
    position_keys: [PositionKey; 2],
    uvs: [Vec2; 2],
}

#[derive(Debug)]
pub struct MeshData {
    pub positions: Arc<Vec<Vec3>>,
    pub uvs: Arc<Vec<Vec2>>,
    pub normals: Arc<Vec<Vec3>>,
    pub indices: Arc<Vec<[u32; 3]>>,
    pub sub_meshes: Arc<Vec<SubMesh>>,
    pub mesh_objects: Arc<Vec<MeshObject>>,
    pub triangle_mesh_ids: Arc<Vec<MeshId>>,
    pub surface_area_world: f32,
    pub uv_area: f32,
    pub uv_linear_scale: f32,
    scene_bounds_min: Vec3,
    scene_bounds_max: Vec3,
    scene_diagonal: f32,
    raycast_bvh: OnceLock<RaycastBvh>,
    triangle_material_indices: OnceLock<Vec<usize>>,
    triangle_geometry_neighbors: OnceLock<Vec<[Vec<usize>; 3]>>,
    uv_paint_boundary_edges: OnceLock<Vec<[UvPaintEdgeKind; 3]>>,
    uv_paint_edge_neighbors: OnceLock<Vec<[Vec<UvPaintEdgeNeighbor>; 3]>>,
    surface_filter_topology: OnceLock<SurfaceFilterTopology>,
}

#[derive(Debug, Default)]
pub struct RaycastScratch {
    stack: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct RaycastBvh {
    nodes: Vec<RaycastBvhNode>,
    root: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct RaycastBvhNode {
    bounds: Aabb,
    left: Option<usize>,
    right: Option<usize>,
    triangles: Vec<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Aabb {
    min: Vec3,
    max: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
struct TriangleBuildData {
    triangle_index: usize,
    bounds: Aabb,
    centroid: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub project_id: ProjectId,
    pub mesh: MeshData,
    mesh_object_persistent_ids: HashMap<MeshId, MeshObjectId>,
    mesh_object_id_high_water: u64,
    pub materials: Vec<MaterialData>,
    material_id_high_water: u64,
    embedded_images: HashMap<EmbeddedImageId, Arc<EmbeddedImageAsset>>,
    embedded_image_id_high_water: u64,
    pub active_selection: ActiveSelection,
    pub selection_masks: SelectionMaskTileStore,
    pub layer_tree: LayerTree,
    pub tiles: DocumentTileStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerInsertion {
    Above(LayerId),
    IntoGroup(LayerId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedRasterLayer {
    pub layer_id: LayerId,
    pub created_surfaces: Vec<PaintSurfaceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddedEmbeddedImageLayer {
    pub layer_id: LayerId,
    pub image_id: EmbeddedImageId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerTreeSurfaceChange {
    pub deleted_surfaces: Vec<PaintSurfaceId>,
    pub created_surfaces: Vec<ChangedTiles>,
    pub updated_surfaces: Vec<ChangedTiles>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedRasterLayer {
    pub layer_id: LayerId,
    pub created_surfaces: Vec<PaintSurfaceId>,
    pub deleted_surfaces: Vec<PaintSurfaceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshReloadResult {
    pub appended_material_indices: Vec<MaterialIndex>,
    pub created_surfaces: Vec<PaintSurfaceId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedAsset {
    pub mesh: MeshData,
    pub materials: Vec<MaterialSpec>,
}

impl Clone for MeshData {
    fn clone(&self) -> Self {
        Self {
            positions: self.positions.clone(),
            uvs: self.uvs.clone(),
            normals: self.normals.clone(),
            indices: self.indices.clone(),
            sub_meshes: self.sub_meshes.clone(),
            mesh_objects: self.mesh_objects.clone(),
            triangle_mesh_ids: self.triangle_mesh_ids.clone(),
            surface_area_world: self.surface_area_world,
            uv_area: self.uv_area,
            uv_linear_scale: self.uv_linear_scale,
            scene_bounds_min: self.scene_bounds_min,
            scene_bounds_max: self.scene_bounds_max,
            scene_diagonal: self.scene_diagonal,
            raycast_bvh: OnceLock::new(),
            triangle_material_indices: OnceLock::new(),
            triangle_geometry_neighbors: OnceLock::new(),
            uv_paint_boundary_edges: OnceLock::new(),
            uv_paint_edge_neighbors: OnceLock::new(),
            surface_filter_topology: OnceLock::new(),
        }
    }
}

impl PartialEq for MeshData {
    fn eq(&self, other: &Self) -> bool {
        self.positions == other.positions
            && self.uvs == other.uvs
            && self.normals == other.normals
            && self.indices == other.indices
            && self.sub_meshes == other.sub_meshes
            && self.mesh_objects == other.mesh_objects
            && self.triangle_mesh_ids == other.triangle_mesh_ids
            && self.surface_area_world == other.surface_area_world
            && self.uv_area == other.uv_area
            && self.uv_linear_scale == other.uv_linear_scale
            && self.scene_bounds_min == other.scene_bounds_min
            && self.scene_bounds_max == other.scene_bounds_max
            && self.scene_diagonal == other.scene_diagonal
    }
}

impl Document {
    pub(crate) fn restore(
        project_id: ProjectId,
        mesh: MeshData,
        mesh_object_persistent_ids: HashMap<MeshId, MeshObjectId>,
        mesh_object_id_high_water: u64,
        materials: Vec<(MaterialId, MaterialSpec)>,
        material_id_high_water: u64,
        embedded_images: HashMap<EmbeddedImageId, Arc<EmbeddedImageAsset>>,
        embedded_image_id_high_water: u64,
        layer_tree: LayerTree,
        tiles: DocumentTileStore,
    ) -> Self {
        let (materials, material_id_high_water) =
            restore_materials(materials, material_id_high_water);
        let active_selection = ActiveSelection::disabled_for_materials(
            materials.iter().enumerate().map(|(index, _)| index.into()),
        );
        Self {
            project_id,
            mesh,
            mesh_object_persistent_ids,
            mesh_object_id_high_water,
            materials,
            material_id_high_water,
            embedded_images,
            embedded_image_id_high_water,
            active_selection,
            selection_masks: SelectionMaskTileStore::default(),
            layer_tree,
            tiles,
        }
    }

    pub fn content_target(&self, layer_id: LayerId) -> ActiveLayerTarget {
        if self.layer_tree.is_paintable(layer_id) {
            ActiveLayerTarget::Raster
        } else if self.layer_tree.is_solid_fill(layer_id) {
            ActiveLayerTarget::SolidFill
        } else if self.layer_tree.is_adjustment(layer_id) {
            ActiveLayerTarget::Adjustment
        } else if self.layer_tree.is_embedded_image(layer_id) {
            ActiveLayerTarget::EmbeddedImage
        } else {
            ActiveLayerTarget::Structure
        }
    }

    pub fn add_raster_layer(
        &mut self,
        insertion: LayerInsertion,
        name: impl Into<String>,
    ) -> Result<Option<AddedRasterLayer>> {
        let mut layer_tree = self.layer_tree.clone();
        let name = name.into();
        let layer_id = match insertion {
            LayerInsertion::Above(anchor) => layer_tree.add_raster_layer_above(anchor, name),
            LayerInsertion::IntoGroup(parent) => layer_tree.add_raster_layer_to_group(parent, name),
        };
        let Some(layer_id) = layer_id else {
            return Ok(None);
        };
        let surface_specs = self
            .materials
            .iter()
            .enumerate()
            .map(|(material_index, material)| {
                (
                    PaintSurfaceId::raster(material_index.into(), layer_id),
                    material.texture_size,
                    InitialPixels::Transparent,
                )
            })
            .collect::<Vec<_>>();
        let created_surfaces = surface_specs
            .iter()
            .map(|(surface, _, _)| *surface)
            .collect();
        self.tiles.create_surfaces_atomically(surface_specs)?;
        self.layer_tree = layer_tree;
        Ok(Some(AddedRasterLayer {
            layer_id,
            created_surfaces,
        }))
    }

    pub fn add_embedded_image_layer(
        &mut self,
        insertion: LayerInsertion,
        name: impl Into<String>,
        file_name: String,
        size: [u32; 2],
        rgba8: Arc<[u8]>,
        material_index: MaterialIndex,
    ) -> Result<Option<AddedEmbeddedImageLayer>> {
        let material = self
            .materials
            .get(material_index.as_usize())
            .ok_or_else(|| anyhow::anyhow!("embedded image target material is missing"))?;
        let transform = EmbeddedImageTransform::initial(size, material.texture_size)
            .ok_or_else(|| anyhow::anyhow!("embedded image dimensions are invalid"))?;
        let image_id = self.next_embedded_image_id();
        let asset = Arc::new(
            EmbeddedImageAsset::new(image_id, file_name, size, rgba8)
                .ok_or_else(|| anyhow::anyhow!("embedded image pixel payload is invalid"))?,
        );
        let mut tree = self.layer_tree.clone();
        let name = name.into();
        let layer_id = match insertion {
            LayerInsertion::Above(anchor) => {
                tree.add_embedded_image_layer_above(anchor, name, image_id, transform, material.id)
            }
            LayerInsertion::IntoGroup(parent) => tree.add_embedded_image_layer_to_group(
                parent,
                name,
                image_id,
                transform,
                material.id,
            ),
        };
        let Some(layer_id) = layer_id else {
            return Ok(None);
        };
        self.embedded_image_id_high_water = image_id.0;
        self.embedded_images.insert(image_id, asset);
        self.layer_tree = tree;
        Ok(Some(AddedEmbeddedImageLayer { layer_id, image_id }))
    }

    pub fn embedded_image(&self, id: EmbeddedImageId) -> Option<&Arc<EmbeddedImageAsset>> {
        self.embedded_images.get(&id)
    }

    pub fn embedded_images(&self) -> impl Iterator<Item = &Arc<EmbeddedImageAsset>> {
        self.embedded_images.values()
    }

    pub fn prune_unreferenced_embedded_images(&mut self) -> Vec<Arc<EmbeddedImageAsset>> {
        let referenced = self
            .layer_tree
            .rows()
            .into_iter()
            .filter_map(|row| {
                self.layer_tree
                    .embedded_image(row.layer_id)
                    .map(|value| value.0)
            })
            .collect::<HashSet<_>>();
        let removed_ids = self
            .embedded_images
            .keys()
            .filter(|id| !referenced.contains(id))
            .copied()
            .collect::<Vec<_>>();
        removed_ids
            .into_iter()
            .filter_map(|id| self.embedded_images.remove(&id))
            .collect()
    }

    pub fn restore_embedded_images(
        &mut self,
        images: impl IntoIterator<Item = Arc<EmbeddedImageAsset>>,
    ) {
        for image in images {
            self.embedded_image_id_high_water = self.embedded_image_id_high_water.max(image.id.0);
            self.embedded_images.insert(image.id, image);
        }
    }

    pub fn embedded_images_referenced_by(&self, tree: &LayerTree) -> Vec<Arc<EmbeddedImageAsset>> {
        let ids = tree
            .rows()
            .into_iter()
            .filter_map(|row| tree.embedded_image(row.layer_id).map(|value| value.0))
            .collect::<HashSet<_>>();
        ids.into_iter()
            .filter_map(|id| self.embedded_images.get(&id).cloned())
            .collect()
    }

    pub fn restore_layer_tree_with_embedded_images(
        &mut self,
        target_tree: LayerTree,
        images: &[LayerImageSnapshot],
        embedded_images: &[Arc<EmbeddedImageAsset>],
    ) -> Result<(LayerTreeSurfaceChange, Vec<Arc<EmbeddedImageAsset>>)> {
        self.restore_embedded_images(embedded_images.iter().cloned());
        for row in target_tree.rows() {
            if let Some((image_id, _)) = target_tree.embedded_image(row.layer_id)
                && !self.embedded_images.contains_key(&image_id)
            {
                bail!("layer tree restore is missing an embedded image asset");
            }
        }
        let change = self.restore_layer_tree(target_tree, images)?;
        let removed = self.prune_unreferenced_embedded_images();
        Ok((change, removed))
    }

    pub fn embedded_image_id_high_water(&self) -> u64 {
        self.embedded_image_id_high_water
    }

    pub fn next_embedded_image_id(&self) -> EmbeddedImageId {
        EmbeddedImageId(
            self.embedded_image_id_high_water
                .checked_add(1)
                .expect("embedded image ID space exhausted"),
        )
    }

    pub fn restore_layer_tree(
        &mut self,
        mut target_tree: LayerTree,
        images: &[LayerImageSnapshot],
    ) -> Result<LayerTreeSurfaceChange> {
        target_tree.preserve_persistent_id_high_water(self.layer_tree.persistent_id_high_water());
        let current_surfaces = self.layer_surfaces_for_tree(&self.layer_tree);
        let target_surfaces = self.layer_surfaces_for_tree(&target_tree);
        let deleted_surfaces = current_surfaces
            .difference(&target_surfaces)
            .copied()
            .collect::<Vec<_>>();
        let created_surface_ids = target_surfaces
            .difference(&current_surfaces)
            .copied()
            .collect::<HashSet<_>>();
        let mut image_by_surface = HashMap::with_capacity(images.len());
        for image in images {
            if image_by_surface.insert(image.surface, image).is_some() {
                bail!(
                    "layer tree restore has duplicate image snapshot: {:?}",
                    image.surface
                );
            }
        }
        if created_surface_ids
            .iter()
            .any(|surface| !image_by_surface.contains_key(surface))
        {
            bail!("layer tree restore is missing an image for a created surface");
        }
        if image_by_surface
            .keys()
            .any(|surface| !target_surfaces.contains(surface))
        {
            bail!("layer tree restore image does not belong to the target tree");
        }

        let mut created_specs = Vec::with_capacity(created_surface_ids.len());
        for &surface in &created_surface_ids {
            let image = image_by_surface
                .get(&surface)
                .expect("validated created surface must have an image");
            let expected_size = self
                .materials
                .get(surface.material_index().as_usize())
                .map(|material| material.texture_size)
                .ok_or_else(|| anyhow::anyhow!("layer tree restore material is missing"))?;
            if image.texture_size != expected_size {
                bail!("layer tree restore image size does not match its material");
            }
            created_specs.push((
                surface,
                image.texture_size,
                InitialPixels::Rgba8(image.rgba8.clone()),
            ));
        }
        let mut writes = Vec::with_capacity(images.len().saturating_sub(created_specs.len()));
        for image in images {
            if created_surface_ids.contains(&image.surface) {
                continue;
            }
            let expected_size = self
                .materials
                .get(image.surface.material_index().as_usize())
                .map(|material| material.texture_size)
                .ok_or_else(|| anyhow::anyhow!("layer tree restore material is missing"))?;
            if image.texture_size != expected_size {
                bail!("layer tree restore image size does not match its material");
            }
            writes.push((
                image.surface,
                RectU32::full(image.texture_size),
                crate::core::tile_payload::PixelSnapshotData::contiguous(image.rgba8.clone()),
            ));
        }

        let AtomicSurfaceChanges {
            created: created_surfaces,
            updated: updated_surfaces,
        } = self.tiles.change_surfaces_and_write_atomically(
            &deleted_surfaces,
            created_specs,
            writes,
        )?;
        self.layer_tree = target_tree;
        Ok(LayerTreeSurfaceChange {
            deleted_surfaces,
            created_surfaces,
            updated_surfaces,
        })
    }

    pub fn replace_layers_with_raster(
        &mut self,
        plan: &LayerMergePlan,
    ) -> Result<Option<MergedRasterLayer>> {
        let deleted_surfaces = plan
            .layer_ids
            .iter()
            .flat_map(|layer_id| self.layer_surfaces_in_subtree(*layer_id))
            .collect::<Vec<_>>();
        let mut target_tree = self.layer_tree.clone();
        let Some(layer_id) = target_tree.insert_raster_layer(
            plan.parent_id,
            plan.insertion_index,
            plan.output_name.clone(),
        ) else {
            return Ok(None);
        };
        for source_layer_id in &plan.layer_ids {
            if target_tree.remove_layer(*source_layer_id) != RemoveLayerResult::Removed {
                bail!("layer merge tree replacement failed");
            }
        }
        let created_specs = self
            .materials
            .iter()
            .enumerate()
            .map(|(material_index, material)| {
                (
                    PaintSurfaceId::raster(material_index.into(), layer_id),
                    material.texture_size,
                    InitialPixels::Transparent,
                )
            })
            .collect::<Vec<_>>();
        let created_surfaces = created_specs
            .iter()
            .map(|(surface, _, _)| *surface)
            .collect::<Vec<_>>();

        self.tiles
            .change_surfaces_atomically(&deleted_surfaces, created_specs)?;
        self.layer_tree = target_tree;
        Ok(Some(MergedRasterLayer {
            layer_id,
            created_surfaces,
            deleted_surfaces,
        }))
    }

    pub fn remove_layer_with_surfaces(
        &mut self,
        layer_id: LayerId,
    ) -> Result<(RemoveLayerResult, Vec<PaintSurfaceId>)> {
        let targets = self.layer_surfaces_in_subtree(layer_id);
        let mut layer_tree = self.layer_tree.clone();
        let result = layer_tree.remove_layer(layer_id);
        if result != RemoveLayerResult::Removed {
            return Ok((result, Vec::new()));
        }
        self.tiles.delete_surfaces_atomically(&targets)?;
        self.layer_tree = layer_tree;
        Ok((result, targets))
    }

    pub fn remove_layers_with_surfaces(
        &mut self,
        layer_ids: &[LayerId],
    ) -> Result<Option<Vec<PaintSurfaceId>>> {
        let targets = layer_ids
            .iter()
            .flat_map(|layer_id| self.layer_surfaces_in_subtree(*layer_id))
            .collect::<Vec<_>>();
        let mut layer_tree = self.layer_tree.clone();
        for &layer_id in layer_ids {
            if layer_tree.remove_layer(layer_id) != RemoveLayerResult::Removed {
                return Ok(None);
            }
        }
        self.tiles.delete_surfaces_atomically(&targets)?;
        self.layer_tree = layer_tree;
        Ok(Some(targets))
    }

    fn layer_surfaces_for_tree(&self, tree: &LayerTree) -> HashSet<PaintSurfaceId> {
        tree.layer_surface_owners()
            .into_iter()
            .flat_map(|(layer_id, include_raster)| {
                (0..self.materials.len()).flat_map(move |material_index| {
                    let mut surfaces = Vec::with_capacity(2);
                    if include_raster {
                        surfaces.push(PaintSurfaceId::raster(material_index.into(), layer_id));
                    }
                    if tree.has_layer_mask(layer_id) {
                        surfaces.push(PaintSurfaceId::layer_mask(material_index.into(), layer_id));
                    }
                    surfaces
                })
            })
            .collect()
    }

    pub fn raycast_visible_with_scratch(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        visibility: &ViewportSceneVisibility,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        self.raycast_visible_with_preferred_triangle(ray_origin, ray_dir, None, visibility, scratch)
    }

    pub fn raycast_visible_with_preferred_triangle(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        preferred_triangle: Option<usize>,
        visibility: &ViewportSceneVisibility,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        self.mesh.raycast_visible_with_materials(
            ray_origin,
            ray_dir,
            preferred_triangle,
            visibility,
            &self.materials,
            scratch,
        )
    }

    pub fn new(mesh: MeshData, materials: Vec<MaterialSpec>) -> Self {
        let (materials, material_id_high_water) = materialize_materials(materials);
        let mut mesh_object_persistent_ids = HashMap::with_capacity(mesh.mesh_objects.len());
        let mut mesh_object_id_high_water = 0_u64;
        for mesh_object in mesh.mesh_objects.iter() {
            mesh_object_id_high_water = mesh_object_id_high_water
                .checked_add(1)
                .expect("mesh object id space exhausted");
            mesh_object_persistent_ids
                .insert(mesh_object.id, MeshObjectId(mesh_object_id_high_water));
        }
        let layer_tree = LayerTree::new_default_raster();
        let active_layer_id = layer_tree
            .default_raster_layer()
            .expect("default layer tree must contain a raster layer");
        let active_selection = ActiveSelection::disabled_for_materials(
            materials.iter().enumerate().map(|(index, _)| index.into()),
        );
        let mut tiles = DocumentTileStore::with_default_tile_size();
        for (material_index, material) in materials.iter().enumerate() {
            tiles
                .create_surface(
                    PaintSurfaceId::raster(material_index.into(), active_layer_id),
                    material.texture_size,
                    InitialPixels::Transparent,
                )
                .expect("default document tile surfaces must be valid");
        }
        Self {
            project_id: ProjectId::new(),
            mesh,
            mesh_object_persistent_ids,
            mesh_object_id_high_water,
            materials,
            material_id_high_water,
            embedded_images: HashMap::new(),
            embedded_image_id_high_water: 0,
            active_selection,
            selection_masks: SelectionMaskTileStore::default(),
            layer_tree,
            tiles,
        }
    }

    pub fn material_id(&self, material_index: MaterialIndex) -> Option<MaterialId> {
        self.materials
            .get(material_index.as_usize())
            .map(|material| material.id)
    }

    pub fn material_index(&self, material_id: MaterialId) -> Option<MaterialIndex> {
        self.materials
            .iter()
            .position(|material| material.id == material_id)
            .map(MaterialIndex)
    }

    pub fn material(&self, material_index: MaterialIndex) -> Option<&MaterialData> {
        self.materials.get(material_index.as_usize())
    }

    pub fn reload_mesh(
        &mut self,
        mesh: MeshData,
        new_materials: Vec<MaterialSpec>,
    ) -> Result<MeshReloadResult> {
        let existing_material_count = self.materials.len();
        let final_material_count = existing_material_count
            .checked_add(new_materials.len())
            .ok_or_else(|| anyhow::anyhow!("material count overflows usize"))?;
        if mesh
            .sub_meshes
            .iter()
            .any(|sub_mesh| sub_mesh.material_index >= final_material_count)
        {
            bail!("reloaded mesh references a material outside the mapped project materials");
        }

        let (appended_materials, next_material_id_high_water) =
            append_materials(&self.materials, new_materials, self.material_id_high_water)?;
        let (mesh_object_persistent_ids, next_mesh_object_id_high_water) =
            remap_mesh_object_persistent_ids(
                &self.mesh,
                &mesh,
                &self.mesh_object_persistent_ids,
                self.mesh_object_id_high_water,
            )?;

        let appended_material_indices = (existing_material_count..final_material_count)
            .map(MaterialIndex)
            .collect::<Vec<_>>();
        let raster_layers = self.layer_tree.ordered_raster_layers();
        let default_raster_layer = self.layer_tree.default_raster_layer();
        let masked_layers = self
            .layer_tree
            .rows()
            .into_iter()
            .map(|row| row.layer_id)
            .filter(|&layer_id| self.layer_tree.has_layer_mask(layer_id))
            .collect::<Vec<_>>();
        let mut surface_specs = Vec::new();
        let mut created_surfaces = Vec::new();
        for (offset, material) in appended_materials.iter().enumerate() {
            let material_index = MaterialIndex(existing_material_count + offset);
            for &layer_id in &raster_layers {
                let surface = PaintSurfaceId::raster(material_index, layer_id);
                created_surfaces.push(surface);
                surface_specs.push((
                    surface,
                    material.texture_size,
                    if Some(layer_id) == default_raster_layer {
                        InitialPixels::SolidRgba8([255; 4])
                    } else {
                        InitialPixels::Transparent
                    },
                ));
            }
            for &layer_id in &masked_layers {
                let surface = PaintSurfaceId::layer_mask(material_index, layer_id);
                created_surfaces.push(surface);
                surface_specs.push((
                    surface,
                    material.texture_size,
                    InitialPixels::SparseDefaultRgba8([255; 4]),
                ));
            }
        }

        self.tiles.create_surfaces_atomically(surface_specs)?;

        self.materials.extend(appended_materials);
        self.material_id_high_water = next_material_id_high_water;
        self.mesh = mesh;
        self.mesh_object_persistent_ids = mesh_object_persistent_ids;
        self.mesh_object_id_high_water = next_mesh_object_id_high_water;
        self.active_selection = ActiveSelection::disabled_for_materials(
            self.materials
                .iter()
                .enumerate()
                .map(|(index, _)| MaterialIndex(index)),
        );
        self.selection_masks = SelectionMaskTileStore::default();

        Ok(MeshReloadResult {
            appended_material_indices,
            created_surfaces,
        })
    }

    pub fn next_material_id(&self) -> MaterialId {
        MaterialId(
            self.material_id_high_water
                .checked_add(1)
                .expect("material id space exhausted"),
        )
    }

    pub fn mesh_object_persistent_id(&self, mesh_id: MeshId) -> Option<MeshObjectId> {
        self.mesh_object_persistent_ids.get(&mesh_id).copied()
    }

    pub fn mesh_object_id_high_water(&self) -> u64 {
        self.mesh_object_id_high_water
    }

    pub fn material_id_high_water(&self) -> u64 {
        self.material_id_high_water
    }

    pub fn composite_tree_for_material(&self, material_index: usize) -> CompositeTree {
        let material_id = self.material_id(MaterialIndex(material_index));
        CompositeTree {
            root: self.composite_group_for_layer(
                self.layer_tree.root(),
                material_index,
                material_id,
                true,
            ),
        }
    }

    pub fn composite_tree_for_layers(
        &self,
        material_index: usize,
        layer_ids: &[LayerId],
    ) -> CompositeTree {
        let material_id = self.material_id(MaterialIndex(material_index));
        CompositeTree {
            root: CompositeGroup {
                layer_id: None,
                mode: GroupCompositeMode::Isolated,
                props: CompositeProps {
                    visible: true,
                    opacity: 1.0,
                    blend_mode: LayerBlendMode::Normal,
                },
                mask: None,
                children: layer_ids
                    .iter()
                    .filter_map(|layer_id| {
                        self.composite_node_for_layer(*layer_id, material_index, material_id, true)
                    })
                    .collect(),
            },
        }
    }

    pub fn composite_tree_for_layer_bake(
        &self,
        material_index: usize,
        layer_id: LayerId,
        apply_mask: bool,
    ) -> Option<CompositeTree> {
        let node = self.layer_tree.get(layer_id)?;
        let material_id = self.material_id(MaterialIndex(material_index));
        let props = CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
        };
        let mask = apply_mask.then(|| PaintSurfaceId::layer_mask(material_index.into(), layer_id));
        let content = match node.content {
            LayerContent::Raster => CompositeNode::Raster {
                surface: PaintSurfaceId::raster(material_index.into(), layer_id),
                mask,
                props,
            },
            LayerContent::EmbeddedImage {
                image_id,
                transform,
            } => {
                let allowed = material_id.is_some_and(|id| node.material_mask.allows(id));
                return Some(CompositeTree {
                    root: neutral_bake_root(allowed.then_some(CompositeNode::EmbeddedImage {
                        layer_id,
                        image_id,
                        transform,
                        mask,
                        props,
                    })),
                });
            }
            LayerContent::SolidFill { color } => CompositeNode::SolidFill {
                layer_id,
                color,
                mask,
                props,
            },
            LayerContent::Adjustment { .. } | LayerContent::Group { .. } => return None,
        };
        Some(CompositeTree {
            root: neutral_bake_root(Some(content)),
        })
    }

    fn composite_group_for_layer(
        &self,
        layer_id: LayerId,
        material_index: usize,
        material_id: Option<MaterialId>,
        ancestors_allowed: bool,
    ) -> CompositeGroup {
        let Some(node) = self.layer_tree.get(layer_id) else {
            return CompositeGroup {
                layer_id: Some(layer_id),
                mode: GroupCompositeMode::Isolated,
                props: CompositeProps {
                    visible: false,
                    opacity: 0.0,
                    blend_mode: Default::default(),
                },
                mask: None,
                children: Vec::new(),
            };
        };
        let is_root = layer_id == self.layer_tree.root();
        let allowed = is_root
            || ancestors_allowed && material_id.is_some_and(|id| node.material_mask.allows(id));
        let children = if allowed {
            self.layer_tree
                .children(layer_id)
                .unwrap_or_default()
                .iter()
                .filter_map(|&child_id| {
                    self.composite_node_for_layer(child_id, material_index, material_id, allowed)
                })
                .collect()
        } else {
            Vec::new()
        };
        CompositeGroup {
            layer_id: Some(layer_id),
            mode: self
                .layer_tree
                .group_composite_mode(layer_id)
                .unwrap_or(GroupCompositeMode::Isolated),
            props: CompositeProps {
                visible: node.props.visible,
                opacity: node.effective_opacity(),
                blend_mode: node.props.blend_mode,
            },
            mask: node
                .mask
                .as_ref()
                .filter(|mask| mask.enabled)
                .map(|_| PaintSurfaceId::layer_mask(material_index.into(), layer_id)),
            children,
        }
    }

    fn composite_node_for_layer(
        &self,
        layer_id: LayerId,
        material_index: usize,
        material_id: Option<MaterialId>,
        ancestors_allowed: bool,
    ) -> Option<CompositeNode> {
        let node = self.layer_tree.get(layer_id)?;
        let material_id = material_id?;
        if !ancestors_allowed || !node.material_mask.allows(material_id) {
            return None;
        }
        match node.content.clone() {
            LayerContent::Raster => Some(CompositeNode::Raster {
                surface: PaintSurfaceId::raster(material_index.into(), layer_id),
                mask: node
                    .mask
                    .as_ref()
                    .filter(|mask| mask.enabled)
                    .map(|_| PaintSurfaceId::layer_mask(material_index.into(), layer_id)),
                props: CompositeProps {
                    visible: node.props.visible,
                    opacity: node.effective_opacity(),
                    blend_mode: node.props.blend_mode,
                },
            }),
            LayerContent::EmbeddedImage {
                image_id,
                transform,
            } => Some(CompositeNode::EmbeddedImage {
                layer_id,
                image_id,
                transform,
                mask: node
                    .mask
                    .as_ref()
                    .filter(|mask| mask.enabled)
                    .map(|_| PaintSurfaceId::layer_mask(material_index.into(), layer_id)),
                props: CompositeProps {
                    visible: node.props.visible,
                    opacity: node.effective_opacity(),
                    blend_mode: node.props.blend_mode,
                },
            }),
            LayerContent::SolidFill { color } => Some(CompositeNode::SolidFill {
                layer_id,
                color,
                mask: node
                    .mask
                    .as_ref()
                    .filter(|mask| mask.enabled)
                    .map(|_| PaintSurfaceId::layer_mask(material_index.into(), layer_id)),
                props: CompositeProps {
                    visible: node.props.visible,
                    opacity: node.effective_opacity(),
                    blend_mode: node.props.blend_mode,
                },
            }),
            LayerContent::Adjustment { adjustment } => Some(CompositeNode::Adjustment {
                layer_id,
                adjustment,
                mask: node
                    .mask
                    .as_ref()
                    .filter(|mask| mask.enabled)
                    .map(|_| PaintSurfaceId::layer_mask(material_index.into(), layer_id)),
                props: CompositeProps {
                    visible: node.props.visible,
                    opacity: node.effective_opacity(),
                    blend_mode: node.props.blend_mode,
                },
            }),
            LayerContent::Group { .. } => {
                Some(CompositeNode::Group(self.composite_group_for_layer(
                    layer_id,
                    material_index,
                    Some(material_id),
                    ancestors_allowed,
                )))
            }
        }
    }
}

fn neutral_bake_root(content: Option<CompositeNode>) -> CompositeGroup {
    CompositeGroup {
        layer_id: None,
        mode: GroupCompositeMode::Isolated,
        props: CompositeProps {
            visible: true,
            opacity: 1.0,
            blend_mode: LayerBlendMode::Normal,
        },
        mask: None,
        children: content.into_iter().collect(),
    }
}

fn remap_mesh_object_persistent_ids(
    old_mesh: &MeshData,
    new_mesh: &MeshData,
    old_persistent_ids: &HashMap<MeshId, MeshObjectId>,
    id_high_water: u64,
) -> Result<(HashMap<MeshId, MeshObjectId>, u64)> {
    let mut assignments = HashMap::with_capacity(new_mesh.mesh_objects.len());
    let mut used_old_ids = HashSet::with_capacity(old_mesh.mesh_objects.len());

    assign_unique_mesh_object_names(
        old_mesh,
        new_mesh,
        old_persistent_ids,
        &mut assignments,
        &mut used_old_ids,
        |name| name.to_owned(),
    );
    assign_unique_mesh_object_names(
        old_mesh,
        new_mesh,
        old_persistent_ids,
        &mut assignments,
        &mut used_old_ids,
        normalize_mesh_object_name,
    );

    let mut next_id = old_persistent_ids
        .values()
        .map(|id| id.0)
        .max()
        .unwrap_or(0)
        .max(id_high_water);
    for mesh_object in new_mesh.mesh_objects.iter() {
        if assignments.contains_key(&mesh_object.id) {
            continue;
        }
        next_id = next_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("mesh object id space exhausted"))?;
        assignments.insert(mesh_object.id, MeshObjectId(next_id));
    }
    Ok((assignments, next_id))
}

fn assign_unique_mesh_object_names(
    old_mesh: &MeshData,
    new_mesh: &MeshData,
    old_persistent_ids: &HashMap<MeshId, MeshObjectId>,
    assignments: &mut HashMap<MeshId, MeshObjectId>,
    used_old_ids: &mut HashSet<MeshId>,
    key: impl Fn(&str) -> String,
) {
    let mut old_by_name = HashMap::<String, Vec<MeshId>>::new();
    for mesh_object in old_mesh.mesh_objects.iter() {
        if used_old_ids.contains(&mesh_object.id) {
            continue;
        }
        old_by_name
            .entry(key(&mesh_object.name))
            .or_default()
            .push(mesh_object.id);
    }
    let mut new_by_name = HashMap::<String, Vec<MeshId>>::new();
    for mesh_object in new_mesh.mesh_objects.iter() {
        if assignments.contains_key(&mesh_object.id) {
            continue;
        }
        new_by_name
            .entry(key(&mesh_object.name))
            .or_default()
            .push(mesh_object.id);
    }
    for (name, new_ids) in new_by_name {
        let Some(old_ids) = old_by_name.get(&name) else {
            continue;
        };
        if new_ids.len() != 1 || old_ids.len() != 1 {
            continue;
        }
        let old_id = old_ids[0];
        let Some(&persistent_id) = old_persistent_ids.get(&old_id) else {
            continue;
        };
        assignments.insert(new_ids[0], persistent_id);
        used_old_ids.insert(old_id);
    }
}

fn normalize_mesh_object_name(name: &str) -> String {
    let trimmed = name.trim().to_ascii_lowercase();
    let mut normalized = trimmed
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((prefix, suffix)) = normalized.rsplit_once(' ')
        && !prefix.is_empty()
        && suffix.chars().all(|ch| ch.is_ascii_digit())
    {
        normalized = prefix.to_owned();
    }
    normalized
}

impl MeshData {
    pub fn empty() -> Self {
        Self {
            positions: Arc::new(Vec::new()),
            uvs: Arc::new(Vec::new()),
            normals: Arc::new(Vec::new()),
            indices: Arc::new(Vec::new()),
            sub_meshes: Arc::new(Vec::new()),
            mesh_objects: Arc::new(Vec::new()),
            triangle_mesh_ids: Arc::new(Vec::new()),
            surface_area_world: 1.0,
            uv_area: 1.0,
            uv_linear_scale: 1.0,
            scene_bounds_min: Vec3::ZERO,
            scene_bounds_max: Vec3::ZERO,
            scene_diagonal: 0.0,
            raycast_bvh: OnceLock::new(),
            triangle_material_indices: OnceLock::new(),
            triangle_geometry_neighbors: OnceLock::new(),
            uv_paint_boundary_edges: OnceLock::new(),
            uv_paint_edge_neighbors: OnceLock::new(),
            surface_filter_topology: OnceLock::new(),
        }
    }

    pub fn new(
        positions: Vec<Vec3>,
        uvs: Vec<Vec2>,
        normals: Vec<Vec3>,
        indices: Vec<[u32; 3]>,
        sub_meshes: Vec<SubMesh>,
        mesh_objects: Vec<MeshObject>,
        triangle_mesh_ids: Vec<MeshId>,
    ) -> Result<Self> {
        if positions.is_empty() || indices.is_empty() {
            bail!("mesh must contain positions and indices");
        }
        if uvs.len() != positions.len() {
            bail!("mesh UV count must match position count");
        }
        if normals.len() != positions.len() {
            bail!("mesh normal count must match position count");
        }
        if triangle_mesh_ids.len() != indices.len() {
            bail!("triangle mesh identity count must match triangle count");
        }

        let mut mesh_ids = HashSet::with_capacity(mesh_objects.len());
        for mesh_object in &mesh_objects {
            if !mesh_ids.insert(mesh_object.id) {
                bail!("mesh object ids must be unique");
            }
        }
        for mesh_id in &triangle_mesh_ids {
            if !mesh_ids.contains(mesh_id) {
                bail!("triangle references an unknown mesh object");
            }
        }
        for sub_mesh in &sub_meshes {
            if !mesh_ids.contains(&sub_mesh.mesh_id) {
                bail!("sub-mesh references an unknown mesh object");
            }
            if sub_mesh.start_index % 3 != 0 || sub_mesh.index_count % 3 != 0 {
                bail!("sub-mesh index range must align to triangle boundaries");
            }
            let end_index = sub_mesh
                .start_index
                .checked_add(sub_mesh.index_count)
                .context("sub-mesh index range overflow")?;
            let total_index_count = indices.len() as u64 * 3;
            if u64::from(end_index) > total_index_count {
                bail!("sub-mesh index range is out of bounds");
            }
            let triangle_start = sub_mesh.start_index as usize / 3;
            let triangle_end = end_index as usize / 3;
            if triangle_mesh_ids[triangle_start..triangle_end]
                .iter()
                .any(|mesh_id| *mesh_id != sub_mesh.mesh_id)
            {
                bail!("sub-mesh range contains triangles from another mesh object");
            }
        }

        let mut bounds_min = Vec3::splat(f32::INFINITY);
        let mut bounds_max = Vec3::splat(f32::NEG_INFINITY);
        for &position in &positions {
            bounds_min = bounds_min.min(position);
            bounds_max = bounds_max.max(position);
        }
        let scene_diagonal = (bounds_max - bounds_min).length();
        let scene_diagonal = if scene_diagonal.is_finite() {
            scene_diagonal
        } else {
            0.0
        };

        let mut surface_area_world = 0.0;
        let mut uv_area = 0.0;
        for tri in &indices {
            let i0 = tri[0] as usize;
            let i1 = tri[1] as usize;
            let i2 = tri[2] as usize;
            if i0 >= positions.len() || i1 >= positions.len() || i2 >= positions.len() {
                bail!("triangle index out of range");
            }

            let p0 = positions[i0];
            let p1 = positions[i1];
            let p2 = positions[i2];
            surface_area_world += 0.5 * (p1 - p0).cross(p2 - p0).length();

            let uv0 = uvs[i0];
            let uv1 = uvs[i1];
            let uv2 = uvs[i2];
            let uv_cross = (uv1 - uv0).perp_dot(uv2 - uv0).abs();
            uv_area += 0.5 * uv_cross;
        }

        let uv_linear_scale = if surface_area_world > 0.0 && uv_area > 0.0 {
            (uv_area / surface_area_world).sqrt()
        } else {
            1.0
        };

        Ok(Self {
            positions: Arc::new(positions),
            uvs: Arc::new(uvs),
            normals: Arc::new(normals),
            indices: Arc::new(indices),
            sub_meshes: Arc::new(sub_meshes),
            mesh_objects: Arc::new(mesh_objects),
            triangle_mesh_ids: Arc::new(triangle_mesh_ids),
            surface_area_world,
            uv_area,
            uv_linear_scale,
            scene_bounds_min: bounds_min,
            scene_bounds_max: bounds_max,
            scene_diagonal,
            raycast_bvh: OnceLock::new(),
            triangle_material_indices: OnceLock::new(),
            triangle_geometry_neighbors: OnceLock::new(),
            uv_paint_boundary_edges: OnceLock::new(),
            uv_paint_edge_neighbors: OnceLock::new(),
            surface_filter_topology: OnceLock::new(),
        })
    }

    pub fn world_to_uv_radius(&self, radius_world: f32) -> f32 {
        radius_world.max(0.0) * self.uv_linear_scale
    }

    pub(crate) fn surface_filter_topology(&self) -> &SurfaceFilterTopology {
        self.surface_filter_topology
            .get_or_init(|| self.build_surface_filter_topology())
    }

    pub(crate) fn surface_filter_reference_texel_density(
        &self,
        material_sizes: &[[u32; 2]],
        enabled_materials: &[bool],
    ) -> f32 {
        let mut world_area = 0.0_f64;
        let mut pixel_area = 0.0_f64;
        for triangle in &self.surface_filter_topology().triangles {
            if !enabled_materials
                .get(triangle.material_index)
                .copied()
                .unwrap_or(false)
            {
                continue;
            }
            let Some(texture_size) = material_sizes.get(triangle.material_index).copied() else {
                continue;
            };
            let [p0, p1, p2] = triangle.positions;
            let triangle_world_area = 0.5 * (p1 - p0).cross(p2 - p0).length();
            let [uv0, uv1, uv2] = triangle.uvs;
            let triangle_uv_area = 0.5 * (uv1 - uv0).perp_dot(uv2 - uv0).abs();
            if !(triangle_world_area.is_finite()
                && triangle_world_area > f32::EPSILON
                && triangle_uv_area.is_finite()
                && triangle_uv_area > f32::EPSILON)
            {
                continue;
            }
            world_area += f64::from(triangle_world_area);
            pixel_area += f64::from(triangle_uv_area)
                * f64::from(texture_size[0].max(1))
                * f64::from(texture_size[1].max(1));
        }
        if world_area <= f64::EPSILON || pixel_area <= f64::EPSILON {
            return 1.0;
        }
        let density = (pixel_area / world_area).sqrt() as f32;
        if density.is_finite() && density > f32::EPSILON {
            density
        } else {
            1.0
        }
    }

    pub fn scene_bounds(&self) -> Option<(Vec3, Vec3)> {
        if !self.scene_bounds_min.is_finite()
            || !self.scene_bounds_max.is_finite()
            || !self.scene_bounds_min.cmple(self.scene_bounds_max).all()
        {
            return None;
        }
        Some((self.scene_bounds_min, self.scene_bounds_max))
    }

    pub fn scene_diagonal(&self) -> f32 {
        self.scene_diagonal
    }

    pub fn triangle_geometric_normal(&self, triangle_index: usize) -> Option<Vec3> {
        let triangle = self.indices.get(triangle_index)?;
        let p0 = *self.positions.get(triangle[0] as usize)?;
        let p1 = *self.positions.get(triangle[1] as usize)?;
        let p2 = *self.positions.get(triangle[2] as usize)?;
        let normal = (p1 - p0).cross(p2 - p0).normalize_or_zero();
        (normal.length_squared() > f32::EPSILON && normal.is_finite()).then_some(normal)
    }

    pub fn local_smooth_triangle_geometric_normal(
        &self,
        triangle_index: usize,
        world_pos: Vec3,
        smooth_angle_degrees: f32,
    ) -> Option<Vec3> {
        let face_normal = self.triangle_geometric_normal(triangle_index)?;
        if !world_pos.is_finite() || !smooth_angle_degrees.is_finite() {
            return None;
        }
        let minimum_dot = smooth_angle_degrees.clamp(0.0, 180.0).to_radians().cos();
        let triangle = *self.indices.get(triangle_index)?;
        let positions = triangle.map(|index| self.positions.get(index as usize).copied());
        let [Some(p0), Some(p1), Some(p2)] = positions else {
            return None;
        };
        let points = [p0, p1, p2];
        let neighbors = self
            .triangle_geometry_neighbors
            .get_or_init(|| self.build_triangle_geometry_neighbors());

        let (vertex_index, vertex_distance) = points
            .iter()
            .enumerate()
            .map(|(index, point)| (index, world_pos.distance(*point)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let vertex_width = points[vertex_index]
            .distance(points[(vertex_index + 1) % 3])
            .min(points[vertex_index].distance(points[(vertex_index + 2) % 3]))
            * 0.2;
        if vertex_width > f32::EPSILON && vertex_distance < vertex_width {
            let feature_normal = self.vertex_fan_normal(
                triangle_index,
                position_key(points[vertex_index]),
                minimum_dot,
                neighbors,
            )?;
            let weight = 1.0 - smoothstep01(vertex_distance / vertex_width);
            return blend_face_and_feature_normal(face_normal, feature_normal, weight);
        }

        let (local_edge, edge_distance) = TRIANGLE_EDGE_VERTEX_INDICES
            .into_iter()
            .enumerate()
            .map(|(local_edge, [a, b])| {
                (
                    local_edge,
                    distance_to_segment_3d(world_pos, points[a], points[b]),
                )
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        let edge_width = triangle_min_altitude(points) * 0.25;
        if edge_width <= f32::EPSILON || edge_distance >= edge_width {
            return Some(face_normal);
        }
        let edge_neighbors = neighbors.get(triangle_index)?.get(local_edge)?;
        let [neighbor_index] = edge_neighbors.as_slice() else {
            return Some(face_normal);
        };
        let neighbor_normal = self.triangle_geometric_normal(*neighbor_index)?;
        if face_normal.dot(neighbor_normal) + 1.0e-6 < minimum_dot {
            return Some(face_normal);
        }
        let normal_sum = face_normal + neighbor_normal;
        if normal_sum.length_squared() <= f32::EPSILON || !normal_sum.is_finite() {
            return Some(face_normal);
        }
        let feature_normal = normal_sum.normalize();
        let weight = 1.0 - smoothstep01(edge_distance / edge_width);
        blend_face_and_feature_normal(face_normal, feature_normal, weight)
    }

    fn vertex_fan_normal(
        &self,
        triangle_index: usize,
        vertex_key: PositionKey,
        minimum_dot: f32,
        neighbors: &[[Vec<usize>; 3]],
    ) -> Option<Vec3> {
        let mut visited = vec![false; self.indices.len()];
        let mut stack = vec![triangle_index];
        visited[triangle_index] = true;
        let mut normal_sum = Vec3::ZERO;
        while let Some(current) = stack.pop() {
            let normal = self.triangle_geometric_normal(current)?;
            let corner = self.triangle_corner_for_position_key(current, vertex_key)?;
            normal_sum += normal * self.triangle_corner_angle(current, corner)?;
            for &neighbor in neighbors
                .get(current)?
                .iter()
                .enumerate()
                .filter(|(local_edge, _)| {
                    TRIANGLE_EDGE_VERTEX_INDICES[*local_edge].contains(&corner)
                })
                .flat_map(|(_, edge_neighbors)| edge_neighbors)
            {
                if visited[neighbor]
                    || self
                        .triangle_corner_for_position_key(neighbor, vertex_key)
                        .is_none()
                {
                    continue;
                }
                let neighbor_normal = self.triangle_geometric_normal(neighbor)?;
                if normal.dot(neighbor_normal) + 1.0e-6 >= minimum_dot {
                    visited[neighbor] = true;
                    stack.push(neighbor);
                }
            }
        }
        if normal_sum.length_squared() <= f32::EPSILON || !normal_sum.is_finite() {
            return self.triangle_geometric_normal(triangle_index);
        }
        Some(normal_sum.normalize())
    }

    fn triangle_corner_for_position_key(
        &self,
        triangle_index: usize,
        vertex_key: PositionKey,
    ) -> Option<usize> {
        self.indices.get(triangle_index)?.iter().position(|index| {
            self.positions
                .get(*index as usize)
                .is_some_and(|position| position_key(*position) == vertex_key)
        })
    }

    fn triangle_corner_angle(&self, triangle_index: usize, corner: usize) -> Option<f32> {
        let triangle = self.indices.get(triangle_index)?;
        let center = *self.positions.get(triangle[corner] as usize)?;
        let a = (*self.positions.get(triangle[(corner + 1) % 3] as usize)? - center)
            .normalize_or_zero();
        let b = (*self.positions.get(triangle[(corner + 2) % 3] as usize)? - center)
            .normalize_or_zero();
        (a.length_squared() > f32::EPSILON && b.length_squared() > f32::EPSILON)
            .then(|| a.dot(b).clamp(-1.0, 1.0).acos())
    }

    fn build_surface_filter_topology(&self) -> SurfaceFilterTopology {
        let triangles = self
            .indices
            .iter()
            .enumerate()
            .filter_map(|(triangle_index, triangle)| {
                let positions = [
                    *self.positions.get(triangle[0] as usize)?,
                    *self.positions.get(triangle[1] as usize)?,
                    *self.positions.get(triangle[2] as usize)?,
                ];
                let uvs = [
                    *self.uvs.get(triangle[0] as usize)?,
                    *self.uvs.get(triangle[1] as usize)?,
                    *self.uvs.get(triangle[2] as usize)?,
                ];
                let world_normal = (positions[1] - positions[0]).cross(positions[2] - positions[0]);
                let uv_determinant = (uvs[1] - uvs[0]).perp_dot(uvs[2] - uvs[0]);
                if positions.iter().any(|position| !position.is_finite())
                    || uvs.iter().any(|uv| !uv.is_finite())
                    || !world_normal.is_finite()
                    || world_normal.length_squared() <= 0.0
                    || !uv_determinant.is_finite()
                    || uv_determinant == 0.0
                {
                    return None;
                }
                Some(SurfaceFilterTriangle {
                    positions,
                    uvs,
                    material_index: self.material_index_for_triangle(triangle_index),
                    mesh_id: *self.triangle_mesh_ids.get(triangle_index)?,
                    edges: [SurfaceFilterEdge::BOUNDARY; 3],
                })
            })
            .collect::<Vec<_>>();

        let mut topology = SurfaceFilterTopology { triangles };
        let mut edge_occurrences: HashMap<
            (MeshId, GeometryEdgeKey),
            Vec<SurfaceFilterEdgeOccurrence>,
        > = HashMap::new();
        for (triangle_index, triangle) in topology.triangles.iter().enumerate() {
            for (local_edge, [a, b]) in TRIANGLE_EDGE_VERTEX_INDICES.into_iter().enumerate() {
                let position_keys = [
                    position_key(triangle.positions[a]),
                    position_key(triangle.positions[b]),
                ];
                edge_occurrences
                    .entry((
                        triangle.mesh_id,
                        geometry_edge_key(position_keys[0], position_keys[1]),
                    ))
                    .or_default()
                    .push(SurfaceFilterEdgeOccurrence {
                        triangle_index,
                        local_edge,
                        position_keys,
                    });
            }
        }

        for occurrences in edge_occurrences.values() {
            let [a, b] = occurrences.as_slice() else {
                continue;
            };
            let reversed = a.position_keys[0] == b.position_keys[1]
                && a.position_keys[1] == b.position_keys[0];
            let same_orientation = a.position_keys == b.position_keys;
            if !reversed && !same_orientation {
                continue;
            }
            topology.triangles[a.triangle_index].edges[a.local_edge] = SurfaceFilterEdge {
                neighbor_triangle: Some(b.triangle_index),
                neighbor_edge: b.local_edge,
                reversed,
            };
            topology.triangles[b.triangle_index].edges[b.local_edge] = SurfaceFilterEdge {
                neighbor_triangle: Some(a.triangle_index),
                neighbor_edge: a.local_edge,
                reversed,
            };
        }

        topology
    }

    fn build_triangle_geometry_neighbors(&self) -> Vec<[Vec<usize>; 3]> {
        let mut edge_triangles: HashMap<GeometryEdgeKey, Vec<(usize, usize)>> = HashMap::new();
        for (triangle_index, triangle) in self.indices.iter().enumerate() {
            for (local_edge, [a, b]) in TRIANGLE_EDGE_VERTEX_INDICES.into_iter().enumerate() {
                let Some(&position_a) = self.positions.get(triangle[a] as usize) else {
                    continue;
                };
                let Some(&position_b) = self.positions.get(triangle[b] as usize) else {
                    continue;
                };
                edge_triangles
                    .entry(geometry_edge_key(
                        position_key(position_a),
                        position_key(position_b),
                    ))
                    .or_default()
                    .push((triangle_index, local_edge));
            }
        }

        let mut neighbors = (0..self.indices.len())
            .map(|_| std::array::from_fn(|_| Vec::new()))
            .collect::<Vec<_>>();
        for occurrences in edge_triangles.values() {
            let [(a, edge_a), (b, edge_b)] = occurrences.as_slice() else {
                continue;
            };
            if self.triangle_mesh_ids.get(*a) == self.triangle_mesh_ids.get(*b) {
                neighbors[*a][*edge_a].push(*b);
                neighbors[*b][*edge_b].push(*a);
            }
        }
        neighbors
    }

    pub fn world_to_uv_texel_radius(&self, radius_world: f32, tex_size: [u32; 2]) -> f32 {
        let tex_scale = ((tex_size[0].max(1) as f32) * (tex_size[1].max(1) as f32)).sqrt();
        self.world_to_uv_radius(radius_world) * tex_scale
    }

    pub fn world_to_uv_view_radius_px(&self, radius_world: f32, view_size: [u32; 2]) -> f32 {
        let view_scale = ((view_size[0].max(1) as f32) * (view_size[1].max(1) as f32)).sqrt();
        self.world_to_uv_radius(radius_world) * view_scale
    }

    pub(crate) fn triangle_candidates_intersecting_aabb(
        &self,
        bounds_min: Vec3,
        bounds_max: Vec3,
    ) -> Vec<usize> {
        if !bounds_min.is_finite() || !bounds_max.is_finite() || !bounds_min.cmple(bounds_max).all()
        {
            return Vec::new();
        }

        let query = Aabb {
            min: bounds_min,
            max: bounds_max,
        };
        let bvh = self.raycast_bvh();
        let mut stack = Vec::new();
        let mut triangles = Vec::new();
        if let Some(root) = bvh.root {
            stack.push(root);
        }
        while let Some(node_index) = stack.pop() {
            let node = &bvh.nodes[node_index];
            if !node.bounds.intersects(query) {
                continue;
            }
            if node.left.is_none() && node.right.is_none() {
                triangles.extend(node.triangles.iter().copied());
                continue;
            }
            if let Some(left) = node.left {
                stack.push(left);
            }
            if let Some(right) = node.right {
                stack.push(right);
            }
        }
        triangles.sort_unstable();
        triangles.dedup();
        triangles
    }

    pub fn raycast(&self, ray_origin: Vec3, ray_dir: Vec3) -> Option<SurfaceHit> {
        let mut scratch = RaycastScratch::default();
        self.raycast_with_scratch(ray_origin, ray_dir, &mut scratch)
    }

    pub fn raycast_visible(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        visibility: &ViewportSceneVisibility,
    ) -> Option<SurfaceHit> {
        let mut scratch = RaycastScratch::default();
        self.raycast_visible_with_scratch(ray_origin, ray_dir, visibility, &mut scratch)
    }

    pub fn raycast_with_scratch(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        self.raycast_with_preferred_triangle(ray_origin, ray_dir, None, scratch)
    }

    pub fn raycast_visible_with_scratch(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        visibility: &ViewportSceneVisibility,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        self.raycast_visible_with_preferred_triangle(ray_origin, ray_dir, None, visibility, scratch)
    }

    pub fn raycast_with_preferred_triangle(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        preferred_triangle: Option<usize>,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        self.raycast_with_optional_visibility(
            ray_origin,
            ray_dir,
            preferred_triangle,
            None,
            None,
            scratch,
        )
    }

    pub fn raycast_visible_with_preferred_triangle(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        preferred_triangle: Option<usize>,
        visibility: &ViewportSceneVisibility,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        self.raycast_with_optional_visibility(
            ray_origin,
            ray_dir,
            preferred_triangle,
            Some(visibility),
            None,
            scratch,
        )
    }

    fn raycast_visible_with_materials(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        preferred_triangle: Option<usize>,
        visibility: &ViewportSceneVisibility,
        materials: &[MaterialData],
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        self.raycast_with_optional_visibility(
            ray_origin,
            ray_dir,
            preferred_triangle,
            Some(visibility),
            Some(materials),
            scratch,
        )
    }

    fn raycast_with_optional_visibility(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        preferred_triangle: Option<usize>,
        visibility: Option<&ViewportSceneVisibility>,
        materials: Option<&[MaterialData]>,
        scratch: &mut RaycastScratch,
    ) -> Option<SurfaceHit> {
        let ray_dir = ray_dir.normalize_or_zero();
        if ray_dir.length_squared() <= f32::EPSILON {
            return None;
        }

        let preferred_triangle = preferred_triangle.filter(|&triangle_index| {
            visibility.is_none_or(|visibility| {
                self.triangle_visible_in_viewport(triangle_index, visibility)
            }) && self.triangle_front_facing_or_double_sided(triangle_index, ray_dir, materials)
        });
        let mut nearest = preferred_triangle
            .and_then(|triangle_index| self.raycast_triangle(ray_origin, ray_dir, triangle_index));
        scratch.stack.clear();
        let bvh = self.raycast_bvh();
        if let Some(root) = bvh.root {
            scratch.stack.push(root);
        }
        while let Some(node_index) = scratch.stack.pop() {
            let node = &bvh.nodes[node_index];
            let nearest_t = nearest.map_or(f32::INFINITY, |hit| hit.t);
            if node
                .bounds
                .ray_intersection(ray_origin, ray_dir, nearest_t)
                .is_none()
            {
                continue;
            }

            if node.left.is_none() && node.right.is_none() {
                for &triangle_index in &node.triangles {
                    if preferred_triangle == Some(triangle_index) {
                        continue;
                    }
                    if visibility.is_some_and(|visibility| {
                        !self.triangle_visible_in_viewport(triangle_index, visibility)
                    }) {
                        continue;
                    }
                    if !self.triangle_front_facing_or_double_sided(
                        triangle_index,
                        ray_dir,
                        materials,
                    ) {
                        continue;
                    }
                    if let Some(hit) = self.raycast_triangle(ray_origin, ray_dir, triangle_index)
                        && nearest.is_none_or(|nearest_hit| hit.t < nearest_hit.t)
                    {
                        nearest = Some(hit);
                    }
                }
                continue;
            }

            let child_hit = |child: Option<usize>, nearest_t: f32| {
                child.and_then(|child| {
                    let child_node = &bvh.nodes[child];
                    child_node
                        .bounds
                        .ray_intersection(ray_origin, ray_dir, nearest_t)
                        .map(|entry_t| (entry_t, child))
                })
            };
            let nearest_t = nearest.map_or(f32::INFINITY, |hit| hit.t);
            match (
                child_hit(node.left, nearest_t),
                child_hit(node.right, nearest_t),
            ) {
                (Some(left), Some(right)) => {
                    let (near, far) = if left.0 <= right.0 {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    scratch.stack.push(far.1);
                    scratch.stack.push(near.1);
                }
                (Some((_, child)), None) | (None, Some((_, child))) => scratch.stack.push(child),
                (None, None) => {}
            }
        }
        nearest
    }

    fn triangle_front_facing_or_double_sided(
        &self,
        triangle_index: usize,
        ray_dir: Vec3,
        materials: Option<&[MaterialData]>,
    ) -> bool {
        let Some(materials) = materials else {
            return true;
        };
        let material_index = self.material_index_for_triangle(triangle_index);
        if materials
            .get(material_index)
            .is_none_or(|material| material.render_settings.double_sided)
        {
            return true;
        }
        let Some([i0, i1, i2]) = self.indices.get(triangle_index).copied() else {
            return false;
        };
        let Some((&p0, &p1, &p2)) = self
            .positions
            .get(i0 as usize)
            .zip(self.positions.get(i1 as usize))
            .zip(self.positions.get(i2 as usize))
            .map(|((p0, p1), p2)| (p0, p1, p2))
        else {
            return false;
        };
        (p1 - p0).cross(p2 - p0).dot(ray_dir) < -1.0e-6
    }

    fn triangle_visible_in_viewport(
        &self,
        triangle_index: usize,
        visibility: &ViewportSceneVisibility,
    ) -> bool {
        let Some(mesh_id) = self.triangle_mesh_ids.get(triangle_index).copied() else {
            return false;
        };
        visibility.geometry_visible(
            mesh_id,
            self.material_index_for_triangle(triangle_index).into(),
        )
    }

    fn raycast_bvh(&self) -> &RaycastBvh {
        self.raycast_bvh
            .get_or_init(|| RaycastBvh::build(&self.positions, &self.indices))
    }

    fn triangle_material_indices(&self) -> &[usize] {
        self.triangle_material_indices.get_or_init(|| {
            (0..self.indices.len())
                .map(|triangle_index| {
                    let first_index = triangle_index as u32 * 3;
                    self.sub_meshes
                        .iter()
                        .find(|sub_mesh| {
                            first_index >= sub_mesh.start_index
                                && first_index
                                    < sub_mesh.start_index.saturating_add(sub_mesh.index_count)
                        })
                        .map(|sub_mesh| sub_mesh.material_index)
                        .unwrap_or(0)
                })
                .collect()
        })
    }

    pub(crate) fn material_index_for_triangle(&self, triangle_index: usize) -> usize {
        self.triangle_material_indices()
            .get(triangle_index)
            .copied()
            .unwrap_or(0)
    }

    fn uv_paint_boundary_edges(&self) -> &[[UvPaintEdgeKind; 3]] {
        self.uv_paint_boundary_edges
            .get_or_init(|| self.build_uv_paint_boundary_edges())
    }

    fn uv_paint_edge_neighbors(&self) -> &[[Vec<UvPaintEdgeNeighbor>; 3]] {
        self.uv_paint_edge_neighbors
            .get_or_init(|| self.build_uv_paint_edge_neighbors())
    }

    pub(crate) fn uv_boundary_rects_for_target_material(
        &self,
        source_triangle_index: usize,
        source_uv: Vec2,
        target_material_index: usize,
        radius_uv: f32,
        texture_size: [u32; 2],
        guard_px: f32,
    ) -> Option<Vec<RectU32>> {
        if !(source_uv.x.is_finite()
            && source_uv.y.is_finite()
            && radius_uv.is_finite()
            && radius_uv >= 0.0
            && guard_px.is_finite()
            && guard_px >= 0.0)
        {
            return None;
        }
        let tri = self.indices.get(source_triangle_index)?;
        let indices = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let vertices = triangle_vertices(
            self.positions.as_ref().as_slice(),
            self.uvs.as_ref().as_slice(),
            indices,
        )?;
        let vertex_position_keys = [
            position_key(vertices[0].0),
            position_key(vertices[1].0),
            position_key(vertices[2].0),
        ];
        let edge_kinds = self.uv_paint_boundary_edges().get(source_triangle_index)?;
        let edge_neighbors = self.uv_paint_edge_neighbors().get(source_triangle_index)?;
        let texture_width = texture_size[0].max(1) as f32;
        let texture_height = texture_size[1].max(1) as f32;
        let guard_uv = (guard_px / texture_width).max(guard_px / texture_height);
        let boundary_radius_uv = radius_uv + guard_uv;
        let mut rects = Vec::new();

        for (local_edge, [a, b]) in TRIANGLE_EDGE_VERTEX_INDICES.into_iter().enumerate() {
            if !edge_kinds[local_edge].blocks_partial_surface_rect() {
                continue;
            }
            let source_a = vertices[a].1;
            let source_b = vertices[b].1;
            if point_to_segment_distance(source_uv, source_a, source_b) > boundary_radius_uv {
                continue;
            }
            let source_position_keys = [vertex_position_keys[a], vertex_position_keys[b]];
            let t = segment_param(source_uv, source_a, source_b);
            for neighbor in edge_neighbors[local_edge]
                .iter()
                .filter(|neighbor| neighbor.material_index == target_material_index)
            {
                let (neighbor_a, neighbor_b) =
                    oriented_neighbor_edge_uvs(neighbor, source_position_keys);
                let neighbor_uv = neighbor_a.lerp(neighbor_b, t);
                if let Some(rect) =
                    uv_center_radius_to_pixel_rect(texture_size, neighbor_uv, radius_uv, guard_px)
                    && !rects.contains(&rect)
                {
                    rects.push(rect);
                }
            }
        }

        Some(rects)
    }

    fn build_uv_paint_boundary_edges(&self) -> Vec<[UvPaintEdgeKind; 3]> {
        let mut edge_kinds = vec![[UvPaintEdgeKind::OpenBoundary; 3]; self.indices.len()];
        let mut edge_occurrences: HashMap<GeometryEdgeKey, Vec<EdgeOccurrence>> = HashMap::new();

        for (triangle_index, tri) in self.indices.iter().enumerate() {
            let indices = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let Some(vertices) = triangle_vertices(
                self.positions.as_ref().as_slice(),
                self.uvs.as_ref().as_slice(),
                indices,
            ) else {
                continue;
            };
            let material_index = self.material_index_for_triangle(triangle_index);
            for (local_edge, [a, b]) in TRIANGLE_EDGE_VERTEX_INDICES.into_iter().enumerate() {
                let position_keys = [position_key(vertices[a].0), position_key(vertices[b].0)];
                let key = geometry_edge_key(position_keys[0], position_keys[1]);
                edge_occurrences
                    .entry(key)
                    .or_default()
                    .push(EdgeOccurrence {
                        triangle_index,
                        local_edge,
                        material_index,
                        position_keys,
                        uvs: [vertices[a].1, vertices[b].1],
                    });
            }
        }

        for occurrences in edge_occurrences.values() {
            let kind = match occurrences.as_slice() {
                [_] => UvPaintEdgeKind::OpenBoundary,
                [a, b] => classify_uv_paint_edge_pair(a, b),
                _ => UvPaintEdgeKind::NonManifold,
            };
            for occurrence in occurrences {
                edge_kinds[occurrence.triangle_index][occurrence.local_edge] = kind;
            }
        }

        edge_kinds
    }

    fn build_uv_paint_edge_neighbors(&self) -> Vec<[Vec<UvPaintEdgeNeighbor>; 3]> {
        let mut neighbors: Vec<[Vec<UvPaintEdgeNeighbor>; 3]> = (0..self.indices.len())
            .map(|_| std::array::from_fn(|_| Vec::new()))
            .collect();
        let mut edge_occurrences: HashMap<GeometryEdgeKey, Vec<EdgeOccurrence>> = HashMap::new();

        for (triangle_index, tri) in self.indices.iter().enumerate() {
            let indices = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
            let Some(vertices) = triangle_vertices(
                self.positions.as_ref().as_slice(),
                self.uvs.as_ref().as_slice(),
                indices,
            ) else {
                continue;
            };
            let material_index = self.material_index_for_triangle(triangle_index);
            for (local_edge, [a, b]) in TRIANGLE_EDGE_VERTEX_INDICES.into_iter().enumerate() {
                let position_keys = [position_key(vertices[a].0), position_key(vertices[b].0)];
                let key = geometry_edge_key(position_keys[0], position_keys[1]);
                edge_occurrences
                    .entry(key)
                    .or_default()
                    .push(EdgeOccurrence {
                        triangle_index,
                        local_edge,
                        material_index,
                        position_keys,
                        uvs: [vertices[a].1, vertices[b].1],
                    });
            }
        }

        for occurrences in edge_occurrences.values() {
            for occurrence in occurrences {
                let Some(edge_neighbors) = neighbors
                    .get_mut(occurrence.triangle_index)
                    .map(|edges| &mut edges[occurrence.local_edge])
                else {
                    continue;
                };
                for neighbor in occurrences {
                    if neighbor.triangle_index == occurrence.triangle_index {
                        continue;
                    }
                    edge_neighbors.push(UvPaintEdgeNeighbor {
                        material_index: neighbor.material_index,
                        position_keys: neighbor.position_keys,
                        uvs: neighbor.uvs,
                    });
                }
            }
        }

        neighbors
    }

    fn raycast_triangle(
        &self,
        ray_origin: Vec3,
        ray_dir: Vec3,
        triangle_index: usize,
    ) -> Option<SurfaceHit> {
        let tri = self.indices.get(triangle_index)?;
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        let (Some(&p0), Some(&p1), Some(&p2)) = (
            self.positions.get(i0),
            self.positions.get(i1),
            self.positions.get(i2),
        ) else {
            return None;
        };

        let (t, u, v) = ray_intersects_triangle(ray_origin, ray_dir, p0, p1, p2)?;
        if t <= 0.0 {
            return None;
        }

        let w = 1.0 - u - v;
        let geometric_normal = (p1 - p0).cross(p2 - p0).normalize_or_zero();
        let world_normal = if let (Some(&n0), Some(&n1), Some(&n2)) = (
            self.normals.get(i0),
            self.normals.get(i1),
            self.normals.get(i2),
        ) {
            (n0 * w + n1 * u + n2 * v).normalize_or_zero()
        } else {
            geometric_normal
        };
        let (uv, uv_edge_distance, uv_paint_boundary_distance) =
            if let (Some(&uv0), Some(&uv1), Some(&uv2)) =
                (self.uvs.get(i0), self.uvs.get(i1), self.uvs.get(i2))
            {
                let uv = uv0 * w + uv1 * u + uv2 * v;
                let uv_edge_distance = distance_to_uv_triangle_edge(uv, uv0, uv1, uv2);
                let edge_kinds = self
                    .uv_paint_boundary_edges()
                    .get(triangle_index)
                    .copied()
                    .unwrap_or([UvPaintEdgeKind::NonManifold; 3]);
                let uv_paint_boundary_distance =
                    distance_to_uv_paint_boundary_edges(uv, [uv0, uv1, uv2], edge_kinds);
                (uv, uv_edge_distance, uv_paint_boundary_distance)
            } else {
                (Vec2::ZERO, 0.0, 0.0)
            };

        Some(SurfaceHit {
            world_pos: ray_origin + ray_dir * t,
            world_normal: if world_normal.length_squared() > f32::EPSILON {
                world_normal
            } else {
                geometric_normal
            },
            uv,
            uv_edge_distance,
            uv_paint_boundary_distance,
            triangle_index,
            material_index: self.material_index_for_triangle(triangle_index).into(),
            mesh_id: self.triangle_mesh_ids[triangle_index],
            t,
        })
    }

    pub fn pick_uv(&self, uv: Vec2, material_filter: Option<usize>) -> Option<UvHit> {
        self.indices
            .iter()
            .enumerate()
            .filter_map(|(triangle_index, tri)| {
                let material_index = self.material_index_for_triangle(triangle_index);
                if material_filter.is_some_and(|filter| filter != material_index) {
                    return None;
                }
                let uv0 = *self.uvs.get(tri[0] as usize)?;
                let uv1 = *self.uvs.get(tri[1] as usize)?;
                let uv2 = *self.uvs.get(tri[2] as usize)?;
                point_in_uv_triangle(uv, uv0, uv1, uv2).then_some(UvHit {
                    uv,
                    triangle_index,
                    material_index: material_index.into(),
                    mesh_id: self.triangle_mesh_ids[triangle_index],
                })
            })
            .next()
    }
}

impl RaycastBvh {
    const LEAF_TRIANGLE_COUNT: usize = 6;

    fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            root: None,
        }
    }

    fn build(positions: &[Vec3], indices: &[[u32; 3]]) -> Self {
        let mut triangles = indices
            .iter()
            .enumerate()
            .filter_map(|(triangle_index, tri)| {
                let p0 = *positions.get(tri[0] as usize)?;
                let p1 = *positions.get(tri[1] as usize)?;
                let p2 = *positions.get(tri[2] as usize)?;
                let bounds = Aabb::from_triangle(p0, p1, p2);
                Some(TriangleBuildData {
                    triangle_index,
                    bounds,
                    centroid: (p0 + p1 + p2) / 3.0,
                })
            })
            .collect::<Vec<_>>();
        if triangles.is_empty() {
            return Self::empty();
        }

        let mut nodes = Vec::new();
        let root = Self::build_node(&mut nodes, &mut triangles);
        Self {
            nodes,
            root: Some(root),
        }
    }

    fn build_node(nodes: &mut Vec<RaycastBvhNode>, triangles: &mut [TriangleBuildData]) -> usize {
        let bounds = triangles
            .iter()
            .fold(Aabb::empty(), |bounds, tri| bounds.union(tri.bounds));
        if triangles.len() <= Self::LEAF_TRIANGLE_COUNT {
            let node_index = nodes.len();
            nodes.push(RaycastBvhNode {
                bounds,
                left: None,
                right: None,
                triangles: triangles.iter().map(|tri| tri.triangle_index).collect(),
            });
            return node_index;
        }

        let centroid_bounds = triangles.iter().fold(Aabb::empty(), |bounds, tri| {
            bounds.include_point(tri.centroid)
        });
        let axis = centroid_bounds.longest_axis();
        triangles
            .sort_by(|a, b| component(a.centroid, axis).total_cmp(&component(b.centroid, axis)));
        let split = triangles.len() / 2;
        let (left_triangles, right_triangles) = triangles.split_at_mut(split);
        let left = Self::build_node(nodes, left_triangles);
        let right = Self::build_node(nodes, right_triangles);

        let node_index = nodes.len();
        nodes.push(RaycastBvhNode {
            bounds,
            left: Some(left),
            right: Some(right),
            triangles: Vec::new(),
        });
        node_index
    }
}

impl Aabb {
    fn empty() -> Self {
        Self {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }

    fn from_triangle(p0: Vec3, p1: Vec3, p2: Vec3) -> Self {
        Self::empty()
            .include_point(p0)
            .include_point(p1)
            .include_point(p2)
    }

    fn include_point(self, point: Vec3) -> Self {
        Self {
            min: self.min.min(point),
            max: self.max.max(point),
        }
    }

    fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    fn longest_axis(self) -> usize {
        let extent = self.max - self.min;
        if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }

    fn ray_intersection(self, ray_origin: Vec3, ray_dir: Vec3, max_t: f32) -> Option<f32> {
        let mut t_min: f32 = 0.0;
        let mut t_max = max_t;
        for axis in 0..3 {
            let origin = component(ray_origin, axis);
            let dir = component(ray_dir, axis);
            let min = component(self.min, axis);
            let max = component(self.max, axis);
            if dir.abs() <= f32::EPSILON {
                if origin < min || origin > max {
                    return None;
                }
                continue;
            }

            let inv_dir = 1.0 / dir;
            let mut near = (min - origin) * inv_dir;
            let mut far = (max - origin) * inv_dir;
            if near > far {
                std::mem::swap(&mut near, &mut far);
            }
            t_min = t_min.max(near);
            t_max = t_max.min(far);
            if t_min > t_max {
                return None;
            }
        }
        Some(t_min)
    }
}

fn component(v: Vec3, axis: usize) -> f32 {
    match axis {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn point_in_uv_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    const EPSILON: f32 = 1e-6;
    let v0 = b - a;
    let v1 = c - a;
    let v2 = p - a;
    let denom = v0.perp_dot(v1);
    if denom.abs() <= EPSILON {
        return false;
    }
    let u = v2.perp_dot(v1) / denom;
    let v = v0.perp_dot(v2) / denom;
    u >= -EPSILON && v >= -EPSILON && u + v <= 1.0 + EPSILON
}

const TRIANGLE_EDGE_VERTEX_INDICES: [[usize; 2]; 3] = [[0, 1], [1, 2], [2, 0]];

fn triangle_vertices(
    positions: &[Vec3],
    uvs: &[Vec2],
    indices: [usize; 3],
) -> Option<[(Vec3, Vec2); 3]> {
    Some([
        (*positions.get(indices[0])?, *uvs.get(indices[0])?),
        (*positions.get(indices[1])?, *uvs.get(indices[1])?),
        (*positions.get(indices[2])?, *uvs.get(indices[2])?),
    ])
}

fn position_key(position: Vec3) -> PositionKey {
    const POSITION_KEY_EPSILON: f32 = 1e-6;
    PositionKey {
        x: quantize_position_component(position.x, POSITION_KEY_EPSILON),
        y: quantize_position_component(position.y, POSITION_KEY_EPSILON),
        z: quantize_position_component(position.z, POSITION_KEY_EPSILON),
    }
}

fn quantize_position_component(value: f32, epsilon: f32) -> i64 {
    if !value.is_finite() {
        return 0;
    }
    if value == 0.0 {
        return 0;
    }
    (value / epsilon).round() as i64
}

fn geometry_edge_key(a: PositionKey, b: PositionKey) -> GeometryEdgeKey {
    if a <= b {
        GeometryEdgeKey(a, b)
    } else {
        GeometryEdgeKey(b, a)
    }
}

fn distance_to_segment_3d(point: Vec3, a: Vec3, b: Vec3) -> f32 {
    let segment = b - a;
    let length_squared = segment.length_squared();
    if length_squared <= f32::EPSILON {
        return point.distance(a);
    }
    let t = ((point - a).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance(a + segment * t)
}

fn triangle_min_altitude(points: [Vec3; 3]) -> f32 {
    TRIANGLE_EDGE_VERTEX_INDICES
        .into_iter()
        .enumerate()
        .map(|(opposite, [a, b])| {
            distance_to_segment_3d(points[(opposite + 2) % 3], points[a], points[b])
        })
        .fold(f32::INFINITY, f32::min)
}

fn smoothstep01(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn blend_face_and_feature_normal(face: Vec3, feature: Vec3, weight: f32) -> Option<Vec3> {
    let mut feature = feature;
    if feature.dot(face) < 0.0 {
        feature = -feature;
    }
    let normal = face
        .lerp(feature, weight.clamp(0.0, 1.0))
        .normalize_or_zero();
    (normal.length_squared() > f32::EPSILON && normal.is_finite()).then_some(normal)
}

fn classify_uv_paint_edge_pair(a: &EdgeOccurrence, b: &EdgeOccurrence) -> UvPaintEdgeKind {
    if a.material_index != b.material_index {
        return UvPaintEdgeKind::MaterialBoundary;
    }
    if edge_uvs_match(a, b) {
        UvPaintEdgeKind::Continuous
    } else {
        UvPaintEdgeKind::UvDiscontinuity
    }
}

fn edge_uvs_match(a: &EdgeOccurrence, b: &EdgeOccurrence) -> bool {
    const UV_EPSILON: f32 = 1e-5;
    let same_orientation = a.position_keys == b.position_keys;
    let reversed_orientation =
        a.position_keys[0] == b.position_keys[1] && a.position_keys[1] == b.position_keys[0];

    if same_orientation {
        uv_points_match(a.uvs[0], b.uvs[0], UV_EPSILON)
            && uv_points_match(a.uvs[1], b.uvs[1], UV_EPSILON)
    } else if reversed_orientation {
        uv_points_match(a.uvs[0], b.uvs[1], UV_EPSILON)
            && uv_points_match(a.uvs[1], b.uvs[0], UV_EPSILON)
    } else {
        false
    }
}

fn uv_points_match(a: Vec2, b: Vec2, epsilon: f32) -> bool {
    (a - b).abs().max_element() <= epsilon
}

fn distance_to_uv_paint_boundary_edges(
    uv: Vec2,
    tri_uvs: [Vec2; 3],
    edge_kinds: [UvPaintEdgeKind; 3],
) -> f32 {
    let mut distance = f32::INFINITY;
    for (local_edge, [a, b]) in TRIANGLE_EDGE_VERTEX_INDICES.into_iter().enumerate() {
        if edge_kinds[local_edge].blocks_partial_surface_rect() {
            distance = distance.min(point_to_segment_distance(uv, tri_uvs[a], tri_uvs[b]));
        }
    }
    distance
}

fn distance_to_uv_triangle_edge(uv: Vec2, uv0: Vec2, uv1: Vec2, uv2: Vec2) -> f32 {
    point_to_segment_distance(uv, uv0, uv1)
        .min(point_to_segment_distance(uv, uv1, uv2))
        .min(point_to_segment_distance(uv, uv2, uv0))
}

fn point_to_segment_distance(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let segment = b - a;
    let len_sq = segment.length_squared();
    if len_sq <= f32::EPSILON {
        return (point - a).length();
    }
    let t = ((point - a).dot(segment) / len_sq).clamp(0.0, 1.0);
    (point - (a + segment * t)).length()
}

fn segment_param(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let segment = b - a;
    let len_sq = segment.length_squared();
    if len_sq <= f32::EPSILON {
        return 0.5;
    }
    ((point - a).dot(segment) / len_sq).clamp(0.0, 1.0)
}

fn oriented_neighbor_edge_uvs(
    neighbor: &UvPaintEdgeNeighbor,
    source_position_keys: [PositionKey; 2],
) -> (Vec2, Vec2) {
    if neighbor.position_keys == source_position_keys {
        (neighbor.uvs[0], neighbor.uvs[1])
    } else if neighbor.position_keys[0] == source_position_keys[1]
        && neighbor.position_keys[1] == source_position_keys[0]
    {
        (neighbor.uvs[1], neighbor.uvs[0])
    } else {
        (neighbor.uvs[0], neighbor.uvs[1])
    }
}

fn uv_center_radius_to_pixel_rect(
    texture_size: [u32; 2],
    uv: Vec2,
    radius_uv: f32,
    guard_px: f32,
) -> Option<RectU32> {
    if !(uv.x.is_finite() && uv.y.is_finite() && radius_uv.is_finite() && guard_px.is_finite()) {
        return None;
    }
    let texture_width = texture_size[0].max(1) as f32;
    let texture_height = texture_size[1].max(1) as f32;
    let center_x = uv.x * texture_width;
    let center_y = uv.y * texture_height;
    let radius_x = radius_uv.max(0.0) * texture_width + guard_px.max(0.0);
    let radius_y = radius_uv.max(0.0) * texture_height + guard_px.max(0.0);
    pixel_bbox_to_rect_for_texture(
        texture_size,
        center_x - radius_x,
        center_y - radius_y,
        center_x + radius_x,
        center_y + radius_y,
    )
}

fn pixel_bbox_to_rect_for_texture(
    texture_size: [u32; 2],
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> Option<RectU32> {
    if !(min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite()) {
        return None;
    }
    let x0 = min_x.min(max_x).floor().clamp(0.0, texture_size[0] as f32) as u32;
    let y0 = min_y.min(max_y).floor().clamp(0.0, texture_size[1] as f32) as u32;
    let x1 = min_x.max(max_x).ceil().clamp(0.0, texture_size[0] as f32) as u32;
    let y1 = min_y.max(max_y).ceil().clamp(0.0, texture_size[1] as f32) as u32;
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    Some(RectU32 {
        origin: [x0, y0],
        size: [x1 - x0, y1 - y0],
    })
}

fn ray_intersects_triangle(
    ray_origin: Vec3,
    ray_dir: Vec3,
    p0: Vec3,
    p1: Vec3,
    p2: Vec3,
) -> Option<(f32, f32, f32)> {
    const EPSILON: f32 = 1e-6;

    let edge1 = p1 - p0;
    let edge2 = p2 - p0;
    let h = ray_dir.cross(edge2);
    let a = edge1.dot(h);
    if a.abs() < EPSILON {
        return None;
    }

    let f = 1.0 / a;
    let s = ray_origin - p0;
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }

    let q = s.cross(edge1);
    let v = f * ray_dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }

    let t = f * edge2.dot(q);
    (t > EPSILON).then_some((t, u, v))
}

impl Document {
    pub fn from_imported(asset: ImportedAsset) -> Self {
        Self::new(asset.mesh, asset.materials)
    }

    pub fn has_materials(&self) -> bool {
        !self.materials.is_empty()
    }

    pub fn composite_tree(&self, material_index: usize) -> Option<CompositeTree> {
        self.materials
            .get(material_index)
            .map(|_| self.composite_tree_for_material(material_index))
    }

    pub fn raster_surfaces(&self) -> Vec<PaintSurfaceId> {
        self.layer_tree
            .ordered_raster_layers()
            .into_iter()
            .flat_map(|layer_id| {
                self.materials
                    .iter()
                    .enumerate()
                    .map(move |(material_index, _)| {
                        PaintSurfaceId::raster(material_index.into(), layer_id)
                    })
            })
            .collect()
    }

    pub fn raster_surfaces_in_subtree(&self, layer_id: LayerId) -> Vec<PaintSurfaceId> {
        self.layer_tree
            .raster_layers_in_subtree(layer_id)
            .into_iter()
            .flat_map(|layer_id| {
                self.materials
                    .iter()
                    .enumerate()
                    .map(move |(material_index, _)| {
                        PaintSurfaceId::raster(material_index.into(), layer_id)
                    })
            })
            .collect()
    }

    pub fn layer_surfaces_in_subtree(&self, layer_id: LayerId) -> Vec<PaintSurfaceId> {
        let mut layers = Vec::new();
        self.collect_layer_surface_owners(layer_id, &mut layers);
        layers
            .into_iter()
            .flat_map(|(layer_id, include_raster)| {
                self.materials
                    .iter()
                    .enumerate()
                    .flat_map(move |(material_index, _)| {
                        let mut surfaces = Vec::new();
                        if include_raster {
                            surfaces.push(PaintSurfaceId::raster(material_index.into(), layer_id));
                        }
                        if self.layer_tree.has_layer_mask(layer_id) {
                            surfaces
                                .push(PaintSurfaceId::layer_mask(material_index.into(), layer_id));
                        }
                        surfaces
                    })
            })
            .collect()
    }

    fn collect_layer_surface_owners(&self, layer_id: LayerId, out: &mut Vec<(LayerId, bool)>) {
        let Some(node) = self.layer_tree.get(layer_id) else {
            return;
        };
        out.push((layer_id, matches!(node.content, LayerContent::Raster)));
        for &child_id in self.layer_tree.children(layer_id).unwrap_or_default() {
            self.collect_layer_surface_owners(child_id, out);
        }
    }

    pub fn layer_mask_surfaces(&self, layer_id: LayerId) -> Vec<PaintSurfaceId> {
        if !self.layer_tree.has_layer_mask(layer_id) {
            return Vec::new();
        }
        self.materials
            .iter()
            .enumerate()
            .map(|(material_index, _)| PaintSurfaceId::layer_mask(material_index.into(), layer_id))
            .collect()
    }

    pub fn resize_material_texture(
        &mut self,
        material_index: MaterialIndex,
        new_size: [u32; 2],
    ) -> Result<Option<MaterialTextureResize>> {
        if new_size[0] == 0 || new_size[1] == 0 {
            bail!("material texture size must be > 0");
        }
        let current_size = self
            .materials
            .get(material_index.as_usize())
            .ok_or_else(|| anyhow::anyhow!("material does not exist: {material_index}"))?
            .texture_size;
        if current_size == new_size {
            return Ok(None);
        }

        let before = self.material_texture_state_snapshot(material_index)?;
        let surfaces = before
            .surfaces
            .iter()
            .map(|snapshot| {
                let rgba8 = if snapshot.surface.is_mask() {
                    resize_mask_rgba8(snapshot.texture_size, new_size, &snapshot.rgba8)?
                } else {
                    resize_rgba8(snapshot.texture_size, new_size, &snapshot.rgba8)?
                };
                Ok(LayerImageSnapshot {
                    surface: snapshot.surface,
                    texture_size: new_size,
                    rgba8,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let selection_mask = before
            .selection_mask
            .as_ref()
            .map(|snapshot| -> Result<SelectionMaskSnapshot> {
                Ok(SelectionMaskSnapshot {
                    texture_size: new_size,
                    r8: resize_r8(snapshot.texture_size, new_size, &snapshot.r8)?,
                })
            })
            .transpose()?;
        let after = MaterialTextureStateSnapshot {
            material_index,
            texture_size: new_size,
            surfaces,
            selection_mask,
        };
        self.restore_material_texture_state(&after)?;
        Ok(Some(MaterialTextureResize { before, after }))
    }

    pub fn restore_material_texture_state(
        &mut self,
        snapshot: &MaterialTextureStateSnapshot,
    ) -> Result<()> {
        if snapshot.texture_size[0] == 0 || snapshot.texture_size[1] == 0 {
            bail!("material texture snapshot size must be > 0");
        }
        if self
            .materials
            .get(snapshot.material_index.as_usize())
            .is_none()
        {
            bail!("material does not exist: {}", snapshot.material_index);
        }
        let expected_selection_len = selection_mask_len(snapshot.texture_size)?;
        if let Some(selection) = &snapshot.selection_mask {
            if selection.texture_size != snapshot.texture_size {
                bail!("selection mask snapshot size does not match material texture size");
            }
            if selection.r8.len() != expected_selection_len {
                bail!("selection mask snapshot payload size does not match material texture size");
            }
        }

        let current_surfaces = self
            .layer_surfaces_in_subtree(self.layer_tree.root())
            .into_iter()
            .filter(|surface| {
                surface.material_index() == snapshot.material_index
                    && self.tiles.contains_surface(*surface)
            })
            .collect::<HashSet<_>>();
        let mut replacements = Vec::with_capacity(snapshot.surfaces.len());
        let mut seen = HashSet::with_capacity(snapshot.surfaces.len());
        for surface_snapshot in &snapshot.surfaces {
            if surface_snapshot.surface.material_index() != snapshot.material_index {
                bail!("material texture snapshot contains a surface from another material");
            }
            if surface_snapshot.texture_size != snapshot.texture_size {
                bail!("surface snapshot size does not match material texture size");
            }
            if !seen.insert(surface_snapshot.surface) {
                bail!("material texture snapshot contains a duplicate surface");
            }
            replacements.push((
                surface_snapshot.surface,
                snapshot.texture_size,
                InitialPixels::Rgba8(surface_snapshot.rgba8.clone()),
            ));
        }
        if seen != current_surfaces {
            bail!("material texture snapshot surface set does not match the document");
        }

        self.tiles.replace_surfaces_atomically(replacements)?;
        self.selection_masks
            .restore_snapshot(snapshot.material_index, snapshot.selection_mask.as_ref())?;
        self.materials[snapshot.material_index.as_usize()].texture_size = snapshot.texture_size;
        Ok(())
    }

    fn material_texture_state_snapshot(
        &self,
        material_index: MaterialIndex,
    ) -> Result<MaterialTextureStateSnapshot> {
        let material = self
            .materials
            .get(material_index.as_usize())
            .ok_or_else(|| anyhow::anyhow!("material does not exist: {material_index}"))?;
        let surfaces = self
            .layer_surfaces_in_subtree(self.layer_tree.root())
            .into_iter()
            .filter(|surface| {
                surface.material_index() == material_index && self.tiles.contains_surface(*surface)
            })
            .map(|surface| self.tiles.read_surface_full(surface))
            .collect::<Result<Vec<_>>>()?;
        for surface in &surfaces {
            if surface.texture_size != material.texture_size {
                bail!("material surface size does not match material texture size");
            }
        }
        let selection_mask = self.selection_masks.snapshot(material_index);
        if let Some(selection) = &selection_mask {
            if selection.texture_size != material.texture_size {
                bail!("selection mask size does not match material texture size");
            }
        }
        Ok(MaterialTextureStateSnapshot {
            material_index,
            texture_size: material.texture_size,
            surfaces,
            selection_mask,
        })
    }

    pub fn texture_size_for_surface(&self, surface: PaintSurfaceId) -> Option<[u32; 2]> {
        let valid_layer_surface = match surface.role {
            crate::core::surface::PaintSurfaceRole::Raster => {
                self.layer_tree.is_paintable(surface.layer_id)
            }
            crate::core::surface::PaintSurfaceRole::LayerMask => {
                self.layer_tree.has_layer_mask(surface.layer_id)
            }
        };
        if !valid_layer_surface {
            return None;
        }
        self.materials
            .get(surface.material_index().as_usize())
            .map(|material| material.texture_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::surface::LayerMaterialMask;

    #[test]
    fn raster_layer_surface_creation_and_removal_are_atomic() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [16, 4]),
            ],
        );
        let base = document.layer_tree.default_raster_layer().unwrap();
        let added = document
            .add_raster_layer(LayerInsertion::Above(base), "Layer")
            .unwrap()
            .unwrap();

        assert_eq!(added.created_surfaces.len(), 2);
        for surface in &added.created_surfaces {
            assert_eq!(
                document.tiles.texture_size(*surface),
                Some(document.materials[surface.material_index().as_usize()].texture_size)
            );
        }

        let missing = added.created_surfaces[0];
        let remaining = added.created_surfaces[1];
        document.tiles.delete_surface(missing).unwrap();
        let revision_before = document.tiles.revision();
        assert!(document.remove_layer_with_surfaces(added.layer_id).is_err());
        assert!(document.layer_tree.contains(added.layer_id));
        assert!(document.tiles.contains_surface(remaining));
        assert_eq!(document.tiles.revision(), revision_before);
    }

    #[test]
    fn material_texture_resize_updates_surfaces_masks_and_selection_only_for_target_material() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [2, 2]),
                MaterialSpec::new("B", [3, 3]),
            ],
        );
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let raster_a = PaintSurfaceId::raster(0.into(), layer);
        let raster_b = PaintSurfaceId::raster(1.into(), layer);
        let raster_a_pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let raster_b_pixels = [10, 20, 30, 255].repeat(9);
        document
            .tiles
            .write_surface_rect(
                raster_a,
                RectU32::full([2, 2]),
                &crate::core::tile_payload::PixelSnapshotData::contiguous(raster_a_pixels.clone()),
            )
            .unwrap();
        document
            .tiles
            .write_surface_rect(
                raster_b,
                RectU32::full([3, 3]),
                &crate::core::tile_payload::PixelSnapshotData::contiguous(raster_b_pixels.clone()),
            )
            .unwrap();

        assert!(document.layer_tree.add_layer_mask(layer));
        let mask_a = PaintSurfaceId::layer_mask(0.into(), layer);
        let mask_b = PaintSurfaceId::layer_mask(1.into(), layer);
        let mask_a_pixels = vec![
            0, 0, 0, 0, 85, 85, 85, 85, 170, 170, 170, 170, 255, 255, 255, 255,
        ];
        let mask_b_pixels = [200; 4].repeat(9);
        document
            .tiles
            .create_surface(mask_a, [2, 2], InitialPixels::Rgba8(mask_a_pixels.clone()))
            .unwrap();
        document
            .tiles
            .create_surface(mask_b, [3, 3], InitialPixels::Rgba8(mask_b_pixels.clone()))
            .unwrap();

        let selection_before = vec![0, 64, 192, 255];
        document
            .selection_masks
            .write_mask(0.into(), [2, 2], selection_before.clone())
            .unwrap();
        let active_before = document.active_selection.clone();
        let raster_b_before = document.tiles.read_surface_full(raster_b).unwrap();
        let mask_b_before = document.tiles.read_surface_full(mask_b).unwrap();

        let resize = document
            .resize_material_texture(0.into(), [4, 4])
            .unwrap()
            .unwrap();

        assert_eq!(document.materials[0].texture_size, [4, 4]);
        assert_eq!(document.materials[1].texture_size, [3, 3]);
        assert_eq!(document.tiles.texture_size(raster_a), Some([4, 4]));
        assert_eq!(document.tiles.texture_size(mask_a), Some([4, 4]));
        assert_eq!(
            document.tiles.read_surface_full(raster_b).unwrap(),
            raster_b_before
        );
        assert_eq!(
            document.tiles.read_surface_full(mask_b).unwrap(),
            mask_b_before
        );
        let selection_after = document.selection_masks.snapshot(0.into()).unwrap();
        assert_eq!(selection_after.texture_size, [4, 4]);
        assert_eq!(selection_after.r8.len(), 16);
        assert_eq!(document.active_selection, active_before);

        document
            .restore_material_texture_state(&resize.before)
            .unwrap();
        assert_eq!(document.materials[0].texture_size, [2, 2]);
        assert_eq!(
            document.tiles.read_surface_full(raster_a).unwrap().rgba8,
            raster_a_pixels
        );
        assert_eq!(
            document.tiles.read_surface_full(mask_a).unwrap().rgba8,
            mask_a_pixels
        );
        assert_eq!(
            document.selection_masks.snapshot(0.into()).unwrap().r8,
            selection_before
        );

        document
            .restore_material_texture_state(&resize.after)
            .unwrap();
        assert_eq!(document.materials[0].texture_size, [4, 4]);
        assert_eq!(
            document.tiles.read_surface_full(raster_a).unwrap().rgba8,
            resize
                .after
                .surfaces
                .iter()
                .find(|surface| surface.surface == raster_a)
                .unwrap()
                .rgba8
        );
    }

    #[test]
    fn material_texture_resize_noop_preserves_revisions() {
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])]);
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let surface = PaintSurfaceId::raster(0.into(), layer);
        let tile_revision = document.tiles.revision();
        let surface_revision = document.tiles.surface_revision(surface);
        let selection_revision = document.selection_masks.revision();

        assert!(
            document
                .resize_material_texture(0.into(), [8, 8])
                .unwrap()
                .is_none()
        );

        assert_eq!(document.tiles.revision(), tile_revision);
        assert_eq!(document.tiles.surface_revision(surface), surface_revision);
        assert_eq!(document.selection_masks.revision(), selection_revision);
    }

    #[test]
    fn raster_layer_creation_failure_keeps_tree_and_surfaces_unchanged() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [16, 4]),
            ],
        );
        let base = document.layer_tree.default_raster_layer().unwrap();
        let tree_before = document.layer_tree.clone();
        let tiles_before = document.tiles.clone();
        document.materials[1].texture_size = [0, 0];

        assert!(
            document
                .add_raster_layer(LayerInsertion::Above(base), "Layer")
                .is_err()
        );

        assert_eq!(document.layer_tree, tree_before);
        assert_eq!(document.tiles, tiles_before);
    }

    #[test]
    fn layer_tree_restore_failure_keeps_tree_and_surfaces_unchanged() {
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])]);
        let base = document.layer_tree.default_raster_layer().unwrap();
        let mut target_tree = document.layer_tree.clone();
        target_tree
            .add_raster_layer_above(base, "Restored Layer")
            .unwrap();
        let before = document.clone();

        assert!(document.restore_layer_tree(target_tree, &[]).is_err());

        assert_eq!(document, before);
    }

    #[test]
    fn raster_merge_failure_keeps_source_tree_and_surfaces_unchanged() {
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])]);
        let base = document.layer_tree.default_raster_layer().unwrap();
        let top = document
            .add_raster_layer(LayerInsertion::Above(base), "Top")
            .unwrap()
            .unwrap()
            .layer_id;
        let plan = document.layer_tree.layer_merge_plan(&[top, base]).unwrap();
        document.materials[0].texture_size = [0, 0];
        let before = document.clone();

        assert!(document.replace_layers_with_raster(&plan).is_err());

        assert_eq!(document, before);
    }

    fn test_triangle_mesh(
        sub_meshes: Vec<SubMesh>,
        mesh_objects: Vec<MeshObject>,
        triangle_mesh_ids: Vec<MeshId>,
    ) -> Result<MeshData> {
        MeshData::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            vec![Vec2::ZERO, Vec2::X, Vec2::Y],
            vec![Vec3::Z; 3],
            vec![[0, 1, 2]],
            sub_meshes,
            mesh_objects,
            triangle_mesh_ids,
        )
    }

    #[test]
    fn mesh_reload_preserves_existing_pixels_and_adds_surfaces_for_new_materials() {
        let mesh_id = MeshId(0);
        let old_mesh = test_triangle_mesh(
            vec![SubMesh {
                mesh_id,
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Body".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Character".to_owned(),
            }],
            vec![mesh_id],
        )
        .unwrap();
        let mut document = Document::new(old_mesh, vec![MaterialSpec::new("Body", [2, 2])]);
        let body_id = document.materials[0].id;
        let mesh_object_id = document.mesh_object_persistent_id(mesh_id).unwrap();
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let top_layer = document
            .add_raster_layer(LayerInsertion::Above(layer), "Top")
            .unwrap()
            .unwrap()
            .layer_id;
        let body_surface = PaintSurfaceId::raster(0.into(), layer);
        let body_pixels = [12, 34, 56, 255].repeat(4);
        document
            .tiles
            .write_surface_rect(
                body_surface,
                RectU32::full([2, 2]),
                &crate::core::tile_payload::PixelSnapshotData::contiguous(body_pixels.clone()),
            )
            .unwrap();
        assert!(document.layer_tree.add_layer_mask(layer));
        document
            .tiles
            .create_surface(
                PaintSurfaceId::layer_mask(0.into(), layer),
                [2, 2],
                InitialPixels::SolidRgba8([128; 4]),
            )
            .unwrap();
        document.active_selection.enable_material_mask(0.into());
        document
            .selection_masks
            .write_mask(0.into(), [2, 2], vec![255; 4])
            .unwrap();

        let reloaded_mesh = test_triangle_mesh(
            vec![SubMesh {
                mesh_id,
                start_index: 0,
                index_count: 3,
                material_index: 1,
                material_name: "Metal".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Character".to_owned(),
            }],
            vec![mesh_id],
        )
        .unwrap();

        let result = document
            .reload_mesh(reloaded_mesh, vec![MaterialSpec::new("Metal", [4, 4])])
            .unwrap();

        assert_eq!(document.materials.len(), 2);
        assert_eq!(document.materials[0].id, body_id);
        assert_eq!(result.appended_material_indices, vec![MaterialIndex(1)]);
        assert_eq!(document.mesh.sub_meshes[0].material_index, 1);
        assert_eq!(
            document.mesh_object_persistent_id(mesh_id),
            Some(mesh_object_id)
        );
        assert_eq!(
            document
                .tiles
                .read_surface_full(body_surface)
                .unwrap()
                .rgba8,
            body_pixels
        );
        assert_eq!(
            document
                .tiles
                .read_surface_full(PaintSurfaceId::raster(1.into(), layer))
                .unwrap()
                .rgba8,
            vec![255; 4 * 4 * 4]
        );
        assert_eq!(
            document
                .tiles
                .read_surface_full(PaintSurfaceId::raster(1.into(), top_layer))
                .unwrap()
                .rgba8,
            vec![0; 4 * 4 * 4]
        );
        assert_eq!(
            document
                .tiles
                .read_surface_full(PaintSurfaceId::layer_mask(1.into(), layer))
                .unwrap()
                .rgba8,
            vec![255; 4 * 4 * 4]
        );
        assert!(!document.active_selection.enabled);
        assert_eq!(document.active_selection.masks.len(), 2);
        assert!(document.selection_masks.snapshot(0.into()).is_none());
    }

    #[test]
    fn mesh_data_rejects_duplicate_mesh_object_ids() {
        let error = test_triangle_mesh(
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![
                MeshObject {
                    id: MeshId(0),
                    name: "A".to_owned(),
                },
                MeshObject {
                    id: MeshId(0),
                    name: "B".to_owned(),
                },
            ],
            vec![MeshId(0)],
        )
        .unwrap_err();

        assert!(error.to_string().contains("mesh object ids must be unique"));
    }

    #[test]
    fn mesh_data_rejects_unknown_triangle_mesh_ids() {
        let error = test_triangle_mesh(
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(1)],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("triangle references an unknown mesh object")
        );
    }

    #[test]
    fn mesh_data_rejects_sub_mesh_ranges_with_other_mesh_ids() {
        let error = MeshData::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::ONE],
            vec![Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE],
            vec![Vec3::Z; 4],
            vec![[0, 1, 2], [1, 3, 2]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 6,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![
                MeshObject {
                    id: MeshId(0),
                    name: "A".to_owned(),
                },
                MeshObject {
                    id: MeshId(1),
                    name: "B".to_owned(),
                },
            ],
            vec![MeshId(0), MeshId(1)],
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("sub-mesh range contains triangles from another mesh object")
        );
    }

    #[test]
    fn document_assigns_stable_material_identity_metadata() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [16, 16]),
            ],
        );
        let first_identity = (document.materials[0].id, document.materials[0].ui_color);
        let second_identity = (document.materials[1].id, document.materials[1].ui_color);

        document.materials[0].name = "Renamed".to_owned();
        document.materials.swap(0, 1);

        assert_eq!(
            (document.materials[0].id, document.materials[0].ui_color),
            second_identity
        );
        assert_eq!(
            (document.materials[1].id, document.materials[1].ui_color),
            first_identity
        );
        assert_eq!(document.material_id(0.into()), Some(second_identity.0));
        assert_eq!(document.material_index(first_identity.0), Some(1.into()));
        assert_eq!(document.material_index(second_identity.0), Some(0.into()));
        assert_eq!(
            document.material(1.into()).map(|material| material.id),
            Some(first_identity.0)
        );
        assert_eq!(document.next_material_id(), MaterialId(3));
    }

    #[test]
    fn document_materializes_specs_with_sequential_ids() {
        let document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("First", [8, 8]),
                MaterialSpec::new("New", [8, 8]),
            ],
        );

        assert_eq!(document.materials[0].id, MaterialId(1));
        assert_eq!(document.materials[1].id, MaterialId(2));
        assert_ne!(
            document.materials[0].ui_color,
            document.materials[1].ui_color
        );
        assert_eq!(document.next_material_id(), MaterialId(3));
    }

    #[test]
    fn composite_tree_preserves_group_composite_mode() {
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])]);
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let group = document.layer_tree.add_group_above(layer, "Group").unwrap();
        assert!(
            document
                .layer_tree
                .set_group_composite_mode(group, GroupCompositeMode::PassThrough)
        );

        let tree = document.composite_tree_for_material(0);
        let composite_group = tree
            .root
            .children
            .iter()
            .find_map(|node| match node {
                CompositeNode::Group(group_node) if group_node.layer_id == Some(group) => {
                    Some(group_node)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(composite_group.mode, GroupCompositeMode::PassThrough);
    }

    #[test]
    fn composite_tree_filters_raster_layers_by_material_mask() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        );
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let material_a = document.materials[0].id;
        assert!(document.layer_tree.set_layer_material_mask(
            layer,
            LayerMaterialMask::Specified([material_a].into_iter().collect()),
        ));

        let tree_a = document.composite_tree_for_material(0);
        let tree_b = document.composite_tree_for_material(1);

        assert!(matches!(
            tree_a.root.children.as_slice(),
            [CompositeNode::Raster { surface, .. }] if surface.layer_id == layer
        ));
        assert!(tree_b.root.children.is_empty());
    }

    #[test]
    fn composite_tree_represents_solid_fill_as_constant_color_with_optional_mask() {
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])]);
        let raster = document.layer_tree.default_raster_layer().unwrap();
        let fill = document
            .layer_tree
            .add_solid_fill_layer_above(raster, "Fill", [0.25, 0.5, 0.75])
            .unwrap();
        assert!(document.layer_tree.add_layer_mask(fill));

        let tree = document.composite_tree_for_material(0);

        assert!(matches!(
            tree.root.children.as_slice(),
            [CompositeNode::Raster { .. }, CompositeNode::SolidFill {
                layer_id,
                color,
                mask: Some(mask),
                ..
            }] if *layer_id == fill
                && *color == [0.25, 0.5, 0.75]
                && *mask == PaintSurfaceId::layer_mask(0.into(), fill)
        ));
    }

    #[test]
    fn composite_tree_filters_solid_fill_by_material_mask() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        );
        let raster = document.layer_tree.default_raster_layer().unwrap();
        let fill = document
            .layer_tree
            .add_solid_fill_layer_above(raster, "Fill", [1.0, 0.0, 0.0])
            .unwrap();
        let material_a = document.materials[0].id;
        assert!(document.layer_tree.set_layer_material_mask(
            fill,
            LayerMaterialMask::Specified([material_a].into_iter().collect()),
        ));

        let tree_a = document.composite_tree_for_material(0);
        let tree_b = document.composite_tree_for_material(1);

        assert!(tree_a.root.children.iter().any(|node| matches!(
            node,
            CompositeNode::SolidFill { layer_id, .. } if *layer_id == fill
        )));
        assert!(!tree_b.root.children.iter().any(|node| matches!(
            node,
            CompositeNode::SolidFill { layer_id, .. } if *layer_id == fill
        )));
    }

    #[test]
    fn composite_tree_filters_masked_groups_and_descendants() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [8, 8]),
                MaterialSpec::new("B", [8, 8]),
            ],
        );
        let layer = document.layer_tree.default_raster_layer().unwrap();
        let group = document.layer_tree.add_group_above(layer, "Group").unwrap();
        assert!(document.layer_tree.move_layer(layer, group, 0));
        let material_a = document.materials[0].id;
        assert!(document.layer_tree.set_layer_material_mask(
            group,
            LayerMaterialMask::Specified([material_a].into_iter().collect()),
        ));

        let tree_a = document.composite_tree_for_material(0);
        let tree_b = document.composite_tree_for_material(1);

        assert!(matches!(
            tree_a.root.children.as_slice(),
            [CompositeNode::Group(group_node)]
                if matches!(group_node.children.as_slice(), [CompositeNode::Raster { surface, .. }] if surface.layer_id == layer)
        ));
        assert!(tree_b.root.children.is_empty());
    }

    #[test]
    fn composite_tree_for_unknown_material_has_an_empty_root() {
        let document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [8, 8])]);

        let tree = document.composite_tree_for_material(99);

        assert!(tree.root.children.is_empty());
    }

    fn two_face_mesh(angle_degrees: f32, mesh_ids: [MeshId; 2]) -> MeshData {
        let angle = angle_degrees.to_radians();
        let positions = vec![
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec3::ZERO,
            Vec3::X,
            Vec3::new(0.0, angle.cos(), angle.sin()),
        ];
        MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO, Vec2::X, Vec2::Y, Vec2::ONE, Vec2::Y, Vec2::X],
            vec![Vec3::NEG_X; positions.len()],
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
            vec![
                MeshObject {
                    id: MeshId(0),
                    name: "A".to_owned(),
                },
                MeshObject {
                    id: MeshId(1),
                    name: "B".to_owned(),
                },
            ],
            mesh_ids.to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn smooth_geometric_normal_crosses_uv_and_material_seams_and_ignores_vertex_normals() {
        let mesh = two_face_mesh(45.0, [MeshId(0), MeshId(0)]);
        let expected = (Vec3::Z
            + Vec3::new(
                0.0,
                -45.0_f32.to_radians().sin(),
                45.0_f32.to_radians().cos(),
            ))
        .normalize();

        assert!(
            mesh.local_smooth_triangle_geometric_normal(0, Vec3::new(0.5, 0.0, 0.0), 45.0)
                .unwrap()
                .abs_diff_eq(expected, 1.0e-5)
        );
        assert_eq!(
            mesh.local_smooth_triangle_geometric_normal(0, Vec3::new(0.5, 0.0, 0.0), 44.0),
            Some(Vec3::Z)
        );
        assert_eq!(
            mesh.local_smooth_triangle_geometric_normal(
                0,
                Vec3::new(1.0 / 3.0, 1.0 / 3.0, 0.0),
                180.0,
            ),
            Some(Vec3::Z)
        );
    }

    #[test]
    fn smooth_geometric_normal_does_not_cross_mesh_boundaries() {
        let mesh = two_face_mesh(30.0, [MeshId(0), MeshId(1)]);

        assert_eq!(
            mesh.local_smooth_triangle_geometric_normal(0, Vec3::new(0.5, 0.0, 0.0), 180.0),
            Some(Vec3::Z)
        );
    }

    #[test]
    fn surface_filter_topology_crosses_uv_material_and_hard_normal_seams() {
        let mesh = two_face_mesh(45.0, [MeshId(0), MeshId(0)]);
        let topology = mesh.surface_filter_topology();

        assert_eq!(topology.triangles.len(), 2);
        assert_eq!(topology.triangles[0].material_index, 0);
        assert_eq!(topology.triangles[1].material_index, 1);
        assert_eq!(
            topology.triangles[0].edges[0],
            SurfaceFilterEdge {
                neighbor_triangle: Some(1),
                neighbor_edge: 0,
                reversed: false,
            }
        );
        assert_eq!(
            topology.triangles[1].edges[0],
            SurfaceFilterEdge {
                neighbor_triangle: Some(0),
                neighbor_edge: 0,
                reversed: false,
            }
        );
    }

    #[test]
    fn surface_filter_topology_does_not_cross_mesh_boundaries() {
        let mesh = two_face_mesh(30.0, [MeshId(0), MeshId(1)]);
        let topology = mesh.surface_filter_topology();

        assert_eq!(topology.triangles[0].edges[0], SurfaceFilterEdge::BOUNDARY);
        assert_eq!(topology.triangles[1].edges[0], SurfaceFilterEdge::BOUNDARY);
    }

    #[test]
    fn surface_filter_topology_treats_non_manifold_edges_as_boundaries() {
        let mesh = MeshData::new(
            vec![
                Vec3::ZERO,
                Vec3::X,
                Vec3::Y,
                Vec3::ZERO,
                Vec3::X,
                Vec3::NEG_Y,
                Vec3::ZERO,
                Vec3::X,
                Vec3::Z,
            ],
            vec![
                Vec2::ZERO,
                Vec2::X,
                Vec2::Y,
                Vec2::ZERO,
                Vec2::X,
                Vec2::Y,
                Vec2::ZERO,
                Vec2::X,
                Vec2::Y,
            ],
            vec![Vec3::Z; 9],
            vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 9,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0); 3],
        )
        .unwrap();
        let topology = mesh.surface_filter_topology();

        for triangle in &topology.triangles {
            assert_eq!(triangle.edges[0], SurfaceFilterEdge::BOUNDARY);
        }
    }

    #[test]
    fn surface_filter_topology_omits_degenerate_world_and_uv_triangles() {
        let mesh_id = MeshId(1);
        let mesh = MeshData::new(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(2.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(0.0, 2.0, 0.0),
                Vec3::new(1.0, 2.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0),
            ],
            vec![
                Vec2::ZERO,
                Vec2::X,
                Vec2::Y,
                Vec2::ZERO,
                Vec2::X,
                Vec2::Y,
                Vec2::ZERO,
                Vec2::X,
                Vec2::new(2.0, 0.0),
            ],
            vec![Vec3::Z; 9],
            vec![[0, 1, 2], [3, 4, 5], [6, 7, 8]],
            vec![SubMesh {
                mesh_id,
                start_index: 0,
                index_count: 9,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: mesh_id,
                name: "Mesh".to_owned(),
            }],
            vec![mesh_id; 3],
        )
        .unwrap();

        let topology = mesh.surface_filter_topology();
        assert_eq!(topology.triangles.len(), 1);
        assert_eq!(topology.triangles[0].positions[0], Vec3::ZERO);
    }

    #[test]
    fn surface_filter_reference_density_uses_material_texture_sizes() {
        let mesh = test_triangle_mesh(
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0)],
        )
        .unwrap();

        assert!(
            (mesh.surface_filter_reference_texel_density(&[[100, 100]], &[true]) - 100.0).abs()
                < 1.0e-5
        );
        assert_eq!(
            mesh.surface_filter_reference_texel_density(&[[100, 100]], &[false]),
            1.0
        );
    }

    #[test]
    fn smooth_geometric_normal_handles_angle_endpoints_and_cancelled_average() {
        let coplanar = two_face_mesh(0.0, [MeshId(0), MeshId(0)]);
        assert_eq!(
            coplanar.local_smooth_triangle_geometric_normal(0, Vec3::new(0.5, 0.0, 0.0), 0.0,),
            Some(Vec3::Z)
        );

        let opposing = two_face_mesh(180.0, [MeshId(0), MeshId(0)]);
        assert_eq!(
            opposing.local_smooth_triangle_geometric_normal(0, Vec3::new(0.5, 0.0, 0.0), 180.0,),
            Some(Vec3::Z)
        );
    }

    #[test]
    fn mesh_caches_scene_diagonal_across_clones() {
        let positions = vec![
            Vec3::new(-1.0, -2.0, -3.0),
            Vec3::new(2.0, -2.0, -3.0),
            Vec3::new(-1.0, 2.0, 9.0),
        ];
        let mesh = MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO; positions.len()],
            vec![Vec3::Z; positions.len()],
            vec![[0, 1, 2]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 3,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0)],
        )
        .unwrap();
        let expected = Vec3::new(3.0, 4.0, 12.0).length();

        assert!((mesh.scene_diagonal() - expected).abs() < 1e-6);
        assert_eq!(
            mesh.scene_bounds(),
            Some((Vec3::new(-1.0, -2.0, -3.0), Vec3::new(2.0, 2.0, 9.0)))
        );
        assert_eq!(mesh.clone().scene_diagonal(), mesh.scene_diagonal());
        assert_eq!(mesh.clone().scene_bounds(), mesh.scene_bounds());
    }

    #[test]
    fn raycast_returns_nearest_surface_hit_with_interpolated_normal_and_material() {
        let positions = vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ];
        let uvs = vec![Vec2::ZERO; positions.len()];
        let normals = vec![Vec3::Z; positions.len()];
        let sub_meshes = vec![
            SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 3,
                material_index: 2,
                material_name: "Near".to_owned(),
                wireframe_edges: Vec::new(),
            },
            SubMesh {
                mesh_id: MeshId(1),
                start_index: 3,
                index_count: 3,
                material_index: 7,
                material_name: "Far".to_owned(),
                wireframe_edges: Vec::new(),
            },
        ];
        let mesh = MeshData::new(
            positions,
            uvs,
            normals,
            vec![[0, 1, 2], [3, 4, 5]],
            sub_meshes,
            vec![
                MeshObject {
                    id: MeshId(0),
                    name: "Near".to_owned(),
                },
                MeshObject {
                    id: MeshId(1),
                    name: "Far".to_owned(),
                },
            ],
            vec![MeshId(0), MeshId(1)],
        )
        .unwrap();

        let hit = mesh
            .raycast(Vec3::new(0.0, 0.0, -2.0), Vec3::Z)
            .expect("ray should hit the near triangle");

        assert_eq!(hit.triangle_index, 0);
        assert_eq!(hit.material_index.as_usize(), 2);
        assert_eq!(hit.mesh_id, MeshId(0));
        assert!((hit.world_pos.z - 0.0).abs() < 1e-6);
        assert!((hit.world_normal.length() - 1.0).abs() < 1e-6);
        assert_eq!(hit.world_normal, Vec3::Z);
    }

    #[test]
    fn visible_raycast_skips_hidden_geometry_and_hidden_preferred_triangle() {
        let positions = vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ];
        let mesh = MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO; positions.len()],
            vec![Vec3::Z; positions.len()],
            vec![[0, 1, 2], [3, 4, 5]],
            vec![
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 0,
                    index_count: 3,
                    material_index: 2,
                    material_name: "Near".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id: MeshId(1),
                    start_index: 3,
                    index_count: 3,
                    material_index: 7,
                    material_name: "Far".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![
                MeshObject {
                    id: MeshId(0),
                    name: "Near".to_owned(),
                },
                MeshObject {
                    id: MeshId(1),
                    name: "Far".to_owned(),
                },
            ],
            vec![MeshId(0), MeshId(1)],
        )
        .unwrap();
        let mut visibility = ViewportSceneVisibility::default();
        visibility.set_mesh_visible(MeshId(0), false);

        let hit = mesh
            .raycast_visible(Vec3::new(0.0, 0.0, -2.0), Vec3::Z, &visibility)
            .expect("hidden near mesh should reveal the far triangle");
        assert_eq!(hit.triangle_index, 1);
        assert_eq!(hit.mesh_id, MeshId(1));

        let mut scratch = RaycastScratch::default();
        let hit = mesh
            .raycast_visible_with_preferred_triangle(
                Vec3::new(0.0, 0.0, -2.0),
                Vec3::Z,
                Some(0),
                &visibility,
                &mut scratch,
            )
            .expect("a hidden preferred triangle must not seed the result");
        assert_eq!(hit.triangle_index, 1);

        visibility.set_material_visible(7.into(), false);
        assert!(
            mesh.raycast_visible(Vec3::new(0.0, 0.0, -2.0), Vec3::Z, &visibility)
                .is_none()
        );
    }

    #[test]
    fn document_raycast_culls_single_sided_back_faces_and_falls_through() {
        let positions = vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ];
        let mesh = MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO; positions.len()],
            vec![Vec3::Z; positions.len()],
            vec![[0, 1, 2], [5, 4, 3]],
            vec![
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 0,
                    index_count: 3,
                    material_index: 0,
                    material_name: "Near".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id: MeshId(1),
                    start_index: 3,
                    index_count: 3,
                    material_index: 1,
                    material_name: "Far".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![
                MeshObject {
                    id: MeshId(0),
                    name: "Near".to_owned(),
                },
                MeshObject {
                    id: MeshId(1),
                    name: "Far".to_owned(),
                },
            ],
            vec![MeshId(0), MeshId(1)],
        )
        .unwrap();
        let single_sided = MaterialRenderSettings {
            double_sided: false,
            ..MaterialRenderSettings::default()
        };
        let mut document = Document::new(
            mesh,
            vec![
                MaterialSpec::new("Near", [8, 8]).with_render_settings(single_sided),
                MaterialSpec::new("Far", [8, 8]).with_render_settings(single_sided),
            ],
        );
        let visibility = ViewportSceneVisibility::default();
        let mut scratch = RaycastScratch::default();

        let hit = document
            .raycast_visible_with_preferred_triangle(
                Vec3::new(0.0, 0.0, -2.0),
                Vec3::Z,
                Some(0),
                &visibility,
                &mut scratch,
            )
            .expect("back-facing preferred triangle should fall through to the front face");
        assert_eq!(hit.triangle_index, 1);

        document.materials[0].render_settings.double_sided = true;
        let hit = document
            .raycast_visible_with_scratch(
                Vec3::new(0.0, 0.0, -2.0),
                Vec3::Z,
                &visibility,
                &mut scratch,
            )
            .expect("double-sided back face should remain hittable");
        assert_eq!(hit.triangle_index, 0);
    }

    #[test]
    fn aabb_query_uses_bvh_to_limit_triangle_candidates() {
        let mesh = grid_mesh(16);
        let hit = mesh
            .raycast(Vec3::new(7.5, 8.5, -1.0), Vec3::Z)
            .expect("ray should hit the grid");
        let candidates = mesh.triangle_candidates_intersecting_aabb(
            Vec3::new(7.25, 8.25, -0.1),
            Vec3::new(7.75, 8.75, 0.1),
        );

        assert!(candidates.contains(&hit.triangle_index));
        assert!(candidates.len() < mesh.indices.len());
    }

    #[test]
    fn raycast_with_preferred_triangle_matches_regular_raycast() {
        let mesh = grid_mesh(8);
        let origin = Vec3::new(3.5, 4.25, -2.0);
        let dir = Vec3::Z;
        let regular = mesh
            .raycast(origin, dir)
            .expect("regular raycast should hit");
        let mut scratch = RaycastScratch::default();
        let seeded = mesh
            .raycast_with_preferred_triangle(
                origin,
                dir,
                Some(regular.triangle_index),
                &mut scratch,
            )
            .expect("seeded raycast should hit");

        assert_eq!(seeded, regular);
    }

    #[test]
    fn raycast_with_preferred_triangle_returns_closer_occluder() {
        let positions = vec![
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, -1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ];
        let mesh = MeshData::new(
            positions.clone(),
            vec![Vec2::ZERO; positions.len()],
            vec![Vec3::Z; positions.len()],
            vec![[0, 1, 2], [3, 4, 5]],
            vec![
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 0,
                    index_count: 3,
                    material_index: 0,
                    material_name: "Near".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 3,
                    index_count: 3,
                    material_index: 1,
                    material_name: "Far".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0), MeshId(0)],
        )
        .unwrap();
        let mut scratch = RaycastScratch::default();
        let hit = mesh
            .raycast_with_preferred_triangle(
                Vec3::new(0.0, 0.0, -2.0),
                Vec3::Z,
                Some(1),
                &mut scratch,
            )
            .expect("seeded raycast should still hit the nearest triangle");

        assert_eq!(hit.triangle_index, 0);
        assert_eq!(hit.material_index.as_usize(), 0);
    }

    #[test]
    fn raycast_returns_none_for_zero_direction_or_miss() {
        let mesh = grid_mesh(2);
        assert!(
            mesh.raycast(Vec3::new(0.0, 0.0, -1.0), Vec3::ZERO)
                .is_none()
        );
        assert!(
            mesh.raycast(Vec3::new(-10.0, -10.0, -1.0), Vec3::Z)
                .is_none()
        );
    }

    #[test]
    fn raycast_bvh_matches_linear_scan_across_grid_mesh() {
        let mesh = grid_mesh(8);
        let rays = [
            (Vec3::new(0.25, 0.25, -2.0), Vec3::Z),
            (Vec3::new(3.5, 4.25, -2.0), Vec3::Z),
            (Vec3::new(7.75, 7.75, -2.0), Vec3::Z),
            (Vec3::new(1.5, 1.5, 2.0), -Vec3::Z),
            (Vec3::new(10.0, 10.0, -2.0), Vec3::Z),
        ];

        for (origin, dir) in rays {
            assert_eq!(
                mesh.raycast(origin, dir),
                linear_raycast(&mesh, origin, dir)
            );
        }
    }

    #[test]
    #[ignore = "prints a rough BVH-vs-linear raycast timing for local performance checks"]
    fn raycast_bvh_perf_smoke() {
        let mesh = grid_mesh(160);
        let rays = (0..4000)
            .map(|i| {
                let x = (i % 160) as f32 + 0.37;
                let y = ((i * 37) % 160) as f32 + 0.61;
                (Vec3::new(x, y, -3.0), Vec3::Z)
            })
            .collect::<Vec<_>>();
        assert!(mesh.raycast(Vec3::new(0.5, 0.5, -3.0), Vec3::Z).is_some());

        let bvh_start = std::time::Instant::now();
        let bvh_hits = rays
            .iter()
            .filter(|(origin, dir)| mesh.raycast(*origin, *dir).is_some())
            .count();
        let bvh_elapsed = bvh_start.elapsed();

        let linear_start = std::time::Instant::now();
        let linear_hits = rays
            .iter()
            .filter(|(origin, dir)| linear_raycast(&mesh, *origin, *dir).is_some())
            .count();
        let linear_elapsed = linear_start.elapsed();

        assert_eq!(bvh_hits, linear_hits);
        eprintln!(
            "raycast perf smoke: bvh={bvh_elapsed:?}, linear={linear_elapsed:?}, hits={bvh_hits}"
        );
    }

    #[test]
    fn uv_pick_returns_triangle_material_and_mesh() {
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        ];
        let uvs = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(0.45, 0.0),
            Vec2::new(0.0, 0.45),
            Vec2::new(1.0, 1.0),
        ];
        let normals = vec![Vec3::Z; positions.len()];
        let sub_meshes = vec![
            SubMesh {
                mesh_id: MeshId(7),
                start_index: 0,
                index_count: 3,
                material_index: 3,
                material_name: "A".to_owned(),
                wireframe_edges: Vec::new(),
            },
            SubMesh {
                mesh_id: MeshId(8),
                start_index: 3,
                index_count: 3,
                material_index: 4,
                material_name: "B".to_owned(),
                wireframe_edges: Vec::new(),
            },
        ];
        let mesh = MeshData::new(
            positions,
            uvs,
            normals,
            vec![[0, 1, 2], [1, 3, 2]],
            sub_meshes,
            vec![
                MeshObject {
                    id: MeshId(7),
                    name: "A".to_owned(),
                },
                MeshObject {
                    id: MeshId(8),
                    name: "B".to_owned(),
                },
            ],
            vec![MeshId(7), MeshId(8)],
        )
        .unwrap();

        let hit = mesh
            .pick_uv(Vec2::new(0.9, 0.9), Some(4))
            .expect("point should hit second UV triangle");

        assert_eq!(hit.triangle_index, 1);
        assert_eq!(hit.material_index.as_usize(), 4);
        assert_eq!(hit.mesh_id, MeshId(8));
        assert!(mesh.pick_uv(Vec2::new(0.9, 0.9), Some(3)).is_none());
    }

    #[test]
    fn uv_paint_boundary_edges_ignore_continuous_internal_triangle_edges() {
        let mesh = square_mesh_with_uvs(
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 1.0),
            ],
            0,
            0,
        );

        let edge_kinds = mesh.uv_paint_boundary_edges();
        assert_eq!(edge_kinds[0][2], UvPaintEdgeKind::Continuous);
        assert_eq!(edge_kinds[1][0], UvPaintEdgeKind::Continuous);

        let hit = mesh
            .raycast(Vec3::new(0.50, 0.49, -1.0), Vec3::Z)
            .expect("ray should hit the square");

        assert!(hit.uv_edge_distance < 0.02);
        assert_eq!(hit.uv_paint_boundary_distance, f32::INFINITY);
    }

    #[test]
    fn uv_paint_boundary_edges_detect_uv_discontinuities() {
        let mesh = MeshData::new(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.25, 0.25),
                Vec2::new(0.25, 0.75),
                Vec2::new(0.0, 1.0),
            ],
            vec![Vec3::Z; 6],
            vec![[0, 1, 2], [3, 4, 5]],
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count: 6,
                material_index: 0,
                material_name: "Material".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0), MeshId(0)],
        )
        .unwrap();

        let edge_kinds = mesh.uv_paint_boundary_edges();
        assert_eq!(edge_kinds[0][2], UvPaintEdgeKind::UvDiscontinuity);
        assert_eq!(edge_kinds[1][0], UvPaintEdgeKind::UvDiscontinuity);

        let hit = mesh
            .raycast(Vec3::new(0.50, 0.49, -1.0), Vec3::Z)
            .expect("ray should hit the square");

        assert!(hit.uv_paint_boundary_distance < 0.02);
    }

    #[test]
    fn uv_paint_boundary_edges_detect_material_boundaries() {
        let mesh = square_mesh_with_uvs(
            vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 1.0),
            ],
            0,
            1,
        );

        let edge_kinds = mesh.uv_paint_boundary_edges();
        assert_eq!(edge_kinds[0][2], UvPaintEdgeKind::MaterialBoundary);
        assert_eq!(edge_kinds[1][0], UvPaintEdgeKind::MaterialBoundary);
    }

    fn linear_raycast(mesh: &MeshData, ray_origin: Vec3, ray_dir: Vec3) -> Option<SurfaceHit> {
        let ray_dir = ray_dir.normalize_or_zero();
        if ray_dir.length_squared() <= f32::EPSILON {
            return None;
        }
        mesh.indices
            .iter()
            .enumerate()
            .filter_map(|(triangle_index, _)| {
                mesh.raycast_triangle(ray_origin, ray_dir, triangle_index)
            })
            .min_by(|a, b| a.t.total_cmp(&b.t))
    }

    fn square_mesh_with_uvs(
        uvs: Vec<Vec2>,
        first_material_index: usize,
        second_material_index: usize,
    ) -> MeshData {
        MeshData::new(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ],
            uvs,
            vec![Vec3::Z; 4],
            vec![[0, 1, 2], [0, 2, 3]],
            vec![
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 0,
                    index_count: 3,
                    material_index: first_material_index,
                    material_name: "First".to_owned(),
                    wireframe_edges: Vec::new(),
                },
                SubMesh {
                    mesh_id: MeshId(0),
                    start_index: 3,
                    index_count: 3,
                    material_index: second_material_index,
                    material_name: "Second".to_owned(),
                    wireframe_edges: Vec::new(),
                },
            ],
            vec![MeshObject {
                id: MeshId(0),
                name: "Mesh".to_owned(),
            }],
            vec![MeshId(0), MeshId(0)],
        )
        .unwrap()
    }

    fn grid_mesh(size: usize) -> MeshData {
        let vertex_size = size + 1;
        let mut positions = Vec::with_capacity(vertex_size * vertex_size);
        let mut uvs = Vec::with_capacity(vertex_size * vertex_size);
        for y in 0..=size {
            for x in 0..=size {
                positions.push(Vec3::new(x as f32, y as f32, 0.0));
                uvs.push(Vec2::new(x as f32 / size as f32, y as f32 / size as f32));
            }
        }

        let mut indices = Vec::with_capacity(size * size * 2);
        let mut triangle_mesh_ids = Vec::with_capacity(size * size * 2);
        for y in 0..size {
            for x in 0..size {
                let i0 = (y * vertex_size + x) as u32;
                let i1 = i0 + 1;
                let i2 = i0 + vertex_size as u32;
                let i3 = i2 + 1;
                indices.push([i0, i1, i2]);
                indices.push([i1, i3, i2]);
                triangle_mesh_ids.push(MeshId(0));
                triangle_mesh_ids.push(MeshId(0));
            }
        }

        let index_count = indices.len() as u32 * 3;
        MeshData::new(
            positions,
            uvs,
            vec![Vec3::Z; vertex_size * vertex_size],
            indices,
            vec![SubMesh {
                mesh_id: MeshId(0),
                start_index: 0,
                index_count,
                material_index: 5,
                material_name: "Grid".to_owned(),
                wireframe_edges: Vec::new(),
            }],
            vec![MeshObject {
                id: MeshId(0),
                name: "Grid".to_owned(),
            }],
            triangle_mesh_ids,
        )
        .unwrap()
    }
}

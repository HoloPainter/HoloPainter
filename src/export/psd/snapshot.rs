use std::{collections::HashMap, sync::Arc};

use anyhow::{Context, Result, bail, ensure};

use crate::core::{
    adjustment::{Adjustment, UvMirrorAdjustment},
    document::Document,
    document_tile_store::DocumentTileStore,
    embedded_image::{EmbeddedImageAsset, EmbeddedImageId},
    material::{MaterialData, MaterialIndex},
    surface::{LayerContent, LayerId, LayerTree, PaintSurfaceId},
};

use super::{
    model::{PsdExportDocument, PsdExportLayer, PsdExportLayerContent, PsdExportMask},
    pixels::{
        mirrored_mask_r8, mirrored_straight_rgba8, rasterize_embedded_image, unpremultiply_rgba8,
    },
};

#[derive(Debug, Clone)]
pub struct PsdExportSnapshot {
    pub document_generation: u64,
    materials: Vec<MaterialData>,
    layer_tree: LayerTree,
    tiles: DocumentTileStore,
    embedded_images: HashMap<EmbeddedImageId, Arc<EmbeddedImageAsset>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PsdExportWarning {
    EmbeddedImageRasterized { layer_name: String },
    GradientMapDitherMayDiffer { layer_name: String },
}

pub fn warnings_for_document(document: &Document) -> Vec<PsdExportWarning> {
    document
        .layer_tree
        .rows()
        .into_iter()
        .filter(|row| {
            document.materials.iter().any(|material| {
                document
                    .layer_tree
                    .effective_material_allowed(row.layer_id, material.id)
            })
        })
        .filter_map(|row| {
            let node = document.layer_tree.get(row.layer_id)?;
            match &node.content {
                LayerContent::EmbeddedImage { .. } => {
                    Some(PsdExportWarning::EmbeddedImageRasterized {
                        layer_name: node.props.name.clone(),
                    })
                }
                LayerContent::Adjustment {
                    adjustment: Adjustment::GradientMap(gradient),
                } if gradient.dither => Some(PsdExportWarning::GradientMapDitherMayDiffer {
                    layer_name: node.props.name.clone(),
                }),
                _ => None,
            }
        })
        .collect()
}

impl PsdExportSnapshot {
    pub fn capture(document: &Document, document_generation: u64) -> Self {
        Self {
            document_generation,
            materials: document.materials.clone(),
            layer_tree: document.layer_tree.clone(),
            tiles: document.tiles.clone(),
            embedded_images: document
                .embedded_images()
                .map(|asset| (asset.id, asset.clone()))
                .collect(),
        }
    }

    pub fn materials(&self) -> &[MaterialData] {
        &self.materials
    }

    pub fn warnings_for_material(&self, material_index: usize) -> Vec<PsdExportWarning> {
        let Some(material) = self.materials.get(material_index) else {
            return Vec::new();
        };
        self.layer_tree
            .rows()
            .into_iter()
            .filter(|row| {
                self.layer_tree
                    .effective_material_allowed(row.layer_id, material.id)
            })
            .filter_map(|row| {
                let node = self.layer_tree.get(row.layer_id)?;
                match &node.content {
                    LayerContent::EmbeddedImage { .. } => {
                        Some(PsdExportWarning::EmbeddedImageRasterized {
                            layer_name: node.props.name.clone(),
                        })
                    }
                    LayerContent::Adjustment {
                        adjustment: Adjustment::GradientMap(gradient),
                    } if gradient.dither => Some(PsdExportWarning::GradientMapDitherMayDiffer {
                        layer_name: node.props.name.clone(),
                    }),
                    _ => None,
                }
            })
            .collect()
    }

    pub fn build_material_document(
        &self,
        material_index: usize,
        mut composite_premultiplied_rgba8: Vec<u8>,
    ) -> Result<PsdExportDocument> {
        let material = self
            .materials
            .get(material_index)
            .with_context(|| format!("material index {material_index} is unavailable"))?;
        validate_rgba_len(&composite_premultiplied_rgba8, material.texture_size)?;
        unpremultiply_rgba8(&mut composite_premultiplied_rgba8);
        let root_children = self
            .layer_tree
            .children(self.layer_tree.root())
            .context("layer tree root is not a group")?;
        let layers = self.normalize_children(root_children, material_index, material.id, &[])?;
        Ok(PsdExportDocument {
            width: material.texture_size[0],
            height: material.texture_size[1],
            layers,
            composite_rgba8: composite_premultiplied_rgba8,
        })
    }

    fn normalize_children(
        &self,
        internal_bottom_to_top: &[LayerId],
        material_index: usize,
        material_id: crate::core::material::MaterialId,
        inherited_mirrors: &[UvMirrorAdjustment],
    ) -> Result<Vec<PsdExportLayer>> {
        let mut mirrors = inherited_mirrors.to_vec();
        let mut output = Vec::new();
        for &layer_id in internal_bottom_to_top.iter().rev() {
            let Some(node) = self.layer_tree.get(layer_id) else {
                bail!("layer tree references a missing child");
            };
            if !node.material_mask.allows(material_id) {
                continue;
            }
            if let LayerContent::Adjustment {
                adjustment: Adjustment::UvMirror(mirror),
            } = node.content
            {
                if node.props.visible {
                    mirrors.push(mirror);
                }
                continue;
            }
            output.push(self.normalize_layer(layer_id, material_index, material_id, &mirrors)?);
        }
        Ok(output)
    }

    fn normalize_layer(
        &self,
        layer_id: LayerId,
        material_index: usize,
        material_id: crate::core::material::MaterialId,
        mirrors: &[UvMirrorAdjustment],
    ) -> Result<PsdExportLayer> {
        let node = self
            .layer_tree
            .get(layer_id)
            .context("normalized layer is missing")?;
        let size = self.materials[material_index].texture_size;
        let mask = node
            .mask
            .map(|properties| -> Result<PsdExportMask> {
                let surface = PaintSurfaceId::layer_mask(MaterialIndex(material_index), layer_id);
                let snapshot = self
                    .tiles
                    .read_surface_full(surface)
                    .with_context(|| format!("reading layer mask for {:?}", layer_id))?;
                ensure!(snapshot.texture_size == size, "layer mask size mismatch");
                Ok(PsdExportMask {
                    enabled: properties.enabled,
                    r8: mirrored_mask_r8(&snapshot.rgba8, size, mirrors)?,
                })
            })
            .transpose()?;

        let content = match &node.content {
            LayerContent::Raster => {
                let surface = PaintSurfaceId::raster(MaterialIndex(material_index), layer_id);
                let snapshot = self
                    .tiles
                    .read_surface_full(surface)
                    .with_context(|| format!("reading raster layer {:?}", layer_id))?;
                ensure!(snapshot.texture_size == size, "raster layer size mismatch");
                PsdExportLayerContent::Raster {
                    rgba8: mirrored_straight_rgba8(&snapshot.rgba8, size, mirrors)?,
                }
            }
            LayerContent::EmbeddedImage {
                image_id,
                transform,
            } => {
                let asset = self
                    .embedded_images
                    .get(image_id)
                    .with_context(|| format!("embedded image {:?} is missing", image_id))?;
                PsdExportLayerContent::Raster {
                    rgba8: rasterize_embedded_image(asset, *transform, size, mirrors)?,
                }
            }
            LayerContent::SolidFill { color } => PsdExportLayerContent::SolidFill { color: *color },
            LayerContent::Adjustment { adjustment } => {
                ensure!(
                    !matches!(adjustment, Adjustment::UvMirror(_)),
                    "UV Mirror must be consumed during normalization"
                );
                PsdExportLayerContent::Adjustment {
                    adjustment: adjustment.clone(),
                }
            }
            LayerContent::Group {
                children,
                composite_mode,
            } => PsdExportLayerContent::Group {
                composite_mode: *composite_mode,
                children: self.normalize_children(
                    children,
                    material_index,
                    material_id,
                    mirrors,
                )?,
            },
        };
        Ok(PsdExportLayer {
            name: node.props.name.clone(),
            visible: node.props.visible,
            locked: node.props.locked,
            opacity: node.effective_opacity(),
            blend_mode: node.props.blend_mode,
            mask,
            content,
        })
    }
}

fn validate_rgba_len(rgba8: &[u8], size: [u32; 2]) -> Result<()> {
    let expected = usize::try_from(size[0])?
        .checked_mul(usize::try_from(size[1])?)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("RGBA byte length overflows")?;
    ensure!(
        rgba8.len() == expected,
        "RGBA byte length does not match material size"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::core::{
        adjustment::AdjustmentKind,
        document::{Document, MeshData},
        document_tile_store::InitialPixels,
        geometry::RectU32,
        material::MaterialSpec,
        surface::{LayerMaterialMask, PaintSurfaceId},
        tile_payload::PixelSnapshotData,
    };

    use super::*;

    #[test]
    fn material_filter_root_removal_order_and_uv_normalization_share_one_export_tree() {
        let mut document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [4, 1]),
                MaterialSpec::new("B", [4, 1]),
            ],
        );
        let raster = document.layer_tree.default_raster_layer().unwrap();
        document
            .layer_tree
            .rename_layer(raster, "Raster".to_owned());
        let mirror = document
            .layer_tree
            .add_adjustment_layer_above(
                raster,
                "UV Mirror",
                AdjustmentKind::UvMirror.default_adjustment(),
            )
            .unwrap();
        document.layer_tree.set_layer_material_mask(
            mirror,
            LayerMaterialMask::Specified(BTreeSet::from([document.materials[0].id])),
        );
        document
            .layer_tree
            .add_solid_fill_layer_above(mirror, "Top Fill", [0.25, 0.5, 0.75])
            .unwrap();

        let row = |values: [u8; 4]| {
            values
                .into_iter()
                .flat_map(|value| [value, 0, 0, 255])
                .collect::<Vec<_>>()
        };
        for (material_index, values) in [(0, [10, 20, 30, 40]), (1, [1, 2, 3, 4])] {
            document
                .tiles
                .write_surface_rect(
                    PaintSurfaceId::raster(MaterialIndex(material_index), raster),
                    RectU32::full([4, 1]),
                    &PixelSnapshotData::contiguous(row(values)),
                )
                .unwrap();
        }

        let snapshot = PsdExportSnapshot::capture(&document, 7);
        let export_a = snapshot.build_material_document(0, vec![0; 16]).unwrap();
        let export_b = snapshot.build_material_document(1, vec![0; 16]).unwrap();

        assert_eq!(snapshot.document_generation, 7);
        assert_eq!(
            export_a
                .layers
                .iter()
                .map(|layer| layer.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Top Fill", "Raster"]
        );
        let PsdExportLayerContent::Raster { rgba8 } = &export_a.layers[1].content else {
            panic!("bottom layer must remain raster");
        };
        assert_eq!(
            rgba8
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            vec![40, 30, 30, 40]
        );
        let PsdExportLayerContent::Raster { rgba8 } = &export_b.layers[1].content else {
            panic!("bottom layer must remain raster");
        };
        assert_eq!(
            rgba8
                .chunks_exact(4)
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn uv_mirror_applies_to_layer_mask_while_preserving_disabled_state() {
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [4, 1])]);
        let raster = document.layer_tree.default_raster_layer().unwrap();
        assert!(document.layer_tree.add_layer_mask(raster));
        document.layer_tree.set_layer_mask_enabled(raster, false);
        let mask_surface = PaintSurfaceId::layer_mask(MaterialIndex(0), raster);
        document
            .tiles
            .create_surface(
                mask_surface,
                [4, 1],
                InitialPixels::Rgba8(
                    [10_u8, 20, 30, 40]
                        .into_iter()
                        .flat_map(|value| [value, 0, 0, 255])
                        .collect(),
                ),
            )
            .unwrap();
        let mirror = document
            .layer_tree
            .add_adjustment_layer_above(
                raster,
                "UV Mirror",
                AdjustmentKind::UvMirror.default_adjustment(),
            )
            .unwrap();
        document.layer_tree.set_layer_material_mask(
            mirror,
            LayerMaterialMask::Specified(BTreeSet::from([document.materials[0].id])),
        );

        let export = PsdExportSnapshot::capture(&document, 1)
            .build_material_document(0, vec![0; 16])
            .unwrap();
        let mask = export.layers[0].mask.as_ref().unwrap();
        assert!(!mask.enabled);
        assert_eq!(mask.r8, vec![40, 30, 30, 40]);
    }

    #[test]
    fn uv_mirror_applies_to_adjustment_layer_mask_without_rasterizing_adjustment() {
        let mut document = Document::new(MeshData::empty(), vec![MaterialSpec::new("A", [4, 1])]);
        let raster = document.layer_tree.default_raster_layer().unwrap();
        let adjustment = document
            .layer_tree
            .add_adjustment_layer_above(
                raster,
                "Levels",
                AdjustmentKind::Levels.default_adjustment(),
            )
            .unwrap();
        assert!(document.layer_tree.add_layer_mask(adjustment));
        document
            .tiles
            .create_surface(
                PaintSurfaceId::layer_mask(MaterialIndex(0), adjustment),
                [4, 1],
                InitialPixels::Rgba8(
                    [10_u8, 20, 30, 40]
                        .into_iter()
                        .flat_map(|value| [value, 0, 0, 255])
                        .collect(),
                ),
            )
            .unwrap();
        let mirror = document
            .layer_tree
            .add_adjustment_layer_above(
                adjustment,
                "UV Mirror",
                AdjustmentKind::UvMirror.default_adjustment(),
            )
            .unwrap();
        document.layer_tree.set_layer_material_mask(
            mirror,
            LayerMaterialMask::Specified(BTreeSet::from([document.materials[0].id])),
        );

        let export = PsdExportSnapshot::capture(&document, 1)
            .build_material_document(0, vec![0; 16])
            .unwrap();
        assert!(matches!(
            export.layers[0].content,
            PsdExportLayerContent::Adjustment {
                adjustment: Adjustment::Levels(_)
            }
        ));
        assert_eq!(
            export.layers[0].mask.as_ref().unwrap().r8,
            vec![40, 30, 30, 40]
        );
    }
}

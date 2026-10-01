use std::collections::HashMap;

use anyhow::{Result, anyhow};

use crate::core::{
    geometry::RectU32,
    material::{MaterialData, MaterialIndex},
};

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveSelection {
    pub enabled: bool,
    pub visible: bool,
    pub masks: Vec<MaterialSelectionMask>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MaterialSelectionMask {
    pub material_index: MaterialIndex,
    pub mask_id: Option<SelectionMaskId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SelectionMaskId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionTilePayload {
    pub material_index: MaterialIndex,
    pub rect: RectU32,
    pub r8: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionMaskSnapshot {
    pub texture_size: [u32; 2],
    pub r8: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionMaskTileStore {
    masks: HashMap<MaterialIndex, SelectionMaskPixels>,
    revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionMaskPixels {
    texture_size: [u32; 2],
    r8: Vec<u8>,
}

impl ActiveSelection {
    pub fn disabled_for_materials(materials: impl IntoIterator<Item = MaterialIndex>) -> Self {
        Self {
            enabled: false,
            visible: true,
            masks: materials
                .into_iter()
                .map(|material_index| MaterialSelectionMask {
                    material_index,
                    mask_id: None,
                })
                .collect(),
        }
    }

    pub fn is_effectively_full(&self) -> bool {
        !self.enabled
    }

    pub fn is_active(&self) -> bool {
        self.enabled && self.has_any_mask()
    }

    pub fn is_visible_active(&self) -> bool {
        self.visible && self.is_active()
    }

    pub fn has_any_mask(&self) -> bool {
        self.masks.iter().any(|mask| mask.mask_id.is_some())
    }

    pub fn disabled_preserving_masks(&self) -> Self {
        // Keep mask ids so undo/redo and the next additive selection can reuse cleared GPU textures.
        let mut selection = self.clone();
        selection.enabled = false;
        selection.visible = true;
        selection
    }

    pub fn enable_material_mask(&mut self, material_index: MaterialIndex) -> SelectionMaskId {
        let mask_id = selection_mask_id_for_material(material_index);
        self.enabled = true;
        if let Some(mask) = self
            .masks
            .iter_mut()
            .find(|mask| mask.material_index == material_index)
        {
            mask.mask_id = Some(mask_id);
        } else {
            self.masks.push(MaterialSelectionMask {
                material_index,
                mask_id: Some(mask_id),
            });
        }
        mask_id
    }

    pub fn material_mask(&self, material_index: MaterialIndex) -> Option<&MaterialSelectionMask> {
        self.masks
            .iter()
            .find(|mask| mask.material_index == material_index)
    }
}

impl SelectionMaskTileStore {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn snapshot(&self, material_index: MaterialIndex) -> Option<SelectionMaskSnapshot> {
        self.masks
            .get(&material_index)
            .map(|mask| SelectionMaskSnapshot {
                texture_size: mask.texture_size,
                r8: mask.r8.clone(),
            })
    }

    pub fn restore_snapshot(
        &mut self,
        material_index: MaterialIndex,
        snapshot: Option<&SelectionMaskSnapshot>,
    ) -> Result<()> {
        match snapshot {
            Some(snapshot) => {
                let expected_len = selection_mask_len(snapshot.texture_size)?;
                if snapshot.r8.len() != expected_len {
                    return Err(anyhow!(
                        "invalid selection mask snapshot payload size for material {}: got {}, expected {}",
                        material_index,
                        snapshot.r8.len(),
                        expected_len
                    ));
                }
                self.masks.insert(
                    material_index,
                    SelectionMaskPixels {
                        texture_size: snapshot.texture_size,
                        r8: snapshot.r8.clone(),
                    },
                );
            }
            None => {
                self.masks.remove(&material_index);
            }
        }
        self.bump_revision();
        Ok(())
    }

    pub fn mask_bytes_or_zeros(
        &self,
        material_index: MaterialIndex,
        texture_size: [u32; 2],
    ) -> Result<Vec<u8>> {
        let len = selection_mask_len(texture_size)?;
        let Some(mask) = self.masks.get(&material_index) else {
            return Ok(vec![0; len]);
        };
        if mask.texture_size != texture_size {
            return Err(anyhow!(
                "selection mask size mismatch for material {}: stored {:?}, requested {:?}",
                material_index,
                mask.texture_size,
                texture_size
            ));
        }
        if mask.r8.len() != len {
            return Err(anyhow!(
                "selection mask payload size mismatch for material {}: got {}, expected {}",
                material_index,
                mask.r8.len(),
                len
            ));
        }
        Ok(mask.r8.clone())
    }

    pub fn write_mask(
        &mut self,
        material_index: MaterialIndex,
        texture_size: [u32; 2],
        r8: Vec<u8>,
    ) -> Result<()> {
        let expected_len = selection_mask_len(texture_size)?;
        if r8.len() != expected_len {
            return Err(anyhow!(
                "invalid selection mask payload size for material {}: got {}, expected {}",
                material_index,
                r8.len(),
                expected_len
            ));
        }
        self.masks
            .insert(material_index, SelectionMaskPixels { texture_size, r8 });
        self.bump_revision();
        Ok(())
    }

    pub fn patch_tiles(
        &mut self,
        materials: &[MaterialData],
        tiles: &[SelectionTilePayload],
    ) -> Result<()> {
        for tile in tiles {
            let texture_size = materials
                .get(tile.material_index.as_usize())
                .ok_or_else(|| {
                    anyhow!("selection material does not exist: {}", tile.material_index)
                })?
                .texture_size;
            let mask = self.ensure_mask(tile.material_index, texture_size)?;
            patch_r8_rect(&mut mask.r8, texture_size, tile.rect, &tile.r8)?;
            self.bump_revision();
        }
        Ok(())
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    fn ensure_mask(
        &mut self,
        material_index: MaterialIndex,
        texture_size: [u32; 2],
    ) -> Result<&mut SelectionMaskPixels> {
        let recreate = self
            .masks
            .get(&material_index)
            .is_none_or(|mask| mask.texture_size != texture_size);
        if recreate {
            self.masks.insert(
                material_index,
                SelectionMaskPixels {
                    texture_size,
                    r8: vec![0; selection_mask_len(texture_size)?],
                },
            );
        }
        Ok(self
            .masks
            .get_mut(&material_index)
            .expect("selection mask was just ensured"))
    }
}

pub fn selection_mask_id_for_material(material_index: MaterialIndex) -> SelectionMaskId {
    SelectionMaskId(0x51E1_EC70_0000_0000u64 | material_index.as_usize() as u64)
}

pub fn selection_mask_len(texture_size: [u32; 2]) -> Result<usize> {
    let width =
        usize::try_from(texture_size[0]).map_err(|_| anyhow!("selection width overflows"))?;
    let height =
        usize::try_from(texture_size[1]).map_err(|_| anyhow!("selection height overflows"))?;
    width
        .checked_mul(height)
        .ok_or_else(|| anyhow!("selection mask byte length overflows"))
}

fn patch_r8_rect(dst: &mut [u8], texture_size: [u32; 2], rect: RectU32, r8: &[u8]) -> Result<()> {
    validate_r8_rect(texture_size, rect)?;
    let expected = (rect.size[0] as usize).saturating_mul(rect.size[1] as usize);
    if r8.len() != expected {
        return Err(anyhow!("selection tile payload size mismatch"));
    }
    let width = texture_size[0] as usize;
    for row in 0..rect.size[1] as usize {
        let dst_start = (rect.origin[1] as usize + row) * width + rect.origin[0] as usize;
        let src_start = row * rect.size[0] as usize;
        dst[dst_start..dst_start + rect.size[0] as usize]
            .copy_from_slice(&r8[src_start..src_start + rect.size[0] as usize]);
    }
    Ok(())
}

fn validate_r8_rect(texture_size: [u32; 2], rect: RectU32) -> Result<()> {
    if rect.size[0] == 0 || rect.size[1] == 0 {
        return Err(anyhow!("selection rect is empty"));
    }
    let end_x = rect.origin[0]
        .checked_add(rect.size[0])
        .ok_or_else(|| anyhow!("selection rect x range overflows"))?;
    let end_y = rect.origin[1]
        .checked_add(rect.size[1])
        .ok_or_else(|| anyhow!("selection rect y range overflows"))?;
    if end_x > texture_size[0] || end_y > texture_size[1] {
        return Err(anyhow!("selection rect is outside texture bounds"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::{Document, MaterialSpec, MeshData};

    #[test]
    fn tile_patch_uses_material_size_as_the_allocation_authority() {
        let document = Document::new(
            MeshData::empty(),
            vec![
                MaterialSpec::new("A", [2, 2]),
                MaterialSpec::new("B", [1, 3]),
            ],
        );
        let mut store = SelectionMaskTileStore::default();

        store
            .patch_tiles(
                &document.materials,
                &[SelectionTilePayload {
                    material_index: 1.into(),
                    rect: RectU32::full([1, 3]),
                    r8: vec![1, 2, 3],
                }],
            )
            .unwrap();

        assert_eq!(
            store.mask_bytes_or_zeros(1.into(), [1, 3]).unwrap(),
            vec![1, 2, 3]
        );
        assert!(
            store
                .patch_tiles(
                    &document.materials,
                    &[SelectionTilePayload {
                        material_index: 1.into(),
                        rect: RectU32::full([2, 2]),
                        r8: vec![0; 4],
                    }],
                )
                .is_err()
        );
    }
}

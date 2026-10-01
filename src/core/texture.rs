use std::collections::HashMap;

use anyhow::{Result, ensure};

use crate::core::{
    brush_engine::TextureResourceFormat,
    image_asset::{
        ImageAssetCatalog, ImageAssetDefinition, ImageAssetOrigin, ImageRepresentation,
        ImageRepresentationFormat,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureResourceDefinition {
    pub id: String,
    pub display_name: String,
    pub(crate) source_asset_id: String,
    pub(crate) source_asset_origin: ImageAssetOrigin,
    pub format: TextureResourceFormat,
    pub size: [u32; 2],
    pub r8: Vec<u8>,
    pub mipmaps: bool,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TextureCatalog {
    textures: HashMap<String, TextureResourceDefinition>,
}

impl TextureCatalog {
    pub fn from_image_assets(assets: &ImageAssetCatalog) -> Result<Self> {
        let mut catalog = Self::default();
        for asset in assets.assets() {
            for representation in &asset.representations {
                if representation.format != ImageRepresentationFormat::R8Unorm {
                    continue;
                }
                ensure!(
                    !catalog.textures.contains_key(&representation.id),
                    "duplicate texture resource id {:?}",
                    representation.id
                );
                catalog.textures.insert(
                    representation.id.clone(),
                    TextureResourceDefinition::from_image_representation(
                        asset,
                        assets
                            .origin(&asset.id)
                            .expect("catalog assets must retain their origin"),
                        representation,
                    )?,
                );
            }
        }
        Ok(catalog)
    }

    pub fn get(&self, id: &str) -> Option<&TextureResourceDefinition> {
        self.textures.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.textures.contains_key(id)
    }

    pub fn textures(&self) -> impl Iterator<Item = &TextureResourceDefinition> {
        self.textures.values()
    }

    pub(crate) fn insert(&mut self, definition: TextureResourceDefinition) -> Result<()> {
        ensure!(
            !self.textures.contains_key(&definition.id),
            "duplicate texture resource id {:?}",
            definition.id
        );
        self.textures.insert(definition.id.clone(), definition);
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: &str) -> Option<TextureResourceDefinition> {
        self.textures.remove(id)
    }

    pub(crate) fn set_display_name(&mut self, id: &str, display_name: &str) -> bool {
        let Some(texture) = self.textures.get_mut(id) else {
            return false;
        };
        texture.display_name = display_name.to_owned();
        true
    }
}

impl TextureResourceDefinition {
    pub(crate) fn from_image_representation(
        asset: &ImageAssetDefinition,
        source_asset_origin: ImageAssetOrigin,
        representation: &ImageRepresentation,
    ) -> Result<Self> {
        ensure!(
            representation.format == ImageRepresentationFormat::R8Unorm,
            "image representation {:?} is not an R8 texture resource",
            representation.id
        );
        Ok(Self {
            id: representation.id.clone(),
            display_name: asset.display_name.clone(),
            source_asset_id: asset.id.clone(),
            source_asset_origin,
            format: TextureResourceFormat::R8Unorm,
            size: representation.size,
            r8: representation
                .r8()
                .expect("R8 image representation must expose R8 bytes")
                .to_vec(),
            mipmaps: representation.mipmaps,
            tags: representation.tags.clone(),
        })
    }
}

pub fn texture_tags_match(texture: &TextureResourceDefinition, required_tags: &[String]) -> bool {
    required_tags
        .iter()
        .all(|required| texture.tags.iter().any(|tag| tag == required))
}

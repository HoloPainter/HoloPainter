use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use image::{ExtendedColorType, ImageEncoder, ImageFormat};
use serde::{Deserialize, Serialize};

use crate::embedded_resources;

pub const IMAGE_ASSET_SCHEMA_VERSION: u32 = 1;
pub const IMAGE_ASSET_BRUSH_TIP_TAG: &str = "brush_tip_mask";
pub const IMAGE_ASSET_DECAL_TAG: &str = "decal_image";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageAssetOrigin {
    Builtin,
    UserOverride,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageRepresentationFormat {
    R8Unorm,
    Rgba8UnormSrgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageMaskSource {
    Alpha,
    Luminance,
    Red,
    Green,
    Blue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMaskRecipe {
    pub source: ImageMaskSource,
    #[serde(default)]
    pub invert: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRepresentation {
    pub id: String,
    pub format: ImageRepresentationFormat,
    pub size: [u32; 2],
    pub bytes: Arc<[u8]>,
    pub mipmaps: bool,
    pub tags: Vec<String>,
    pub mask_recipe: Option<ImageMaskRecipe>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAssetDefinition {
    pub id: String,
    pub display_name: String,
    pub tags: Vec<String>,
    pub representations: Vec<ImageRepresentation>,
}

impl ImageAssetDefinition {
    pub fn representation_with_tag(&self, required_tag: &str) -> Option<&ImageRepresentation> {
        self.representations
            .iter()
            .find(|representation| representation.tags.iter().any(|tag| tag == required_tag))
    }

    pub fn representation(&self, id: &str) -> Option<&ImageRepresentation> {
        self.representations
            .iter()
            .find(|representation| representation.id == id)
    }

    pub fn rgba8_representation(&self) -> Option<&ImageRepresentation> {
        self.representations.iter().find(|representation| {
            representation.format == ImageRepresentationFormat::Rgba8UnormSrgb
        })
    }
}

#[derive(Debug, Clone)]
struct ImageAssetRecord {
    definition: ImageAssetDefinition,
    file: ImageAssetFileV1,
    file_path: Option<PathBuf>,
    origin: ImageAssetOrigin,
}

#[derive(Debug, Clone, Default)]
pub struct ImageAssetCatalog {
    assets: Vec<Option<ImageAssetRecord>>,
    by_id: HashMap<String, usize>,
    user_root: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageAssetFileV1 {
    schema_version: u32,
    id: String,
    display_name: String,
    #[serde(default)]
    tags: Vec<String>,
    representations: Vec<ImageRepresentationFileV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageRepresentationFileV1 {
    id: String,
    format: ImageRepresentationFormat,
    source: ImageSourceFileV1,
    #[serde(default)]
    mipmaps: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    mask_recipe: Option<ImageMaskRecipe>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum ImageSourceFileV1 {
    File {
        path: String,
        decode: ImageFileDecodeV1,
    },
    SolidR8 {
        width: u32,
        height: u32,
        value: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum ImageFileDecodeV1 {
    RawR8 { width: u32, height: u32 },
    PngRgba8,
}

impl ImageAssetCatalog {
    pub fn load_effective(user_path: Option<&Path>) -> Result<Self> {
        let mut catalog = Self {
            user_root: user_path.map(Path::to_path_buf),
            ..Self::default()
        };
        for resource in embedded_resources::files_under("images")
            .filter(|file| file.path.ends_with(".image.ron"))
        {
            let (file, definition) = parse_image_asset_file(
                resource.bytes,
                &format!("embedded resource {:?}", resource.path),
                &|path| read_embedded_image_source(resource.path, path, embedded_resources::bytes),
            )?;
            ensure!(
                !catalog.by_id.contains_key(&definition.id),
                "duplicate image asset id {:?}",
                definition.id
            );
            let index = catalog.assets.len();
            catalog.by_id.insert(definition.id.clone(), index);
            catalog.assets.push(Some(ImageAssetRecord {
                definition,
                file,
                file_path: None,
                origin: ImageAssetOrigin::Builtin,
            }));
        }

        if let Some(user_path) = user_path
            && user_path.exists()
        {
            for file_path in image_asset_files_in(user_path)? {
                let (file, definition) = match load_image_asset_file(&file_path) {
                    Ok(asset) => asset,
                    Err(error) => {
                        eprintln!(
                            "Ignoring invalid user image asset {}: {error:#}",
                            file_path.display()
                        );
                        continue;
                    }
                };
                if let Some(index) = catalog.by_id.get(&definition.id).copied() {
                    let record = catalog.assets[index]
                        .as_mut()
                        .expect("image asset index must reference a record");
                    record.definition = definition;
                    record.file = file;
                    record.file_path = Some(file_path);
                    record.origin = ImageAssetOrigin::UserOverride;
                } else {
                    let index = catalog.assets.len();
                    catalog.by_id.insert(definition.id.clone(), index);
                    catalog.assets.push(Some(ImageAssetRecord {
                        definition,
                        file,
                        file_path: Some(file_path),
                        origin: ImageAssetOrigin::User,
                    }));
                }
            }
        }

        Ok(catalog)
    }

    pub fn get(&self, id: &str) -> Option<&ImageAssetDefinition> {
        let index = *self.by_id.get(id)?;
        self.assets
            .get(index)
            .and_then(Option::as_ref)
            .map(|record| &record.definition)
    }

    pub(crate) fn origin(&self, id: &str) -> Option<ImageAssetOrigin> {
        let index = *self.by_id.get(id)?;
        self.assets
            .get(index)
            .and_then(Option::as_ref)
            .map(|record| record.origin)
    }

    pub fn assets(&self) -> impl Iterator<Item = &ImageAssetDefinition> {
        self.assets
            .iter()
            .filter_map(Option::as_ref)
            .map(|record| &record.definition)
    }

    pub fn assets_with_representation_tag<'a>(
        &'a self,
        required_tag: &'a str,
    ) -> impl Iterator<Item = &'a ImageAssetDefinition> + 'a {
        self.assets().filter(move |asset| {
            asset
                .representations
                .iter()
                .any(|representation| representation.tags.iter().any(|tag| tag == required_tag))
        })
    }

    pub(crate) fn import_image(
        &mut self,
        display_name: &str,
        size: [u32; 2],
        rgba8: Arc<[u8]>,
        representation_tags: Vec<String>,
    ) -> Result<String> {
        validate_size("imported image", size[0], size[1])?;
        ensure!(
            rgba8.len() == size[0] as usize * size[1] as usize * 4,
            "imported image RGBA8 pixel length does not match dimensions"
        );
        let mut encoded = Vec::new();
        image::codecs::png::PngEncoder::new(&mut encoded)
            .write_image(&rgba8, size[0], size[1], ExtendedColorType::Rgba8)
            .context("encoding imported image as PNG")?;
        self.import_png_bytes(display_name, size, rgba8, representation_tags, &encoded)
    }

    pub(crate) fn import_png_bytes(
        &mut self,
        display_name: &str,
        size: [u32; 2],
        rgba8: Arc<[u8]>,
        representation_tags: Vec<String>,
        encoded_png: &[u8],
    ) -> Result<String> {
        let user_root = self
            .user_root
            .clone()
            .ok_or_else(|| anyhow::anyhow!("user image asset directory is unavailable"))?;
        validate_size("imported image", size[0], size[1])?;
        ensure!(
            rgba8.len() == size[0] as usize * size[1] as usize * 4,
            "imported image RGBA8 pixel length does not match dimensions"
        );
        validate_tags(&representation_tags, "image representation")?;
        let display_name = display_name.trim();
        ensure!(
            !display_name.is_empty(),
            "image display_name must not be empty"
        );

        let uuid = uuid::Uuid::new_v4().to_string();
        let asset_id = format!("image.user.{uuid}");
        let representation_id = format!("image.user.{uuid}.rgba");
        let asset_dir = user_root.join(&uuid);
        ensure!(
            !asset_dir.exists(),
            "generated image asset directory already exists"
        );
        fs::create_dir_all(&asset_dir)
            .with_context(|| format!("creating image asset directory {}", asset_dir.display()))?;

        let result = (|| {
            let source_name = "source.png";
            fs::write(asset_dir.join(source_name), encoded_png).with_context(|| {
                format!(
                    "writing imported image {}",
                    asset_dir.join(source_name).display()
                )
            })?;
            let file = ImageAssetFileV1 {
                schema_version: IMAGE_ASSET_SCHEMA_VERSION,
                id: asset_id.clone(),
                display_name: display_name.to_owned(),
                tags: Vec::new(),
                representations: vec![ImageRepresentationFileV1 {
                    id: representation_id.clone(),
                    format: ImageRepresentationFormat::Rgba8UnormSrgb,
                    source: ImageSourceFileV1::File {
                        path: source_name.to_owned(),
                        decode: ImageFileDecodeV1::PngRgba8,
                    },
                    mipmaps: false,
                    tags: representation_tags,
                    mask_recipe: None,
                }],
            };
            let file_path = asset_dir.join("asset.image.ron");
            write_asset_file(&file_path, &file)?;
            let definition = ImageAssetDefinition {
                id: asset_id.clone(),
                display_name: display_name.to_owned(),
                tags: Vec::new(),
                representations: vec![ImageRepresentation {
                    id: representation_id,
                    format: ImageRepresentationFormat::Rgba8UnormSrgb,
                    size,
                    bytes: rgba8,
                    mipmaps: false,
                    tags: file.representations[0].tags.clone(),
                    mask_recipe: None,
                }],
            };
            self.insert_user_record(file, definition, file_path)?;
            Ok(asset_id.clone())
        })();

        if result.is_err() {
            let _ = fs::remove_dir_all(&asset_dir);
        }
        result
    }

    pub(crate) fn add_r8_representation(
        &mut self,
        asset_id: &str,
        bytes: Vec<u8>,
        size: [u32; 2],
        tags: Vec<String>,
        mask_recipe: ImageMaskRecipe,
    ) -> Result<String> {
        validate_size(asset_id, size[0], size[1])?;
        ensure!(
            bytes.len() == size[0] as usize * size[1] as usize,
            "image asset {:?} R8 pixel length does not match dimensions",
            asset_id
        );
        validate_tags(&tags, "image representation")?;
        let index = *self
            .by_id
            .get(asset_id)
            .ok_or_else(|| anyhow::anyhow!("unknown image asset {:?}", asset_id))?;
        let record = self.assets[index]
            .as_mut()
            .expect("image asset index must reference a record");
        ensure!(
            matches!(record.origin, ImageAssetOrigin::User),
            "only user image assets can be modified"
        );
        ensure!(
            !record
                .definition
                .representations
                .iter()
                .any(|representation| representation.tags.iter().any(|tag| tags.contains(tag))),
            "image asset {:?} already has a representation for one of the requested tags",
            asset_id
        );

        let asset_uuid = asset_id
            .strip_prefix("image.user.")
            .ok_or_else(|| anyhow::anyhow!("user image asset id has unexpected form"))?;
        let representation_uuid = uuid::Uuid::new_v4();
        let representation_id = format!("texture.user.{asset_uuid}.{representation_uuid}");
        ensure!(
            !record
                .definition
                .representations
                .iter()
                .any(|representation| representation.id == representation_id),
            "duplicate image representation id {:?}",
            representation_id
        );
        let asset_dir = user_file_path(record)?
            .parent()
            .ok_or_else(|| anyhow::anyhow!("image asset file has no parent directory"))?;
        let mask_name = format!("{representation_uuid}.r8");
        let mask_path = asset_dir.join(&mask_name);
        fs::write(&mask_path, &bytes)
            .with_context(|| format!("writing image representation {}", mask_path.display()))?;

        let representation_file = ImageRepresentationFileV1 {
            id: representation_id.clone(),
            format: ImageRepresentationFormat::R8Unorm,
            source: ImageSourceFileV1::File {
                path: mask_name,
                decode: ImageFileDecodeV1::RawR8 {
                    width: size[0],
                    height: size[1],
                },
            },
            mipmaps: true,
            tags: tags.clone(),
            mask_recipe: Some(mask_recipe.clone()),
        };
        record.file.representations.push(representation_file);
        if let Err(error) = write_asset_file(user_file_path(record)?, &record.file) {
            record.file.representations.pop();
            let _ = fs::remove_file(mask_path);
            return Err(error);
        }
        record.definition.representations.push(ImageRepresentation {
            id: representation_id.clone(),
            format: ImageRepresentationFormat::R8Unorm,
            size,
            bytes: Arc::from(bytes),
            mipmaps: true,
            tags,
            mask_recipe: Some(mask_recipe),
        });
        Ok(representation_id)
    }

    pub(crate) fn add_r8_representation_from_rgba(
        &mut self,
        asset_id: &str,
        tags: Vec<String>,
        mask_recipe: ImageMaskRecipe,
    ) -> Result<String> {
        let (size, rgba8) = {
            let asset = self
                .get(asset_id)
                .ok_or_else(|| anyhow::anyhow!("unknown image asset {:?}", asset_id))?;
            let representation = asset.rgba8_representation().ok_or_else(|| {
                anyhow::anyhow!("image asset {:?} has no RGBA representation", asset_id)
            })?;
            (
                representation.size,
                representation
                    .rgba8()
                    .expect("RGBA representation must expose RGBA bytes")
                    .to_vec(),
            )
        };
        let bytes = build_r8_mask(&rgba8, size, &mask_recipe)?;
        self.add_r8_representation(asset_id, bytes, size, tags, mask_recipe)
    }

    pub(crate) fn set_user_rgba_tag(
        &mut self,
        asset_id: &str,
        tag: &str,
        enabled: bool,
    ) -> Result<()> {
        ensure!(
            !tag.trim().is_empty(),
            "image representation tag must not be empty"
        );
        let record = self.user_record_mut(asset_id)?;
        let file_index = record
            .file
            .representations
            .iter()
            .position(|representation| {
                representation.format == ImageRepresentationFormat::Rgba8UnormSrgb
            })
            .ok_or_else(|| {
                anyhow::anyhow!("image asset {:?} has no RGBA representation", asset_id)
            })?;
        let definition_index = record
            .definition
            .representations
            .iter()
            .position(|representation| {
                representation.format == ImageRepresentationFormat::Rgba8UnormSrgb
            })
            .expect("resolved image asset must mirror file representations");

        let old_tags = record.file.representations[file_index].tags.clone();
        let tags = &mut record.file.representations[file_index].tags;
        if enabled {
            if !tags.iter().any(|existing| existing == tag) {
                tags.push(tag.to_owned());
            }
        } else {
            tags.retain(|existing| existing != tag);
        }
        validate_tags(tags, "image representation")?;
        if let Err(error) = write_asset_file(user_file_path(record)?, &record.file) {
            record.file.representations[file_index].tags = old_tags;
            return Err(error);
        }
        record.definition.representations[definition_index].tags =
            record.file.representations[file_index].tags.clone();
        Ok(())
    }

    pub(crate) fn remove_user_r8_representation(
        &mut self,
        asset_id: &str,
        representation_id: &str,
    ) -> Result<ImageRepresentation> {
        let record = self.user_record_mut(asset_id)?;
        ensure!(
            record.file.representations.len() > 1,
            "image asset {:?} must keep at least one representation",
            asset_id
        );
        let file_index = record
            .file
            .representations
            .iter()
            .position(|representation| representation.id == representation_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "image asset {:?} has no representation {:?}",
                    asset_id,
                    representation_id
                )
            })?;
        let definition_index = record
            .definition
            .representations
            .iter()
            .position(|representation| representation.id == representation_id)
            .expect("resolved image asset must mirror file representations");
        ensure!(
            record.file.representations[file_index].format == ImageRepresentationFormat::R8Unorm,
            "only user R8 image representations can be removed"
        );

        let removed_file = record.file.representations.remove(file_index);
        let removed_definition = record.definition.representations.remove(definition_index);
        if let Err(error) = write_asset_file(user_file_path(record)?, &record.file) {
            record.file.representations.insert(file_index, removed_file);
            record
                .definition
                .representations
                .insert(definition_index, removed_definition);
            return Err(error);
        }

        if let ImageSourceFileV1::File { path, .. } = &removed_file.source
            && Path::new(path).components().count() == 1
            && !record.file.representations.iter().any(|representation| {
                matches!(
                    &representation.source,
                    ImageSourceFileV1::File { path: other, .. } if other == path
                )
            })
            && let Some(asset_dir) = user_file_path(record)?.parent()
        {
            let _ = fs::remove_file(asset_dir.join(path));
        }

        Ok(removed_definition)
    }

    pub(crate) fn rename_user(&mut self, asset_id: &str, display_name: &str) -> Result<()> {
        let display_name = display_name.trim();
        ensure!(
            !display_name.is_empty(),
            "image display_name must not be empty"
        );
        let record = self.user_record_mut(asset_id)?;
        let old = record.file.display_name.clone();
        record.file.display_name = display_name.to_owned();
        if let Err(error) = write_asset_file(user_file_path(record)?, &record.file) {
            record.file.display_name = old;
            return Err(error);
        }
        record.definition.display_name = display_name.to_owned();
        Ok(())
    }

    pub(crate) fn set_user_tags(&mut self, asset_id: &str, tags: Vec<String>) -> Result<()> {
        validate_tags(&tags, "image asset")?;
        let record = self.user_record_mut(asset_id)?;
        let old = std::mem::replace(&mut record.file.tags, tags.clone());
        if let Err(error) = write_asset_file(user_file_path(record)?, &record.file) {
            record.file.tags = old;
            return Err(error);
        }
        record.definition.tags = tags;
        Ok(())
    }

    pub(crate) fn remove_user(&mut self, asset_id: &str) -> Result<()> {
        let index = *self
            .by_id
            .get(asset_id)
            .ok_or_else(|| anyhow::anyhow!("unknown image asset {:?}", asset_id))?;
        let record = self.assets[index]
            .as_ref()
            .expect("image asset index must reference a record");
        ensure!(
            matches!(record.origin, ImageAssetOrigin::User),
            "only user image assets can be removed"
        );
        let user_root = self
            .user_root
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("user image asset directory is unavailable"))?;
        let asset_dir = user_file_path(record)?
            .parent()
            .ok_or_else(|| anyhow::anyhow!("image asset file has no parent directory"))?;
        ensure!(
            asset_dir.parent() == Some(user_root),
            "refusing to remove unexpected image asset directory {}",
            asset_dir.display()
        );
        fs::remove_dir_all(asset_dir)
            .with_context(|| format!("removing image asset directory {}", asset_dir.display()))?;
        self.assets[index] = None;
        self.by_id.remove(asset_id);
        Ok(())
    }

    fn user_record_mut(&mut self, asset_id: &str) -> Result<&mut ImageAssetRecord> {
        let index = *self
            .by_id
            .get(asset_id)
            .ok_or_else(|| anyhow::anyhow!("unknown image asset {:?}", asset_id))?;
        let record = self.assets[index]
            .as_mut()
            .expect("image asset index must reference a record");
        ensure!(
            matches!(record.origin, ImageAssetOrigin::User),
            "only user image assets can be modified"
        );
        user_file_path(record)?;
        Ok(record)
    }

    fn insert_user_record(
        &mut self,
        file: ImageAssetFileV1,
        definition: ImageAssetDefinition,
        file_path: PathBuf,
    ) -> Result<()> {
        ensure!(
            !self.by_id.contains_key(&definition.id),
            "duplicate image asset id {:?}",
            definition.id
        );
        let index = self.assets.len();
        self.by_id.insert(definition.id.clone(), index);
        self.assets.push(Some(ImageAssetRecord {
            definition,
            file,
            file_path: Some(file_path),
            origin: ImageAssetOrigin::User,
        }));
        Ok(())
    }
}

impl ImageRepresentation {
    pub fn rgba8(&self) -> Option<&[u8]> {
        matches!(self.format, ImageRepresentationFormat::Rgba8UnormSrgb)
            .then_some(self.bytes.as_ref())
    }

    pub fn r8(&self) -> Option<&[u8]> {
        matches!(self.format, ImageRepresentationFormat::R8Unorm).then_some(self.bytes.as_ref())
    }
}

pub fn build_r8_mask(rgba8: &[u8], size: [u32; 2], recipe: &ImageMaskRecipe) -> Result<Vec<u8>> {
    validate_size("image mask", size[0], size[1])?;
    ensure!(
        rgba8.len() == size[0] as usize * size[1] as usize * 4,
        "RGBA8 pixel length does not match image mask dimensions"
    );
    let mut mask = Vec::with_capacity(size[0] as usize * size[1] as usize);
    for pixel in rgba8.chunks_exact(4) {
        let value = match recipe.source {
            ImageMaskSource::Alpha => pixel[3],
            ImageMaskSource::Luminance => {
                ((pixel[0] as u32 * 54 + pixel[1] as u32 * 183 + pixel[2] as u32 * 19 + 128) / 256)
                    as u8
            }
            ImageMaskSource::Red => pixel[0],
            ImageMaskSource::Green => pixel[1],
            ImageMaskSource::Blue => pixel[2],
        };
        mask.push(if recipe.invert { 255 - value } else { value });
    }
    Ok(mask)
}

fn image_asset_files_in(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_image_asset_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_image_asset_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("reading image asset directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_image_asset_files(&path, files)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".image.ron"))
        {
            files.push(path);
        }
    }
    Ok(())
}

fn load_image_asset_file(file_path: &Path) -> Result<(ImageAssetFileV1, ImageAssetDefinition)> {
    let source = fs::read(file_path)
        .with_context(|| format!("reading image asset {}", file_path.display()))?;
    let base_dir = file_path.parent().unwrap_or_else(|| Path::new("."));
    parse_image_asset_file(&source, &file_path.display().to_string(), &|path| {
        let path = base_dir.join(path);
        fs::read(&path).with_context(|| format!("reading image representation {}", path.display()))
    })
}

fn parse_image_asset_file(
    source: &[u8],
    source_name: &str,
    read_relative: &impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<(ImageAssetFileV1, ImageAssetDefinition)> {
    let file: ImageAssetFileV1 = ron::de::from_bytes(source)
        .with_context(|| format!("parsing image asset {source_name}"))?;
    let definition = resolve_image_asset_file(&file, read_relative)
        .with_context(|| format!("validating image asset {source_name}"))?;
    Ok((file, definition))
}

fn read_embedded_image_source(
    ron_path: &str,
    relative_path: &str,
    lookup: impl Fn(&str) -> Option<&'static [u8]>,
) -> Result<Vec<u8>> {
    // Virtual paths must behave the same on all hosts, without filesystem resolution.
    let relative_path = relative_path.replace('\\', "/");
    ensure!(
        !relative_path.starts_with('/') && !relative_path.contains(':'),
        "embedded image source path must be relative: {relative_path:?}"
    );
    let mut components = ron_path.split('/').collect::<Vec<_>>();
    components.pop();
    for component in relative_path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                ensure!(
                    components.pop().is_some(),
                    "embedded image source escapes resource root: {relative_path:?}"
                );
            }
            _ => components.push(component),
        }
    }
    let path = components.join("/");
    lookup(&path)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| anyhow::anyhow!("embedded resource {path:?} was not found"))
}

fn user_file_path(record: &ImageAssetRecord) -> Result<&Path> {
    record.file_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "user image asset {:?} has no filesystem path",
            record.definition.id
        )
    })
}

fn resolve_image_asset_file(
    file: &ImageAssetFileV1,
    read_relative: &impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<ImageAssetDefinition> {
    ensure!(
        file.schema_version == IMAGE_ASSET_SCHEMA_VERSION,
        "unsupported image asset schema_version {}",
        file.schema_version
    );
    validate_id(&file.id, "image asset")?;
    ensure!(
        !file.display_name.trim().is_empty(),
        "image asset display_name must not be empty"
    );
    validate_tags(&file.tags, "image asset")?;
    ensure!(
        !file.representations.is_empty(),
        "image asset {:?} must contain at least one representation",
        file.id
    );

    let mut representation_ids = HashSet::new();
    let mut representations = Vec::with_capacity(file.representations.len());
    for representation in &file.representations {
        validate_id(&representation.id, "image representation")?;
        ensure!(
            representation_ids.insert(representation.id.clone()),
            "image asset {:?} has duplicate representation id {:?}",
            file.id,
            representation.id
        );
        validate_tags(&representation.tags, "image representation")?;
        let (size, bytes) = resolve_image_source(
            &file.id,
            representation.format,
            &representation.source,
            read_relative,
        )?;
        representations.push(ImageRepresentation {
            id: representation.id.clone(),
            format: representation.format,
            size,
            bytes: Arc::from(bytes),
            mipmaps: representation.mipmaps,
            tags: representation.tags.clone(),
            mask_recipe: representation.mask_recipe.clone(),
        });
    }

    Ok(ImageAssetDefinition {
        id: file.id.clone(),
        display_name: file.display_name.clone(),
        tags: file.tags.clone(),
        representations,
    })
}

fn resolve_image_source(
    asset_id: &str,
    format: ImageRepresentationFormat,
    source: &ImageSourceFileV1,
    read_relative: &impl Fn(&str) -> Result<Vec<u8>>,
) -> Result<([u32; 2], Vec<u8>)> {
    match source {
        ImageSourceFileV1::File { path, decode } => {
            ensure!(
                !path.trim().is_empty(),
                "image asset {:?} file path is empty",
                asset_id
            );
            match decode {
                ImageFileDecodeV1::RawR8 { width, height } => {
                    ensure!(
                        format == ImageRepresentationFormat::R8Unorm,
                        "image asset {:?} RawR8 source must use R8Unorm format",
                        asset_id
                    );
                    validate_size(asset_id, *width, *height)?;
                    let bytes = read_relative(path)?;
                    let expected_len = *width as usize * *height as usize;
                    ensure!(
                        bytes.len() == expected_len,
                        "image asset {:?} RawR8 expected {} bytes, got {}",
                        asset_id,
                        expected_len,
                        bytes.len()
                    );
                    Ok(([*width, *height], bytes))
                }
                ImageFileDecodeV1::PngRgba8 => {
                    ensure!(
                        format == ImageRepresentationFormat::Rgba8UnormSrgb,
                        "image asset {:?} PNG source must use Rgba8UnormSrgb format",
                        asset_id
                    );
                    let bytes = read_relative(path)?;
                    let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
                        .with_context(|| format!("decoding PNG image {path:?}"))?;
                    let rgba = decoded.to_rgba8();
                    validate_size(asset_id, rgba.width(), rgba.height())?;
                    Ok(([rgba.width(), rgba.height()], rgba.into_raw()))
                }
            }
        }
        ImageSourceFileV1::SolidR8 {
            width,
            height,
            value,
        } => {
            ensure!(
                format == ImageRepresentationFormat::R8Unorm,
                "image asset {:?} SolidR8 source must use R8Unorm format",
                asset_id
            );
            validate_size(asset_id, *width, *height)?;
            Ok((
                [*width, *height],
                vec![*value; *width as usize * *height as usize],
            ))
        }
    }
}

fn write_asset_file(path: &Path, file: &ImageAssetFileV1) -> Result<()> {
    let mut encoded = ron::ser::to_string_pretty(file, ron::ser::PrettyConfig::default())
        .context("serializing image asset metadata")?;
    encoded.push('\n');
    fs::write(path, encoded).with_context(|| format!("writing image asset {}", path.display()))
}

fn validate_id(id: &str, kind: &str) -> Result<()> {
    ensure!(!id.trim().is_empty(), "{kind} id must not be empty");
    ensure!(
        id.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-')),
        "{kind} id {:?} contains characters that are not safe for persistence",
        id
    );
    Ok(())
}

fn validate_tags(tags: &[String], kind: &str) -> Result<()> {
    let mut seen = HashSet::new();
    for tag in tags {
        ensure!(!tag.trim().is_empty(), "{kind} has an empty tag");
        ensure!(seen.insert(tag), "{kind} has duplicate tag {:?}", tag);
    }
    Ok(())
}

fn validate_size(id: &str, width: u32, height: u32) -> Result<()> {
    ensure!(width > 0, "image {:?} width must be > 0", id);
    ensure!(height > 0, "image {:?} height must be > 0", id);
    width
        .checked_mul(height)
        .ok_or_else(|| anyhow::anyhow!("image {:?} dimensions overflow", id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_assets_load_and_expose_brush_tip_representations() {
        let catalog = ImageAssetCatalog::load_effective(None).unwrap();
        assert!(catalog.assets.iter().flatten().all(|record| {
            record.origin == ImageAssetOrigin::Builtin && record.file_path.is_none()
        }));
        let assets = catalog
            .assets_with_representation_tag("brush_tip_mask")
            .collect::<Vec<_>>();
        assert!(assets.len() >= 4);
        assert!(assets.iter().all(|asset| {
            asset
                .representation_with_tag("brush_tip_mask")
                .is_some_and(|representation| {
                    representation.format == ImageRepresentationFormat::R8Unorm
                })
        }));
    }

    #[test]
    fn embedded_png_and_filesystem_png_share_relative_resolution_and_decode() {
        const RON: &[u8] = include_bytes!("../../tests/fixtures/images/nested/pixel.image.ron");
        const PNG: &[u8] = include_bytes!("../../tests/fixtures/images/pixel.png");
        let (_, embedded) = parse_image_asset_file(RON, "embedded PNG fixture", &|path| {
            read_embedded_image_source("images/nested/pixel.image.ron", path, |key| {
                (key == "images/pixel.png").then_some(PNG)
            })
        })
        .unwrap();
        assert_eq!(embedded.representations[0].size, [1, 1]);
        assert_eq!(embedded.representations[0].bytes.as_ref(), &[1, 2, 3, 4]);

        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("nested")).unwrap();
        fs::write(directory.path().join("pixel.png"), PNG).unwrap();
        let ron_path = directory.path().join("nested/pixel.image.ron");
        fs::write(&ron_path, RON).unwrap();
        let (_, user) = load_image_asset_file(&ron_path).unwrap();
        assert_eq!(embedded, user);
    }

    #[test]
    fn embedded_image_paths_are_virtual_and_missing_sources_do_not_fall_back() {
        let ron_path = "images/brush_tips/brush.image.ron";
        let expected = embedded_resources::bytes("images/brush_tips/brush.r8").unwrap();
        for path in [
            "brush.r8",
            "./brush.r8",
            "../brush_tips/brush.r8",
            r"..\brush_tips\brush.r8",
        ] {
            assert_eq!(
                read_embedded_image_source(ron_path, path, embedded_resources::bytes).unwrap(),
                expected
            );
        }
        for path in [
            "../../../brush.r8",
            "/brush.r8",
            r"C:\brush.r8",
            r"\server\brush.r8",
        ] {
            assert!(read_embedded_image_source(ron_path, path, embedded_resources::bytes).is_err());
        }
        let error = read_embedded_image_source(ron_path, "missing.r8", embedded_resources::bytes)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("embedded resource \"images/brush_tips/missing.r8\" was not found")
        );
    }

    #[test]
    fn image_decode_validates_raw_length_and_rejects_non_png_bytes() {
        let raw = ImageSourceFileV1::File {
            path: "mask.r8".to_owned(),
            decode: ImageFileDecodeV1::RawR8 {
                width: 2,
                height: 2,
            },
        };
        let error = resolve_image_source("test", ImageRepresentationFormat::R8Unorm, &raw, &|_| {
            Ok(vec![0; 3])
        })
        .unwrap_err();
        assert!(error.to_string().contains("expected 4 bytes, got 3"));
        let png = ImageSourceFileV1::File {
            path: "image.png".to_owned(),
            decode: ImageFileDecodeV1::PngRgba8,
        };
        assert!(
            resolve_image_source(
                "test",
                ImageRepresentationFormat::Rgba8UnormSrgb,
                &png,
                &|_| Ok(vec![0; 4])
            )
            .is_err()
        );
    }

    #[test]
    fn user_override_uses_its_own_filesystem_source_and_cannot_be_edited() {
        let directory = tempfile::tempdir().unwrap();
        let source = embedded_resources::bytes("images/brush_tips/brush.image.ron").unwrap();
        let mut file: ImageAssetFileV1 = ron::de::from_bytes(source).unwrap();
        file.display_name = "User override".to_owned();
        fs::write(directory.path().join("brush.r8"), vec![42; 27 * 27]).unwrap();
        write_asset_file(&directory.path().join("brush.image.ron"), &file).unwrap();
        let mut catalog = ImageAssetCatalog::load_effective(Some(directory.path())).unwrap();
        assert_eq!(
            catalog.origin(&file.id),
            Some(ImageAssetOrigin::UserOverride)
        );
        assert_eq!(
            catalog.get(&file.id).unwrap().representations[0]
                .bytes
                .as_ref(),
            &[42; 27 * 27]
        );
        let record = catalog.assets[catalog.by_id[&file.id]].as_ref().unwrap();
        assert_eq!(
            record.file_path.as_deref(),
            Some(directory.path().join("brush.image.ron").as_path())
        );
        assert!(catalog.rename_user(&file.id, "Changed").is_err());
        assert!(catalog.remove_user(&file.id).is_err());

        fs::write(directory.path().join("brush.image.ron"), "invalid RON").unwrap();
        let reloaded = ImageAssetCatalog::load_effective(Some(directory.path())).unwrap();
        assert_eq!(reloaded.origin(&file.id), Some(ImageAssetOrigin::Builtin));
    }

    #[test]
    fn r8_mask_conversion_supports_alpha_luminance_and_invert() {
        let rgba = [100, 150, 200, 50, 255, 0, 0, 255];
        let alpha = build_r8_mask(
            &rgba,
            [2, 1],
            &ImageMaskRecipe {
                source: ImageMaskSource::Alpha,
                invert: false,
            },
        )
        .unwrap();
        assert_eq!(alpha, vec![50, 255]);

        let luminance = build_r8_mask(
            &rgba,
            [2, 1],
            &ImageMaskRecipe {
                source: ImageMaskSource::Luminance,
                invert: true,
            },
        )
        .unwrap();
        assert_eq!(luminance.len(), 2);
        assert_eq!(luminance[1], 201);
    }

    #[test]
    fn user_asset_uses_the_same_schema_as_builtin_assets() {
        let dir = tempfile::tempdir().unwrap();
        let mut catalog = ImageAssetCatalog::load_effective(Some(dir.path())).unwrap();
        let id = catalog
            .import_image(
                "Imported",
                [1, 1],
                Arc::from(vec![1, 2, 3, 4]),
                vec!["decal_image".to_owned()],
            )
            .unwrap();
        assert_eq!(catalog.origin(&id), Some(ImageAssetOrigin::User));
        let reloaded = ImageAssetCatalog::load_effective(Some(dir.path())).unwrap();
        let asset = reloaded.get(&id).unwrap();
        assert_eq!(asset.display_name, "Imported");
        assert_eq!(
            asset
                .representation_with_tag("decal_image")
                .unwrap()
                .bytes
                .as_ref(),
            &[1, 2, 3, 4]
        );
        catalog.rename_user(&id, "Renamed").unwrap();
        catalog
            .set_user_tags(&id, vec!["custom".to_owned()])
            .unwrap();
        let mut reloaded = ImageAssetCatalog::load_effective(Some(dir.path())).unwrap();
        assert_eq!(reloaded.get(&id).unwrap().display_name, "Renamed");
        assert_eq!(reloaded.get(&id).unwrap().tags, vec!["custom"]);
        reloaded.remove_user(&id).unwrap();
        assert!(reloaded.get(&id).is_none());
        assert!(
            ImageAssetCatalog::load_effective(Some(dir.path()))
                .unwrap()
                .get(&id)
                .is_none()
        );
    }

    #[test]
    fn user_asset_mask_representations_persist_with_independent_tags() {
        let dir = tempfile::tempdir().unwrap();
        let rgba = vec![20, 40, 80, 100, 200, 100, 50, 255];
        let mut catalog = ImageAssetCatalog::load_effective(Some(dir.path())).unwrap();
        let id = catalog
            .import_image("Masks", [2, 1], Arc::from(rgba), Vec::new())
            .unwrap();
        let brush_id = catalog
            .add_r8_representation_from_rgba(
                &id,
                vec!["brush_tip_mask".to_owned()],
                ImageMaskRecipe {
                    source: ImageMaskSource::Alpha,
                    invert: false,
                },
            )
            .unwrap();
        let future_id = catalog
            .add_r8_representation_from_rgba(
                &id,
                vec!["future_mask".to_owned()],
                ImageMaskRecipe {
                    source: ImageMaskSource::Luminance,
                    invert: true,
                },
            )
            .unwrap();
        assert_ne!(brush_id, future_id);

        let reloaded = ImageAssetCatalog::load_effective(Some(dir.path())).unwrap();
        let asset = reloaded.get(&id).unwrap();
        let brush = asset.representation(&brush_id).unwrap();
        assert_eq!(brush.r8().unwrap(), &[100, 255]);
        assert_eq!(
            brush.mask_recipe,
            Some(ImageMaskRecipe {
                source: ImageMaskSource::Alpha,
                invert: false,
            })
        );
        let future = asset.representation(&future_id).unwrap();
        assert_eq!(future.tags, vec!["future_mask".to_owned()]);
        assert_eq!(
            future.mask_recipe,
            Some(ImageMaskRecipe {
                source: ImageMaskSource::Luminance,
                invert: true,
            })
        );
    }
    #[test]
    fn removing_user_mask_representation_updates_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let rgba = vec![20, 40, 80, 100, 200, 100, 50, 255];
        let mut catalog = ImageAssetCatalog::load_effective(Some(dir.path())).unwrap();
        let id = catalog
            .import_image("Masks", [2, 1], Arc::from(rgba), Vec::new())
            .unwrap();
        let brush_id = catalog
            .add_r8_representation_from_rgba(
                &id,
                vec!["brush_tip_mask".to_owned()],
                ImageMaskRecipe {
                    source: ImageMaskSource::Alpha,
                    invert: false,
                },
            )
            .unwrap();

        let removed = catalog
            .remove_user_r8_representation(&id, &brush_id)
            .unwrap();
        assert_eq!(removed.id, brush_id);
        assert!(
            catalog
                .get(&id)
                .unwrap()
                .representation(&removed.id)
                .is_none()
        );

        let reloaded = ImageAssetCatalog::load_effective(Some(dir.path())).unwrap();
        let asset = reloaded.get(&id).unwrap();
        assert!(asset.representation(&removed.id).is_none());
        assert!(asset.rgba8_representation().is_some());
    }
}

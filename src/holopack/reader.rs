use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read, Seek},
    path::{Component, Path},
};

use anyhow::{Context, Result, bail, ensure};
use zip::ZipArchive;

use super::{HoloPackManifest, HoloPackResourceEntry};

const MANIFEST_PATH: &str = "manifest.ron";

#[derive(Debug, Clone)]
pub(crate) struct HoloPackResource {
    pub(crate) entry: HoloPackResourceEntry,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub(crate) struct HoloPackContent {
    pub(crate) manifest: HoloPackManifest,
    pub(crate) resources: Vec<HoloPackResource>,
}

pub(crate) struct HoloPackReader;

impl HoloPackReader {
    pub(crate) fn read<R: Read + Seek>(mut source: R) -> Result<HoloPackContent> {
        let mut archive_bytes = Vec::new();
        source
            .read_to_end(&mut archive_bytes)
            .context("reading HoloPack ZIP archive")?;
        let mut archive = ZipArchive::new(Cursor::new(archive_bytes.as_slice()))
            .context("opening HoloPack ZIP archive")?;
        let central_directory_start = archive.central_directory_start() as usize;
        let raw_names = central_directory_names(&archive_bytes, central_directory_start)?;
        let mut unique_names = HashSet::new();
        for name in raw_names {
            ensure!(
                unique_names.insert(name.clone()),
                "duplicate ZIP entry {name:?}"
            );
        }
        let mut entries = HashMap::<String, Vec<usize>>::new();
        for index in 0..archive.len() {
            let file = archive
                .by_index(index)
                .with_context(|| format!("reading HoloPack ZIP entry {index}"))?;
            entries
                .entry(file.name().to_owned())
                .or_default()
                .push(index);
        }

        let manifest_indices = entries.get(MANIFEST_PATH).cloned().unwrap_or_default();
        ensure!(!manifest_indices.is_empty(), "missing manifest.ron");
        ensure!(manifest_indices.len() == 1, "duplicate manifest.ron");
        let manifest_source = read_entry(&mut archive, manifest_indices[0], MANIFEST_PATH)?;
        let manifest: HoloPackManifest =
            ron::de::from_bytes(&manifest_source).context("parsing HoloPack manifest.ron")?;
        manifest.validate().map_err(anyhow::Error::msg)?;

        let mut declared_paths = HashSet::new();
        let mut resources = Vec::with_capacity(manifest.resources.len());
        for entry in &manifest.resources {
            validate_resource_path(&entry.path)
                .with_context(|| format!("invalid resource path {:?}", entry.path))?;
            ensure!(
                declared_paths.insert(entry.path.clone()),
                "duplicate resource path {:?}",
                entry.path
            );
            let indices = entries
                .get(&entry.path)
                .ok_or_else(|| anyhow::anyhow!("missing resource file {:?}", entry.path))?;
            ensure!(
                indices.len() == 1,
                "resource path {:?} occurs more than once in the archive",
                entry.path
            );
            let file = archive
                .by_index(indices[0])
                .with_context(|| format!("opening HoloPack resource {:?}", entry.path))?;
            ensure!(
                !file.is_dir(),
                "resource path {:?} refers to a directory",
                entry.path
            );
            drop(file);
            resources.push(HoloPackResource {
                entry: entry.clone(),
                bytes: read_entry(&mut archive, indices[0], &entry.path)?,
            });
        }

        Ok(HoloPackContent {
            manifest,
            resources,
        })
    }
}

fn central_directory_names(bytes: &[u8], mut offset: usize) -> Result<Vec<String>> {
    const CENTRAL_HEADER_SIZE: usize = 46;
    const CENTRAL_HEADER_SIGNATURE: &[u8; 4] = b"PK\x01\x02";

    let mut names = Vec::new();
    while bytes.get(offset..offset + 4) == Some(CENTRAL_HEADER_SIGNATURE) {
        let header = bytes
            .get(offset..offset + CENTRAL_HEADER_SIZE)
            .context("truncated ZIP central directory header")?;
        let name_len = u16::from_le_bytes([header[28], header[29]]) as usize;
        let extra_len = u16::from_le_bytes([header[30], header[31]]) as usize;
        let comment_len = u16::from_le_bytes([header[32], header[33]]) as usize;
        let name_start = offset + CENTRAL_HEADER_SIZE;
        let name_end = name_start
            .checked_add(name_len)
            .context("ZIP central directory filename length overflow")?;
        let name = bytes
            .get(name_start..name_end)
            .context("truncated ZIP central directory filename")?;
        names.push(String::from_utf8_lossy(name).into_owned());
        offset = name_end
            .checked_add(extra_len)
            .and_then(|value| value.checked_add(comment_len))
            .context("ZIP central directory entry length overflow")?;
    }
    Ok(names)
}

fn read_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    index: usize,
    name: &str,
) -> Result<Vec<u8>> {
    let mut file = archive
        .by_index(index)
        .with_context(|| format!("opening HoloPack entry {name:?}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("reading HoloPack entry {name:?}"))?;
    Ok(bytes)
}

fn validate_resource_path(value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "resource path must not be empty");
    ensure!(
        !value.contains('\\'),
        "resource path must use ZIP forward-slash separators"
    );
    let path = Path::new(value);
    ensure!(
        !path.is_absolute(),
        "absolute resource paths are not allowed"
    );
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => bail!("current-directory path components are not allowed"),
            Component::ParentDir => bail!("parent-directory path components are not allowed"),
            Component::RootDir | Component::Prefix(_) => {
                bail!("rooted resource paths are not allowed")
            }
        }
    }
    ensure!(
        path.components().next().is_some(),
        "resource path must name a file"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::{ZipWriter, write::SimpleFileOptions};

    use super::*;

    fn pack(manifest: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut bytes);
            let options = SimpleFileOptions::default();
            writer.start_file(MANIFEST_PATH, options).unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
            for (name, contents) in files {
                writer.start_file(*name, options).unwrap();
                writer.write_all(contents).unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.into_inner()
    }

    fn manifest(path: &str, kind: &str) -> String {
        format!(
            "(format_version:1,id:\"com.example.test\",name:\"Test\",version:\"1\",resources:[(kind:\"{kind}\",path:\"{path}\")])"
        )
    }

    #[test]
    fn reads_declared_resources_and_allows_unknown_kind_and_extra_file() {
        let manifest = manifest("future/data.bin", "future_kind");
        let bytes = pack(
            &manifest,
            &[("future/data.bin", b"data"), ("README.txt", b"extra")],
        );
        let content = HoloPackReader::read(Cursor::new(bytes)).unwrap();
        assert_eq!(content.resources.len(), 1);
        assert_eq!(content.resources[0].entry.kind, "future_kind");
        assert_eq!(content.resources[0].bytes, b"data");
    }

    #[test]
    fn rejects_missing_manifest() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut bytes);
            writer
                .start_file("README.txt", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"missing").unwrap();
            writer.finish().unwrap();
        }
        assert!(
            HoloPackReader::read(Cursor::new(bytes.into_inner()))
                .unwrap_err()
                .to_string()
                .contains("missing manifest")
        );
    }

    #[test]
    fn rejects_duplicate_and_malformed_manifest() {
        let mut duplicate = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut duplicate);
            let options = SimpleFileOptions::default();
            writer.start_file(MANIFEST_PATH, options).unwrap();
            writer.write_all(b"invalid").unwrap();
            writer.start_file("manifezt.ron", options).unwrap();
            writer.write_all(b"invalid").unwrap();
            writer.finish().unwrap();
        }
        let mut duplicate = duplicate.into_inner();
        for index in 0..=duplicate.len() - b"manifezt.ron".len() {
            if &duplicate[index..index + b"manifezt.ron".len()] == b"manifezt.ron" {
                duplicate[index..index + b"manifest.ron".len()].copy_from_slice(b"manifest.ron");
            }
        }
        let error = HoloPackReader::read(Cursor::new(duplicate)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("duplicate ZIP entry \"manifest.ron\""),
            "unexpected error: {error:#}"
        );

        let malformed = pack("not valid RON", &[]);
        assert!(
            HoloPackReader::read(Cursor::new(malformed))
                .unwrap_err()
                .to_string()
                .contains("parsing HoloPack manifest")
        );
    }

    #[test]
    fn rejects_traversal_absolute_directory_and_duplicate_declared_paths() {
        for path in ["../escape", "/absolute", "C:/absolute"] {
            let manifest = manifest(path, "texture");
            let error = HoloPackReader::read(Cursor::new(pack(&manifest, &[]))).unwrap_err();
            assert!(error.to_string().contains("invalid resource path"));
        }

        let duplicate = "(format_version:1,id:\"id\",name:\"n\",version:\"1\",resources:[(kind:\"texture\",path:\"a.png\"),(kind:\"texture\",path:\"a.png\")])";
        let error =
            HoloPackReader::read(Cursor::new(pack(duplicate, &[("a.png", b"x")]))).unwrap_err();
        assert!(error.to_string().contains("duplicate resource path"));
    }

    #[test]
    fn rejects_unsupported_format_version_and_missing_resource() {
        let unsupported =
            manifest("a.png", "texture").replace("format_version:1", "format_version:2");
        assert!(
            HoloPackReader::read(Cursor::new(pack(&unsupported, &[("a.png", b"x")])))
                .unwrap_err()
                .to_string()
                .contains("unsupported HoloPack format version")
        );
        let missing = manifest("missing.png", "texture");
        assert!(
            HoloPackReader::read(Cursor::new(pack(&missing, &[])))
                .unwrap_err()
                .to_string()
                .contains("missing resource file")
        );
    }

    #[test]
    fn rejects_directory_resource() {
        let manifest = manifest("textures/", "texture");
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut bytes);
            let options = SimpleFileOptions::default();
            writer.start_file(MANIFEST_PATH, options).unwrap();
            writer.write_all(manifest.as_bytes()).unwrap();
            writer.add_directory("textures/", options).unwrap();
            writer.finish().unwrap();
        }
        assert!(
            HoloPackReader::read(Cursor::new(bytes.into_inner()))
                .unwrap_err()
                .to_string()
                .contains("refers to a directory")
        );
    }
}

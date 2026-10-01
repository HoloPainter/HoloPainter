use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::{core::brush_preset::BrushPresetDefinition, persistence::save_ron_atomic};

#[derive(Debug)]
pub(crate) struct BrushPresetStore {
    dir: Option<PathBuf>,
    last_saved: BTreeMap<String, BrushPresetDefinition>,
}

impl BrushPresetStore {
    pub(crate) fn new(dir: Option<PathBuf>, baseline: Vec<BrushPresetDefinition>) -> Self {
        Self {
            dir,
            last_saved: definitions_by_id(baseline),
        }
    }

    #[cfg(test)]
    fn sync_now(&mut self, current: Vec<BrushPresetDefinition>) -> Result<(), String> {
        self.sync_with_layout(current, || Ok(()))
    }

    /// Publish resources before their references, and remove resources only after
    /// the layout has stopped referencing them. Successful file writes are retained
    /// across failures so the next check retries only unfinished work.
    pub(crate) fn sync_with_layout(
        &mut self,
        current: Vec<BrushPresetDefinition>,
        save_layout: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(dir) = self.dir.as_deref() else {
            return save_layout();
        };
        let current = definitions_by_id(current);
        for (id, definition) in &current {
            if self.last_saved.get(id) != Some(definition) {
                save_ron_atomic(&preset_path(dir, id), definition)?;
                self.last_saved.insert(id.clone(), definition.clone());
            }
        }
        save_layout()?;
        let removed: Vec<_> = self
            .last_saved
            .keys()
            .filter(|id| !current.contains_key(*id))
            .cloned()
            .collect();
        for id in removed {
            match fs::remove_file(preset_path(dir, &id)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("deleting brush preset {id:?}: {error}")),
            }
            self.last_saved.remove(&id);
        }
        Ok(())
    }
}

fn definitions_by_id(
    definitions: Vec<BrushPresetDefinition>,
) -> BTreeMap<String, BrushPresetDefinition> {
    definitions
        .into_iter()
        .map(|definition| (definition.id().to_owned(), definition))
        .collect()
}

fn preset_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.ron"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::brush_preset::BrushPresetCatalog;

    fn preset(id: &str) -> BrushPresetDefinition {
        let fixture_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brushes/presets");
        let catalog = BrushPresetCatalog::load_from_fixtures(fixture_dir, None).unwrap();
        catalog
            .get(catalog.handle("builtin.brush.paint.round_soft").unwrap())
            .unwrap()
            .clone_as_user(id, id)
    }

    #[test]
    fn layout_failure_keeps_old_resources_and_retries_without_rewriting_new_resources() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = BrushPresetStore::new(Some(temp.path().to_owned()), Vec::new());
        store.sync_now(vec![preset("old")]).unwrap();
        let current = vec![preset("new")];
        let result = store.sync_with_layout(current.clone(), || {
            assert!(temp.path().join("new.ron").exists());
            assert!(temp.path().join("old.ron").exists());
            Err("layout write failed".to_owned())
        });
        assert!(result.is_err());
        assert!(store.last_saved.contains_key("old"));
        assert!(store.last_saved.contains_key("new"));
        // Detect an unnecessary rewrite without relying on filesystem clock precision.
        fs::write(temp.path().join("new.ron"), "unchanged write marker").unwrap();
        store
            .sync_with_layout(current, || {
                assert!(temp.path().join("old.ron").exists());
                Ok(())
            })
            .unwrap();
        assert!(!temp.path().join("old.ron").exists());
        assert_eq!(
            fs::read_to_string(temp.path().join("new.ron")).unwrap(),
            "unchanged write marker"
        );
    }

    #[test]
    fn resource_write_failure_does_not_publish_layout() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("presets");
        fs::write(&dir, "blocks directory creation").unwrap();
        let mut store = BrushPresetStore::new(Some(dir.clone()), Vec::new());
        assert!(
            store
                .sync_with_layout(vec![preset("new")], || panic!("layout must not be saved"))
                .is_err()
        );
        assert!(store.last_saved.is_empty());
        fs::remove_file(&dir).unwrap();
        store
            .sync_with_layout(vec![preset("new")], || {
                assert!(dir.join("new.ron").exists());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn deletion_failure_remains_pending_until_retry() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = BrushPresetStore::new(Some(temp.path().to_owned()), vec![preset("old")]);
        fs::create_dir(temp.path().join("old.ron")).unwrap();
        assert!(store.sync_with_layout(Vec::new(), || Ok(())).is_err());
        assert!(store.last_saved.contains_key("old"));
        fs::remove_dir(temp.path().join("old.ron")).unwrap();
        store.sync_with_layout(Vec::new(), || Ok(())).unwrap();
        assert!(store.last_saved.is_empty());
    }

    #[test]
    fn sync_writes_and_deletes_user_preset_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let fixture_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brushes/presets");
        let catalog =
            BrushPresetCatalog::load_from_fixtures(fixture_dir, None).expect("fixture presets");
        let source = catalog
            .get(
                catalog
                    .handle("builtin.brush.paint.round_soft")
                    .expect("preset"),
            )
            .expect("preset")
            .clone_as_user("user.brush.test", "Test Brush");
        let mut store = BrushPresetStore::new(Some(temp.path().to_owned()), Vec::new());

        assert!(!temp.path().join("user.brush.test.ron").exists());
        store.sync_now(vec![source]).expect("save preset");
        assert!(temp.path().join("user.brush.test.ron").exists());
        store.sync_now(Vec::new()).expect("delete preset");
        assert!(!temp.path().join("user.brush.test.ron").exists());
    }
}

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Serialize, de::DeserializeOwned};
use tempfile::NamedTempFile;

pub(crate) const RON_AUTOSAVE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) struct RonAutosave<T> {
    label: &'static str,
    path: Option<PathBuf>,
    last_saved: T,
    next_check_at: Instant,
}

impl<T> RonAutosave<T>
where
    T: PartialEq + Serialize,
{
    pub(crate) fn new(
        label: &'static str,
        path: Option<PathBuf>,
        baseline: T,
        now: Instant,
    ) -> Self {
        Self {
            label,
            path,
            last_saved: baseline,
            next_check_at: now + RON_AUTOSAVE_INTERVAL,
        }
    }

    pub(crate) fn poll(
        &mut self,
        now: Instant,
        capture: impl FnOnce() -> Result<T, String>,
        validate: impl FnOnce(&T) -> Result<(), String>,
    ) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if now < self.next_check_at {
            return;
        }
        self.next_check_at = now + RON_AUTOSAVE_INTERVAL;

        let snapshot = match capture() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!("Unable to capture {} snapshot: {error}", self.label);
                return;
            }
        };
        if snapshot == self.last_saved {
            return;
        }
        if let Err(error) = validate(&snapshot) {
            eprintln!("Unable to save invalid {} snapshot: {error}", self.label);
            return;
        }
        match save_ron_atomic(path, &snapshot) {
            Ok(()) => self.last_saved = snapshot,
            Err(error) => eprintln!("Failed to save {} {}: {error}", self.label, path.display()),
        }
    }

    pub(crate) fn save_now(
        &mut self,
        snapshot: T,
        validate: impl FnOnce(&T) -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(path) = self.path.as_deref() else {
            self.last_saved = snapshot;
            return Ok(());
        };
        validate(&snapshot)?;
        save_ron_atomic(path, &snapshot)?;
        self.last_saved = snapshot;
        Ok(())
    }

    pub(crate) fn save_if_changed(
        &mut self,
        snapshot: T,
        validate: impl FnOnce(&T) -> Result<(), String>,
    ) -> Result<(), String> {
        if snapshot == self.last_saved {
            return Ok(());
        }
        self.save_now(snapshot, validate)
    }

    pub(crate) fn time_until_check(&self, now: Instant) -> Option<Duration> {
        self.path
            .as_ref()
            .map(|_| self.next_check_at.saturating_duration_since(now))
    }
}

pub(crate) fn parse_ron<T>(source: &str, context: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    ron::from_str(source).map_err(|error| format!("parsing {context}: {error}"))
}

pub(crate) fn load_ron<T>(path: &Path) -> Result<Option<T>, String>
where
    T: DeserializeOwned,
{
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("reading RON file: {error}")),
    };
    parse_ron(&text, "RON file").map(Some)
}

pub(crate) fn save_ron_atomic<T>(path: &Path, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    let parent = path
        .parent()
        .ok_or_else(|| "RON path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("creating RON directory: {error}"))?;
    let text = ron::ser::to_string_pretty(value, ron::ser::PrettyConfig::default())
        .map_err(|error| format!("serializing RON file: {error}"))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| format!("creating temporary RON file: {error}"))?;
    temporary
        .write_all(text.as_bytes())
        .map_err(|error| format!("writing temporary RON file: {error}"))?;
    temporary
        .flush()
        .map_err(|error| format!("flushing temporary RON file: {error}"))?;
    temporary
        .persist(path)
        .map_err(|error| format!("replacing RON file: {}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestSnapshot {
        value: u32,
    }

    #[test]
    fn parse_ron_includes_context_in_errors() {
        let error = parse_ron::<TestSnapshot>("not ron", "embedded test snapshot")
            .expect_err("invalid RON should fail");

        assert!(error.contains("parsing embedded test snapshot"));
    }

    #[test]
    fn poll_skips_capture_until_interval_is_due() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("state.ron");
        let t0 = Instant::now();
        let mut autosave = RonAutosave::new(
            "test state",
            Some(path.clone()),
            TestSnapshot { value: 1 },
            t0,
        );
        let captures = Cell::new(0);

        autosave.poll(
            t0 + RON_AUTOSAVE_INTERVAL - Duration::from_millis(1),
            || {
                captures.set(captures.get() + 1);
                Ok(TestSnapshot { value: 2 })
            },
            |_| Ok(()),
        );

        assert_eq!(captures.get(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn unchanged_snapshot_does_not_create_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("state.ron");
        let t0 = Instant::now();
        let mut autosave = RonAutosave::new(
            "test state",
            Some(path.clone()),
            TestSnapshot { value: 1 },
            t0,
        );

        autosave.poll(
            t0 + RON_AUTOSAVE_INTERVAL,
            || Ok(TestSnapshot { value: 1 }),
            |_| Ok(()),
        );

        assert!(!path.exists());
        assert!(!path.parent().expect("parent").exists());
    }

    #[test]
    fn changed_snapshots_replace_saved_ron_on_check_interval() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("state.ron");
        let t0 = Instant::now();
        let mut autosave = RonAutosave::new(
            "test state",
            Some(path.clone()),
            TestSnapshot { value: 1 },
            t0,
        );

        autosave.poll(
            t0 + RON_AUTOSAVE_INTERVAL,
            || Ok(TestSnapshot { value: 2 }),
            |_| Ok(()),
        );
        assert_eq!(
            load_ron::<TestSnapshot>(&path).expect("load saved state"),
            Some(TestSnapshot { value: 2 })
        );

        autosave.poll(
            t0 + RON_AUTOSAVE_INTERVAL * 2,
            || Ok(TestSnapshot { value: 3 }),
            |_| Ok(()),
        );
        assert_eq!(
            load_ron::<TestSnapshot>(&path).expect("load replaced state"),
            Some(TestSnapshot { value: 3 })
        );
    }

    #[test]
    fn failed_write_keeps_previous_baseline_for_next_check() {
        let temp = tempfile::tempdir().expect("tempdir");
        let blocked_parent = temp.path().join("blocked");
        fs::write(&blocked_parent, b"not a directory").expect("create blocking file");
        let path = blocked_parent.join("state.ron");
        let t0 = Instant::now();
        let mut autosave = RonAutosave::new(
            "test state",
            Some(path.clone()),
            TestSnapshot { value: 1 },
            t0,
        );

        autosave.poll(
            t0 + RON_AUTOSAVE_INTERVAL,
            || Ok(TestSnapshot { value: 2 }),
            |_| Ok(()),
        );
        assert!(!path.exists());

        fs::remove_file(&blocked_parent).expect("remove blocking file");
        fs::create_dir(&blocked_parent).expect("create parent directory");
        autosave.poll(
            t0 + RON_AUTOSAVE_INTERVAL * 2,
            || Ok(TestSnapshot { value: 2 }),
            |_| Ok(()),
        );
        assert_eq!(
            load_ron::<TestSnapshot>(&path).expect("load retried state"),
            Some(TestSnapshot { value: 2 })
        );
    }

    #[test]
    fn no_path_disables_checks() {
        let t0 = Instant::now();
        let mut autosave = RonAutosave::new("test state", None, TestSnapshot { value: 1 }, t0);
        let captures = Cell::new(0);

        autosave.poll(
            t0 + RON_AUTOSAVE_INTERVAL,
            || {
                captures.set(captures.get() + 1);
                Ok(TestSnapshot { value: 2 })
            },
            |_| Ok(()),
        );

        assert_eq!(captures.get(), 0);
        assert!(autosave.time_until_check(t0).is_none());
    }
}

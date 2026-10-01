use std::{collections::HashSet, fs, io::Write, path::PathBuf};

use anyhow::{Context, Result, bail, ensure};
use tempfile::{NamedTempFile, TempPath};

/// Writes every PSD to a temporary file before changing any destination, then
/// replaces the destinations as one recoverable transaction.
pub fn write_psd_files_transactionally(files: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    ensure!(!files.is_empty(), "there are no PSD files to write");
    let mut paths = HashSet::with_capacity(files.len());
    for (path, _) in files {
        ensure!(
            paths.insert(path.clone()),
            "duplicate PSD output path: {}",
            path.display()
        );
        let parent = path
            .parent()
            .context("PSD output path has no parent folder")?;
        ensure!(
            parent.is_dir(),
            "PSD output folder is unavailable: {}",
            parent.display()
        );
        if path.exists() && !path.is_file() {
            bail!("PSD output is not a file: {}", path.display());
        }
    }

    let mut staged = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        let parent = path.parent().expect("output parent was validated");
        let mut temporary = NamedTempFile::new_in(parent)
            .with_context(|| format!("creating a temporary file for {}", path.display()))?;
        temporary
            .write_all(bytes)
            .with_context(|| format!("writing temporary PSD for {}", path.display()))?;
        temporary
            .flush()
            .with_context(|| format!("flushing temporary PSD for {}", path.display()))?;
        staged.push((temporary, path.clone()));
    }

    let mut backup_slots = Vec::new();
    for (_, path) in &staged {
        if !path.exists() {
            continue;
        }
        let parent = path.parent().expect("output parent was validated");
        let placeholder = NamedTempFile::new_in(parent)
            .with_context(|| format!("creating an overwrite backup for {}", path.display()))?;
        backup_slots.push((path.clone(), placeholder.into_temp_path()));
    }
    for (path, backup) in &backup_slots {
        fs::remove_file(backup)
            .with_context(|| format!("preparing an overwrite backup for {}", path.display()))?;
    }

    let mut backups = Vec::new();
    for (path, backup) in backup_slots {
        if let Err(error) = fs::rename(&path, &backup) {
            let rollback = restore_backups(&mut backups);
            let primary = anyhow::Error::new(error)
                .context(format!("backing up {} before overwrite", path.display()));
            return match rollback {
                Ok(()) => Err(primary),
                Err(rollback_error) => {
                    Err(primary.context(format!("rollback also failed: {rollback_error:#}")))
                }
            };
        }
        backups.push((path, backup));
    }

    let mut committed = Vec::new();
    for (temporary, path) in staged {
        if let Err(error) = temporary.persist(&path) {
            let primary = anyhow::Error::new(error.error)
                .context(format!("installing exported PSD {}", path.display()));
            let rollback = rollback_commit(&committed, &mut backups);
            return match rollback {
                Ok(()) => Err(primary),
                Err(rollback_error) => {
                    Err(primary.context(format!("rollback also failed: {rollback_error:#}")))
                }
            };
        }
        committed.push(path);
    }
    Ok(())
}

fn rollback_commit(committed: &[PathBuf], backups: &mut Vec<(PathBuf, TempPath)>) -> Result<()> {
    let mut errors = Vec::new();
    for path in committed.iter().rev() {
        if let Err(error) = fs::remove_file(path) {
            errors.push(format!("removing {}: {error}", path.display()));
        }
    }
    if let Err(error) = restore_backups(backups) {
        errors.push(format!("restoring overwritten files: {error:#}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(errors.join("; "))
    }
}

fn restore_backups(backups: &mut Vec<(PathBuf, TempPath)>) -> Result<()> {
    let mut errors = Vec::new();
    while let Some((destination, backup)) = backups.pop() {
        if let Err(error) = fs::rename(&backup, &destination) {
            errors.push(format!("{}: {error}", destination.display()));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        bail!(errors.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_new_files_and_replaces_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.psd");
        let second = directory.path().join("second.psd");
        fs::write(&first, b"old").unwrap();

        write_psd_files_transactionally(&[
            (first.clone(), b"new first".to_vec()),
            (second.clone(), b"new second".to_vec()),
        ])
        .unwrap();

        assert_eq!(fs::read(first).unwrap(), b"new first");
        assert_eq!(fs::read(second).unwrap(), b"new second");
    }

    #[test]
    fn preflight_failure_does_not_change_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let valid = directory.path().join("valid.psd");
        let invalid = directory.path().join("folder.psd");
        fs::write(&valid, b"old").unwrap();
        fs::create_dir(&invalid).unwrap();

        assert!(
            write_psd_files_transactionally(&[
                (valid.clone(), b"new".to_vec()),
                (invalid, b"bad".to_vec()),
            ])
            .is_err()
        );
        assert_eq!(fs::read(valid).unwrap(), b"old");
    }

    #[test]
    fn rejects_duplicate_destinations_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("same.psd");
        assert!(
            write_psd_files_transactionally(&[(path.clone(), vec![1]), (path, vec![2]),]).is_err()
        );
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};

struct DirectorySnapshot {
    metadata: Metadata,
    entries: BTreeMap<OsString, Metadata>,
}

/// Restore source directory access/modification times only after known removals.
/// Unrelated changes or unavailable native identities leave observed times intact.
pub struct SourceDirectoryCleanupSnapshot {
    directories: BTreeMap<PathBuf, DirectorySnapshot>,
    requested_root: PathBuf,
    canonical_root: PathBuf,
}

impl SourceDirectoryCleanupSnapshot {
    /// Snapshot only cleanup candidates' parent directories and their ancestors.
    /// This does not authorize removal of any candidate.
    pub fn capture(root: &Path, candidates: &[PathBuf]) -> io::Result<Self> {
        let requested_root = root.to_path_buf();
        let canonical_root = root.canonicalize()?;
        let root = canonical_root.as_path();
        let mut directories = BTreeSet::from([root.to_path_buf()]);
        for candidate in candidates {
            let relative = candidate
                .strip_prefix(&requested_root)
                .map_err(io::Error::other)?;
            if relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "source cleanup candidate must remain inside its root",
                ));
            }
            let candidate = root.join(relative);
            let mut directory = match fs::symlink_metadata(&candidate) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    Some(candidate.as_path())
                }
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    candidate.parent()
                }
                Ok(_) => candidate.parent(),
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            while let Some(path) = directory.filter(|path| path.starts_with(root)) {
                directories.insert(path.to_path_buf());
                directory = path.parent();
            }
        }
        let directories = directories
            .into_iter()
            .map(|path| {
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "source cleanup snapshot refuses symlink/non-directory parents",
                    ));
                }
                let entries = directory_entries(&path)?;
                Ok((path, DirectorySnapshot { metadata, entries }))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            directories,
            requested_root,
            canonical_root,
        })
    }

    /// Return false without changing times when the observed change is not ours.
    /// The comparison is a filesystem observation, not a content/custody proof.
    pub fn restore_after_cleanup(self, known_removed_files: &[PathBuf]) -> io::Result<bool> {
        self.restore_after_cleanup_with(known_removed_files, || {})
    }

    fn restore_after_cleanup_with(
        self,
        known_removed_files: &[PathBuf],
        before_apply: impl FnOnce(),
    ) -> io::Result<bool> {
        let known_removed_files = known_removed_files
            .iter()
            .map(|path| {
                path.strip_prefix(&self.requested_root)
                    .map(|relative| self.canonical_root.join(relative))
                    .map_err(io::Error::other)
            })
            .collect::<io::Result<BTreeSet<_>>>()?;
        let mut surviving = Vec::new();
        for (path, before) in &self.directories {
            let (metadata, entries) = match fs::symlink_metadata(path) {
                Ok(metadata) => {
                    if !same_node(&before.metadata, &metadata) {
                        return Ok(false);
                    }
                    (Some(metadata), directory_entries(path)?)
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => (None, BTreeMap::new()),
                Err(error) => return Err(error),
            };
            if entries
                .keys()
                .any(|name| !before.entries.contains_key(name))
            {
                return Ok(false);
            }
            let mut removed = false;
            for (name, original) in &before.entries {
                let entry_path = path.join(name);
                match entries.get(name) {
                    Some(current) => {
                        if !same_node(original, current)
                            || (!self.directories.contains_key(&entry_path)
                                && !same_file_observation(original, current))
                        {
                            return Ok(false);
                        }
                    }
                    None if (original.is_file()
                        && !original.file_type().is_symlink()
                        && known_removed_files.contains(&entry_path))
                        || (self.directories.contains_key(&entry_path)
                            && known_removed_files
                                .iter()
                                .any(|removed| removed.starts_with(&entry_path))) =>
                    {
                        removed = true;
                    }
                    None => return Ok(false),
                }
            }
            if let Some(metadata) = metadata {
                if !removed && !same_file_observation(&before.metadata, &metadata) {
                    return Ok(false);
                }
                if removed {
                    surviving.push((path, before, metadata, entries));
                }
            }
        }
        before_apply();
        // Recheck the observation, not just the inode: another process may edit it meanwhile.
        for (path, directory, observed, entries) in surviving {
            let handle = File::open(path)?;
            let current = fs::symlink_metadata(path)?;
            let opened = handle.metadata()?;
            if !same_node(&observed, &current)
                || !same_file_observation(&observed, &current)
                || !same_node(&observed, &opened)
                || !same_file_observation(&observed, &opened)
                || !same_directory_entries(&entries, &directory_entries(path)?)
            {
                return Ok(false);
            }
            filetime::set_file_handle_times(
                &handle,
                Some(filetime::FileTime::from_last_access_time(
                    &directory.metadata,
                )),
                Some(filetime::FileTime::from_last_modification_time(
                    &directory.metadata,
                )),
            )?;
        }
        Ok(true)
    }
}

fn same_directory_entries(
    before: &BTreeMap<OsString, Metadata>,
    after: &BTreeMap<OsString, Metadata>,
) -> bool {
    before.len() == after.len()
        && before.iter().all(|(name, original)| {
            after.get(name).is_some_and(|current| {
                same_node(original, current) && same_file_observation(original, current)
            })
        })
}

fn directory_entries(path: &Path) -> io::Result<BTreeMap<OsString, Metadata>> {
    fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            Ok((entry.file_name(), fs::symlink_metadata(entry.path())?))
        })
        .collect()
}

fn same_node(before: &Metadata, after: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.mode() == after.mode()
            && before.uid() == after.uid()
            && before.gid() == after.gid()
    }
    #[cfg(not(unix))]
    {
        let _ = (before, after);
        false
    }
}

fn same_file_observation(before: &Metadata, after: &Metadata) -> bool {
    let unchanged = before.len() == after.len()
        && filetime::FileTime::from_last_modification_time(before)
            == filetime::FileTime::from_last_modification_time(after);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        unchanged && before.ctime() == after.ctime() && before.ctime_nsec() == after.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        let _ = unchanged;
        false
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn old_time(path: &Path) -> io::Result<filetime::FileTime> {
        let time = filetime::FileTime::from_unix_time(1_500_000_000, 123_456_789);
        filetime::set_file_times(path, time, time)?;
        Ok(time)
    }

    #[test]
    fn known_source_removals_and_empty_pruning_preserve_retained_directory_times() -> io::Result<()>
    {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("source");
        let nested = root.join("nested");
        fs::create_dir_all(&nested)?;
        let source = nested.join("converted.jpg");
        let sidecar = nested.join("converted.xmp");
        fs::write(&source, b"synthetic source")?;
        fs::write(&sidecar, b"synthetic sidecar")?;
        fs::write(root.join("failed.jpg"), b"retained source")?;
        let time = old_time(&root)?;
        let snapshot =
            SourceDirectoryCleanupSnapshot::capture(&root, std::slice::from_ref(&source))?;
        fs::remove_file(&source)?;
        fs::remove_file(&sidecar)?;
        fs::remove_dir(nested)?;
        assert!(snapshot.restore_after_cleanup(&[source, sidecar])?);
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&root)?),
            time
        );
        Ok(())
    }

    #[test]
    fn unrelated_additions_edits_and_replacements_do_not_restore_old_dates() -> io::Result<()> {
        for kind in ["new", "edited", "replaced", "touched"] {
            let root = tempfile::tempdir()?;
            let source = root.path().join("converted.jpg");
            let retained = root.path().join("retained.jpg");
            fs::write(&source, b"synthetic source")?;
            fs::write(&retained, b"retained source")?;
            old_time(root.path())?;
            let snapshot = SourceDirectoryCleanupSnapshot::capture(
                root.path(),
                std::slice::from_ref(&source),
            )?;
            fs::remove_file(&source)?;
            match kind {
                "new" => fs::write(root.path().join("user.txt"), b"user edit")?,
                "edited" => fs::write(&retained, b"changed source")?,
                "replaced" => {
                    fs::rename(&retained, root.path().join("renamed.jpg"))?;
                    fs::write(&retained, b"retained source")?;
                }
                "touched" => {
                    let time = filetime::FileTime::from_unix_time(1_600_000_000, 0);
                    filetime::set_file_mtime(&retained, time)?;
                }
                _ => unreachable!(),
            }
            let observed =
                filetime::FileTime::from_last_modification_time(&fs::metadata(root.path())?);
            assert!(!snapshot.restore_after_cleanup(&[source])?, "{kind}");
            assert_eq!(
                filetime::FileTime::from_last_modification_time(&fs::metadata(root.path())?),
                observed
            );
        }
        Ok(())
    }

    #[test]
    fn edits_after_validation_do_not_restore_old_dates() -> io::Result<()> {
        for kind in ["new", "edited", "directory"] {
            let root = tempfile::tempdir()?;
            let source = root.path().join("converted.jpg");
            let retained = root.path().join("retained.jpg");
            fs::write(&source, b"synthetic source")?;
            fs::write(&retained, b"retained source")?;
            let original = old_time(root.path())?;
            let snapshot = SourceDirectoryCleanupSnapshot::capture(
                root.path(),
                std::slice::from_ref(&source),
            )?;
            fs::remove_file(&source)?;
            let user_time = filetime::FileTime::from_unix_time(1_600_000_000, 0);
            assert!(
                !snapshot.restore_after_cleanup_with(&[source], || {
                    match kind {
                        "new" => fs::write(root.path().join("user.txt"), b"user edit"),
                        "edited" => fs::write(&retained, b"changed source"),
                        "directory" => filetime::set_file_mtime(root.path(), user_time),
                        _ => unreachable!(),
                    }
                    .expect("apply synthetic concurrent edit");
                })?,
                "{kind}"
            );
            let observed =
                filetime::FileTime::from_last_modification_time(&fs::metadata(root.path())?);
            assert_ne!(observed, original, "{kind}");
            if kind == "directory" {
                assert_eq!(observed, user_time);
            }
        }
        Ok(())
    }

    #[test]
    fn absent_candidate_without_actual_removal_receipt_does_not_restore_dates() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("converted.jpg");
        fs::write(&source, b"synthetic source")?;
        let original = old_time(root.path())?;
        let snapshot =
            SourceDirectoryCleanupSnapshot::capture(root.path(), std::slice::from_ref(&source))?;
        fs::remove_file(source)?;
        assert!(!snapshot.restore_after_cleanup(&[])?);
        assert_ne!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(root.path())?),
            original
        );
        Ok(())
    }

    #[test]
    fn unrelated_empty_directory_removal_does_not_restore_dates() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("converted.jpg");
        let empty = root.path().join("empty");
        fs::write(&source, b"synthetic source")?;
        fs::create_dir(&empty)?;
        let original = old_time(root.path())?;
        let snapshot =
            SourceDirectoryCleanupSnapshot::capture(root.path(), &[source.clone(), empty.clone()])?;
        fs::remove_file(&source)?;
        fs::remove_dir(empty)?;
        assert!(!snapshot.restore_after_cleanup(&[source])?);
        assert_ne!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(root.path())?),
            original
        );
        Ok(())
    }
}

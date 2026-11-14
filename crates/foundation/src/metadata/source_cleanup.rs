use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, File, Metadata};
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FileKind {
    File,
    Directory,
    Symlink,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeTime {
    seconds: i64,
    nanoseconds: u32,
}

impl NativeTime {
    #[cfg(unix)]
    fn from_native(seconds: i64, nanoseconds: i64) -> io::Result<Self> {
        let time = Self {
            seconds,
            nanoseconds: u32::try_from(nanoseconds).map_err(io::Error::other)?,
        };
        time.validate()?;
        Ok(time)
    }

    const fn from_filetime(time: filetime::FileTime) -> Self {
        Self {
            seconds: time.unix_seconds(),
            nanoseconds: time.nanoseconds(),
        }
    }

    const fn filetime(self) -> filetime::FileTime {
        filetime::FileTime::from_unix_time(self.seconds, self.nanoseconds)
    }

    fn validate(self) -> io::Result<()> {
        if self.nanoseconds >= 1_000_000_000 {
            return Err(io::Error::other(
                "directory observation has invalid nanoseconds",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnixIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    changed: NativeTime,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeObservation {
    kind: FileKind,
    length: u64,
    accessed: NativeTime,
    modified: NativeTime,
    created: Option<NativeTime>,
    unix: Option<UnixIdentity>,
}

impl NativeObservation {
    const fn has_native_identity(&self) -> bool {
        #[cfg(unix)]
        {
            self.unix.is_some()
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    fn capture(metadata: &Metadata) -> io::Result<Self> {
        let kind = if metadata.is_dir() {
            FileKind::Directory
        } else if metadata.is_file() {
            FileKind::File
        } else if metadata.file_type().is_symlink() {
            FileKind::Symlink
        } else {
            FileKind::Other
        };
        #[cfg(unix)]
        let unix = {
            use std::os::unix::fs::MetadataExt;
            Some(UnixIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
                mode: metadata.mode(),
                uid: metadata.uid(),
                gid: metadata.gid(),
                changed: NativeTime::from_native(metadata.ctime(), metadata.ctime_nsec())?,
            })
        };
        #[cfg(not(unix))]
        let unix = None;
        Ok(Self {
            kind,
            length: metadata.len(),
            accessed: NativeTime::from_filetime(filetime::FileTime::from_last_access_time(
                metadata,
            )),
            modified: NativeTime::from_filetime(filetime::FileTime::from_last_modification_time(
                metadata,
            )),
            created: filetime::FileTime::from_creation_time(metadata)
                .map(NativeTime::from_filetime),
            unix,
        })
    }

    fn validate(&self) -> io::Result<()> {
        self.accessed.validate()?;
        self.modified.validate()?;
        if let Some(created) = self.created {
            created.validate()?;
        }
        if let Some(identity) = &self.unix {
            identity.changed.validate()?;
            #[cfg(unix)]
            {
                fn native_mode(value: impl Into<u32>) -> u32 {
                    value.into()
                }
                let mode_kind = identity.mode & native_mode(libc::S_IFMT);
                let matches_kind = match self.kind {
                    FileKind::File => mode_kind == native_mode(libc::S_IFREG),
                    FileKind::Directory => mode_kind == native_mode(libc::S_IFDIR),
                    FileKind::Symlink => mode_kind == native_mode(libc::S_IFLNK),
                    FileKind::Other => ![
                        native_mode(libc::S_IFREG),
                        native_mode(libc::S_IFDIR),
                        native_mode(libc::S_IFLNK),
                    ]
                    .contains(&mode_kind),
                };
                if !matches_kind {
                    return Err(io::Error::other(
                        "directory observation kind disagrees with native mode",
                    ));
                }
            }
        }
        Ok(())
    }
}

struct DirectorySnapshot {
    metadata: NativeObservation,
    entries: BTreeMap<OsString, NativeObservation>,
}

// JSON object keys cannot represent every native filename. Serde's OsString
// representation preserves native bytes, and ordered records let us reject
// duplicate paths/names rather than silently replacing an earlier observation.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableSnapshot {
    schema: u8,
    requested_root: OsString,
    canonical_root: OsString,
    directories: Vec<DurableDirectory>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableDirectory {
    path: OsString,
    metadata: NativeObservation,
    entries: Vec<DurableEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DurableEntry {
    name: OsString,
    metadata: NativeObservation,
}

const DURABLE_SCHEMA: u8 = 1;
const MAX_DURABLE_BYTES: usize = 256 * 1024 * 1024;

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
                let metadata = NativeObservation::capture(&metadata)?;
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

    /// Serialize the original native observations; this grants no removal or custody permission.
    pub(crate) fn to_durable_json(&self) -> io::Result<Vec<u8>> {
        self.validate_durable_scope(&self.canonical_root, &[])?;
        let snapshot = DurableSnapshot {
            schema: DURABLE_SCHEMA,
            requested_root: self.requested_root.as_os_str().to_owned(),
            canonical_root: self.canonical_root.as_os_str().to_owned(),
            directories: self
                .directories
                .iter()
                .map(|(path, directory)| DurableDirectory {
                    path: path.as_os_str().to_owned(),
                    metadata: directory.metadata.clone(),
                    entries: directory
                        .entries
                        .iter()
                        .map(|(name, metadata)| DurableEntry {
                            name: name.clone(),
                            metadata: metadata.clone(),
                        })
                        .collect(),
                })
                .collect(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(io::Error::other)?;
        if bytes.len() > MAX_DURABLE_BYTES {
            return Err(io::Error::other(
                "durable directory snapshot exceeds its size limit",
            ));
        }
        Ok(bytes)
    }

    /// Decode only the versioned observations. No current filesystem facts are inferred here.
    pub(crate) fn from_durable_json(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_DURABLE_BYTES {
            return Err(io::Error::other(
                "durable directory snapshot exceeds its size limit",
            ));
        }
        let snapshot: DurableSnapshot = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        if snapshot.schema != DURABLE_SCHEMA {
            return Err(io::Error::other(
                "unsupported durable directory snapshot schema",
            ));
        }
        let mut directories = BTreeMap::new();
        for directory in snapshot.directories {
            let mut entries = BTreeMap::new();
            for entry in directory.entries {
                if entries.insert(entry.name, entry.metadata).is_some() {
                    return Err(io::Error::other("duplicate durable directory entry name"));
                }
            }
            if directories
                .insert(
                    PathBuf::from(directory.path),
                    DirectorySnapshot {
                        metadata: directory.metadata,
                        entries,
                    },
                )
                .is_some()
            {
                return Err(io::Error::other("duplicate durable directory path"));
            }
        }
        let snapshot = Self {
            directories,
            requested_root: PathBuf::from(snapshot.requested_root),
            canonical_root: PathBuf::from(snapshot.canonical_root),
        };
        snapshot.validate_durable_scope(&snapshot.canonical_root, &[])?;
        Ok(snapshot)
    }

    /// Pure structural validation for cold database checks: never reads a media path.
    pub(crate) fn validate_durable_scope(
        &self,
        expected_root: &Path,
        known_removed: &[PathBuf],
    ) -> io::Result<()> {
        validate_absolute_path(expected_root)?;
        validate_absolute_path(&self.requested_root)?;
        validate_absolute_path(&self.canonical_root)?;
        if self.canonical_root != expected_root
            || !self.directories.contains_key(&self.canonical_root)
        {
            return Err(io::Error::other(
                "durable directory snapshot has a different root scope",
            ));
        }
        for removed in known_removed {
            validate_absolute_path(removed)?;
            let relative = removed
                .strip_prefix(&self.requested_root)
                .map_err(io::Error::other)?;
            if relative.as_os_str().is_empty() {
                return Err(io::Error::other("directory root is not a removable file"));
            }
        }
        for (path, directory) in &self.directories {
            validate_absolute_path(path)?;
            if !path.starts_with(&self.canonical_root)
                || directory.metadata.kind != FileKind::Directory
                || (path != &self.canonical_root
                    && path
                        .parent()
                        .is_none_or(|parent| !self.directories.contains_key(parent)))
            {
                return Err(io::Error::other(
                    "durable directory observation escapes its recorded scope",
                ));
            }
            directory.metadata.validate()?;
            for (name, metadata) in &directory.entries {
                let name_path = Path::new(name);
                if name_path.as_os_str().as_encoded_bytes().contains(&0)
                    || name_path.components().count() != 1
                    || !matches!(name_path.components().next(), Some(Component::Normal(_)))
                    || name_path.components().collect::<PathBuf>().as_os_str() != name.as_os_str()
                {
                    return Err(io::Error::other(
                        "durable directory entry is not one native filename",
                    ));
                }
                metadata.validate()?;
                if let Some(child) = self.directories.get(&path.join(name))
                    && (metadata.kind != child.metadata.kind
                        || metadata.length != child.metadata.length
                        || metadata.modified != child.metadata.modified
                        || metadata.created != child.metadata.created
                        || metadata.unix != child.metadata.unix)
                {
                    return Err(io::Error::other(
                        "durable child directory observations disagree",
                    ));
                }
            }
            if path != &self.canonical_root {
                let name = path.file_name().ok_or_else(|| {
                    io::Error::other("durable child directory has no native filename")
                })?;
                let parent = path
                    .parent()
                    .and_then(|parent| self.directories.get(parent));
                if parent.is_none_or(|parent| !parent.entries.contains_key(name)) {
                    return Err(io::Error::other(
                        "durable child directory has no parent entry",
                    ));
                }
            }
        }
        Ok(())
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
        if self.directories.values().any(|directory| {
            !directory.metadata.has_native_identity()
                || directory
                    .entries
                    .values()
                    .any(|entry| !entry.has_native_identity())
        }) {
            return Ok(false);
        }
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
                    let metadata = NativeObservation::capture(&metadata)?;
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
                    None if (original.kind == FileKind::File
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
            let current = NativeObservation::capture(&fs::symlink_metadata(path)?)?;
            let opened = NativeObservation::capture(&handle.metadata()?)?;
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
                Some(directory.metadata.accessed.filetime()),
                Some(directory.metadata.modified.filetime()),
            )?;
        }
        Ok(true)
    }
}

fn same_directory_entries(
    before: &BTreeMap<OsString, NativeObservation>,
    after: &BTreeMap<OsString, NativeObservation>,
) -> bool {
    before.len() == after.len()
        && before.iter().all(|(name, original)| {
            after.get(name).is_some_and(|current| {
                same_node(original, current) && same_file_observation(original, current)
            })
        })
}

fn directory_entries(path: &Path) -> io::Result<BTreeMap<OsString, NativeObservation>> {
    fs::read_dir(path)?
        .map(|entry| {
            let entry = entry?;
            Ok((
                entry.file_name(),
                NativeObservation::capture(&fs::symlink_metadata(entry.path())?)?,
            ))
        })
        .collect()
}

fn same_node(before: &NativeObservation, after: &NativeObservation) -> bool {
    #[cfg(unix)]
    {
        match (&before.unix, &after.unix) {
            (Some(before_id), Some(after_id)) => {
                before.kind == after.kind
                    && before.created == after.created
                    && before_id.device == after_id.device
                    && before_id.inode == after_id.inode
                    && before_id.mode == after_id.mode
                    && before_id.uid == after_id.uid
                    && before_id.gid == after_id.gid
            }
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (before, after);
        false
    }
}

fn same_file_observation(before: &NativeObservation, after: &NativeObservation) -> bool {
    let unchanged = before.length == after.length && before.modified == after.modified;
    #[cfg(unix)]
    {
        unchanged
            && match (&before.unix, &after.unix) {
                (Some(before), Some(after)) => before.changed == after.changed,
                _ => false,
            }
    }
    #[cfg(not(unix))]
    {
        let _ = unchanged;
        false
    }
}

fn validate_absolute_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path.file_name().is_none()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || path.components().collect::<PathBuf>().as_os_str() != path.as_os_str()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(io::Error::other(
            "durable directory paths must be normalized absolute non-root paths",
        ));
    }
    Ok(())
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
    fn durable_observations_restore_the_original_dates_after_reloading_and_keep_native_names()
    -> io::Result<()> {
        use std::os::unix::ffi::OsStringExt;

        let temporary = tempfile::tempdir()?;
        let root = temporary.path().join("source");
        fs::create_dir(&root)?;
        let source = root.join("checkpoint.json");
        let native_name = OsString::from("保留的媒体📷.jpg");
        let retained = root.join(&native_name);
        fs::write(&source, b"synthetic state")?;
        fs::write(&retained, b"retained bytes")?;
        let original_time = old_time(&root)?;
        let canonical_root = root.canonicalize()?;
        let snapshot =
            SourceDirectoryCleanupSnapshot::capture(&root, std::slice::from_ref(&source))?;
        let bytes = snapshot.to_durable_json()?;
        drop(snapshot);

        // APFS can reject non-UTF-8 names. Exercise their exact native-byte
        // representation in memory, separately from the real cleanup fixture.
        let raw_name = OsString::from_vec(vec![b'r', 0xff, b't']);
        let mut native_bytes = SourceDirectoryCleanupSnapshot::from_durable_json(&bytes)?;
        let directory = native_bytes.directories.get_mut(&canonical_root).unwrap();
        let observation = directory.entries[&native_name].clone();
        assert!(
            directory
                .entries
                .insert(raw_name.clone(), observation.clone())
                .is_none()
        );
        let native_bytes = native_bytes.to_durable_json()?;
        let native_bytes = SourceDirectoryCleanupSnapshot::from_durable_json(&native_bytes)?;
        assert_eq!(
            native_bytes.directories[&canonical_root]
                .entries
                .get(&raw_name),
            Some(&observation)
        );
        drop(native_bytes);

        // Simulate the process disappearing after cleanup but before restoration.
        fs::remove_file(&source)?;
        let recovered = SourceDirectoryCleanupSnapshot::from_durable_json(&bytes)?;
        recovered.validate_durable_scope(&canonical_root, std::slice::from_ref(&source))?;
        assert!(
            recovered.directories[&canonical_root]
                .entries
                .contains_key(&native_name)
        );
        assert!(recovered.restore_after_cleanup(std::slice::from_ref(&source))?);
        assert_eq!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(&root)?),
            original_time
        );
        assert_eq!(fs::read(&retained)?, b"retained bytes");

        // Cold SQL integrity must still be inspectable after the media tree is gone.
        fs::remove_file(&retained)?;
        fs::remove_dir(&root)?;
        SourceDirectoryCleanupSnapshot::from_durable_json(&bytes)?
            .validate_durable_scope(&canonical_root, &[source])?;
        assert!(!root.exists());
        Ok(())
    }

    #[test]
    fn reloaded_observations_still_reject_external_edits_and_replaced_directories() -> io::Result<()>
    {
        for kind in ["addition", "retained_edit", "directory_replacement"] {
            let temporary = tempfile::tempdir()?;
            let root = temporary.path().join("source");
            fs::create_dir(&root)?;
            let source = root.join("removed.json");
            let retained = root.join("retained.jpg");
            fs::write(&source, b"synthetic state")?;
            fs::write(&retained, b"retained bytes")?;
            let original = old_time(&root)?;
            let bytes =
                SourceDirectoryCleanupSnapshot::capture(&root, std::slice::from_ref(&source))?
                    .to_durable_json()?;
            fs::remove_file(&source)?;
            match kind {
                "addition" => fs::write(root.join("new.txt"), b"external edit")?,
                "retained_edit" => fs::write(&retained, b"different retained bytes")?,
                "directory_replacement" => {
                    fs::rename(&root, temporary.path().join("original-source"))?;
                    fs::create_dir(&root)?;
                    fs::write(&retained, b"retained bytes")?;
                }
                _ => unreachable!(),
            }
            let before = filetime::FileTime::from_last_modification_time(&fs::metadata(&root)?);
            assert!(
                !SourceDirectoryCleanupSnapshot::from_durable_json(&bytes)?
                    .restore_after_cleanup(&[source])?,
                "{kind}"
            );
            assert_ne!(before, original);
            assert_eq!(
                filetime::FileTime::from_last_modification_time(&fs::metadata(&root)?),
                before
            );
        }
        Ok(())
    }

    #[test]
    fn durable_schema_scope_duplicates_and_native_times_are_strictly_validated() -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("removed.json");
        fs::write(&source, b"synthetic state")?;
        let snapshot = SourceDirectoryCleanupSnapshot::capture(
            temporary.path(),
            std::slice::from_ref(&source),
        )?;
        let bytes = snapshot.to_durable_json()?;
        let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        let mut invalid = Vec::new();
        let mut changed = value.clone();
        changed["schema"] = serde_json::json!(2);
        invalid.push(changed);
        let mut changed = value.clone();
        changed["unknown_future_requirement"] = serde_json::json!(true);
        invalid.push(changed);
        let mut changed = value.clone();
        changed["directories"][0]["metadata"]["unknown_native_fact"] = serde_json::json!(true);
        invalid.push(changed);
        let mut changed = value.clone();
        let duplicate = changed["directories"][0].clone();
        changed["directories"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        invalid.push(changed);
        let mut changed = value.clone();
        let duplicate = changed["directories"][0]["entries"][0].clone();
        changed["directories"][0]["entries"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        invalid.push(changed);
        let mut changed = value.clone();
        changed["directories"][0]["metadata"]["modified"]["nanoseconds"] =
            serde_json::json!(1_000_000_000u32);
        invalid.push(changed);
        let mut changed = value.clone();
        changed["directories"][0]["metadata"]["unix"]["changed"]["nanoseconds"] =
            serde_json::json!(-1);
        invalid.push(changed);
        let mut changed = value.clone();
        changed["directories"][0]["metadata"]["kind"] = serde_json::json!("file");
        invalid.push(changed);
        let mut changed = value.clone();
        changed["directories"][0]["path"] =
            serde_json::to_value(OsString::from("/unrelated/directory"))
                .map_err(io::Error::other)?;
        invalid.push(changed);
        let mut changed = value;
        changed["directories"][0]["entries"][0]["name"] =
            serde_json::to_value(OsString::from("../escape")).map_err(io::Error::other)?;
        invalid.push(changed);
        for changed in invalid {
            assert!(
                SourceDirectoryCleanupSnapshot::from_durable_json(
                    &serde_json::to_vec(&changed).map_err(io::Error::other)?,
                )
                .is_err(),
                "{changed}"
            );
        }
        let duplicated_schema = std::str::from_utf8(&bytes).unwrap().replacen(
            "\"schema\":1",
            "\"schema\":1,\"schema\":1",
            1,
        );
        assert!(
            SourceDirectoryCleanupSnapshot::from_durable_json(duplicated_schema.as_bytes())
                .is_err()
        );
        assert!(
            snapshot
                .validate_durable_scope(&temporary.path().join("other"), &[])
                .is_err()
        );
        assert!(
            snapshot
                .validate_durable_scope(
                    &snapshot.canonical_root,
                    &[temporary.path().join("../outside")]
                )
                .is_err()
        );
        assert!(NativeTime::from_native(1, -1).is_err());
        assert!(NativeTime::from_native(1, 1_000_000_000).is_err());
        Ok(())
    }

    #[test]
    fn durable_observations_without_native_identity_cannot_authorize_time_restoration()
    -> io::Result<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("removed.json");
        fs::write(&source, b"synthetic state")?;
        let original = old_time(temporary.path())?;
        let snapshot = SourceDirectoryCleanupSnapshot::capture(
            temporary.path(),
            std::slice::from_ref(&source),
        )?;
        let mut value: serde_json::Value =
            serde_json::from_slice(&snapshot.to_durable_json()?).map_err(io::Error::other)?;
        value["directories"][0]["metadata"]["unix"] = serde_json::Value::Null;
        let bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
        fs::remove_file(&source)?;
        assert!(
            !SourceDirectoryCleanupSnapshot::from_durable_json(&bytes)?
                .restore_after_cleanup(&[source])?
        );
        assert_ne!(
            filetime::FileTime::from_last_modification_time(&fs::metadata(temporary.path())?),
            original
        );
        Ok(())
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

use super::XmpSidecarIndex;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

pub(super) struct DirectoryView {
    metadata: Metadata,
    membership: blake3::Hash,
    candidates: BTreeMap<String, Vec<PathBuf>>,
}

impl XmpSidecarIndex {
    pub(crate) fn capture(paths: &[PathBuf]) -> io::Result<Self> {
        let mut directories = BTreeMap::new();
        for path in paths {
            let parent = crate::media_conversion_gate::path_parent_or_dot(path).to_path_buf();
            if let std::collections::btree_map::Entry::Vacant(entry) = directories.entry(parent) {
                let view = DirectoryView::read(entry.key())?;
                entry.insert(Mutex::new(view));
            }
        }
        Ok(Self { directories })
    }

    pub(crate) fn find(&self, src: &Path) -> io::Result<Option<PathBuf>> {
        let (stem, full_name) = super::xmp_source_names(src)?;
        let parent = crate::media_conversion_gate::path_parent_or_dot(src);
        let view = self.checked_directory(parent, false)?;
        let mut selected: Option<PathBuf> = None;
        for name in [&stem, &full_name] {
            for candidate in view.candidates.get(name).into_iter().flatten() {
                let path = super::require_regular_sidecar(candidate.clone(), "XMP")?;
                if let Some(previous) = &selected {
                    if previous == &path {
                        continue;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "ambiguous XMP sidecars for {}: {} and {}; originals retained",
                            src.display(),
                            previous.display(),
                            path.display()
                        ),
                    ));
                }
                selected = Some(path);
            }
        }
        Ok(selected)
    }

    /// Must finish before the phase can authorize delivery or reuse its results.
    pub(crate) fn verify_unchanged(&self) -> io::Result<()> {
        for directory in self.directories.keys() {
            drop(self.checked_directory(directory, true)?);
        }
        Ok(())
    }

    fn checked_directory(
        &self,
        parent: &Path,
        full_scan: bool,
    ) -> io::Result<MutexGuard<'_, DirectoryView>> {
        let directory = self.directories.get(parent).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "XMP directory is outside this phase snapshot",
            )
        })?;
        let mut view = directory
            .lock()
            .map_err(|_| io::Error::other("XMP directory snapshot lock is poisoned"))?;
        let current = fs::metadata(parent)?;
        if !same_directory(&view.metadata, &current) {
            return Err(io::Error::other(format!(
                "XMP directory identity changed or is unavailable: {}; phase evidence retained",
                parent.display()
            )));
        }
        // Native change stamps trigger a scan; only fresh membership confirms the namespace.
        if full_scan || !same_change_stamp(&view.metadata, &current) {
            let fresh = DirectoryView::read(parent)?;
            if !same_directory(&view.metadata, &fresh.metadata)
                || view.membership != fresh.membership
            {
                return Err(io::Error::other(format!(
                    "XMP candidate names or file identities changed under {}; restart verification with a fresh namespace snapshot",
                    parent.display()
                )));
            }
            *view = fresh;
        }
        Ok(view)
    }
}

impl DirectoryView {
    fn read(parent: &Path) -> io::Result<Self> {
        let before = fs::metadata(parent)?;
        let mut names = BTreeSet::new();
        let mut candidates: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            let path = entry.path();
            if path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp"))
            {
                names.insert(entry.file_name());
                if let Some(key) = super::xmp_candidate_key(&path) {
                    candidates.entry(key).or_default().push(path);
                }
            }
        }
        let mut membership = blake3::Hasher::new();
        for name in names {
            let bytes = name.as_encoded_bytes();
            membership.update(
                &u64::try_from(bytes.len())
                    .map_err(io::Error::other)?
                    .to_le_bytes(),
            );
            membership.update(bytes);
            update_candidate_identity(&mut membership, &parent.join(name))?;
        }
        let metadata = fs::metadata(parent)?;
        if !same_directory(&before, &metadata) || !same_change_stamp(&before, &metadata) {
            return Err(io::Error::other(format!(
                "Directory changed during XMP namespace discovery: {}; no stable snapshot was obtained",
                parent.display()
            )));
        }
        Ok(Self {
            metadata,
            membership: membership.finalize(),
            candidates,
        })
    }
}

fn update_candidate_identity(hash: &mut blake3::Hasher, path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    hash.update(&[
        u8::from(metadata.is_file()),
        u8::from(metadata.is_dir()),
        u8::from(metadata.file_type().is_symlink()),
    ]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        hash.update(&metadata.dev().to_le_bytes());
        hash.update(&metadata.ino().to_le_bytes());
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let volume = metadata
            .volume_serial_number()
            .ok_or_else(|| io::Error::other("XMP candidate volume identity is unavailable"))?;
        let index = metadata
            .file_index()
            .ok_or_else(|| io::Error::other("XMP candidate file identity is unavailable"))?;
        hash.update(&volume.to_le_bytes());
        hash.update(&index.to_le_bytes());
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "XMP candidate native identity is unavailable",
        ))
    }
}

fn same_directory(left: &Metadata, right: &Metadata) -> bool {
    if !left.is_dir() || !right.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        match (
            left.volume_serial_number(),
            left.file_index(),
            right.volume_serial_number(),
            right.file_index(),
        ) {
            (Some(ld), Some(li), Some(rd), Some(ri)) => ld == rd && li == ri,
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn same_change_stamp(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (left.ctime(), left.ctime_nsec()) == (right.ctime(), right.ctime_nsec())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        left.last_write_time() == right.last_write_time()
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_lookup_matches_live_discovery_without_per_file_directory_scans() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let mut paths = Vec::new();
        for ordinal in 0..256 {
            let path = root.path().join(format!("synthetic-{ordinal}.jpg"));
            fs::write(&path, [])?;
            paths.push(path);
        }
        fs::write(root.path().join("synthetic-3.JPG.XMP"), b"synthetic XMP")?;
        let started = std::time::Instant::now();
        let expected = paths
            .iter()
            .map(|path| crate::metadata::find_xmp_sidecar(path))
            .collect::<io::Result<Vec<_>>>()?;
        let live_elapsed = started.elapsed();
        let started = std::time::Instant::now();
        let index = XmpSidecarIndex::capture(&paths)?;
        let actual = paths
            .iter()
            .map(|path| index.find(path))
            .collect::<io::Result<Vec<_>>>()?;
        index.verify_unchanged()?;
        assert_eq!(actual, expected);
        println!(
            "XMP lookup 256 siblings: live={live_elapsed:?}, indexed={:?}",
            started.elapsed()
        );
        Ok(())
    }

    #[test]
    fn unrelated_entries_refresh_the_hint_but_new_xmp_invalidates_the_phase() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("synthetic.jpg");
        let index = XmpSidecarIndex::capture(std::slice::from_ref(&source))?;
        fs::write(root.path().join(".DS_Store"), b"synthetic finder data")?;
        assert_eq!(index.find(&source)?, None);
        index.verify_unchanged()?;
        let modified = fs::metadata(root.path())?.modified()?;
        fs::write(root.path().join("synthetic.jpg.xmp"), b"new sidecar")?;
        filetime::set_file_mtime(root.path(), filetime::FileTime::from_system_time(modified))?;
        assert!(index.verify_unchanged().is_err());
        let fresh = XmpSidecarIndex::capture(std::slice::from_ref(&source))?;
        assert_eq!(
            fresh.find(&source)?,
            Some(root.path().join("synthetic.jpg.xmp"))
        );
        Ok(())
    }

    #[test]
    fn final_namespace_verification_does_not_trust_the_change_hint() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("synthetic.jpg");
        let index = XmpSidecarIndex::capture(std::slice::from_ref(&source))?;
        fs::write(root.path().join("synthetic.xmp"), b"new sidecar")?;
        let mut view = index.directories[root.path()].lock().unwrap();
        view.metadata = fs::metadata(root.path())?;
        drop(view);
        assert!(index.verify_unchanged().is_err());
        Ok(())
    }

    #[test]
    fn indexed_discovery_keeps_compound_ambiguity_and_link_rejection() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("synthetic.jpg");
        let sidecar = root.path().join("synthetic.xmp");
        fs::write(&sidecar, b"one")?;
        let bare_source = root.path().join("synthetic");
        let bare_index = XmpSidecarIndex::capture(std::slice::from_ref(&bare_source))?;
        assert_eq!(bare_index.find(&bare_source)?, Some(sidecar.clone()));
        fs::write(root.path().join("synthetic.jpg.XMP"), b"two")?;
        let index = XmpSidecarIndex::capture(std::slice::from_ref(&source))?;
        assert!(index.find(&source).is_err());
        assert!(crate::metadata::find_xmp_sidecar(&source).is_err());
        fs::remove_file(root.path().join("synthetic.jpg.XMP"))?;
        #[cfg(unix)]
        {
            let index = XmpSidecarIndex::capture(std::slice::from_ref(&source))?;
            fs::remove_file(&sidecar)?;
            std::os::unix::fs::symlink(root.path().join("missing-target"), &sidecar)?;
            assert!(index.find(&source).is_err());
        }
        Ok(())
    }
}

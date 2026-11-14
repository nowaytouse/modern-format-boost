//! Fast IMG namespace observation shared by the launcher and worker.

use super::AdmittedImageFile;
use anyhow::{Context, Result, ensure};
use std::fs;
use std::path::{Path, PathBuf};

/// The selected regular files and the directory identities observed during traversal.
/// Media content and delivery checks remain the responsibility of the worker.
pub struct FastImgInputScope {
    pub src_root: PathBuf,
    pub candidates: Vec<AdmittedImageFile>,
    pub selected_is_directory: bool,
    token: String,
    directory_witness: FastImgDirectoryWitness,
}

/// Directory identities from a scan, retained through media classification.
pub struct FastImgDirectoryWitness {
    input: PathBuf,
    selected: PathBuf,
    root: PathBuf,
    root_stamp: DirectoryStamp,
    descendants: Vec<(PathBuf, DirectoryStamp)>,
}

impl FastImgDirectoryWitness {
    /// Recheck observed directories without enumerating their entries again.
    ///
    /// # Errors
    /// Returns an error if a directory or selected path changed or cannot be read.
    pub fn verify_unchanged(&self) -> Result<()> {
        for (directory, stamp) in self.descendants.iter().rev() {
            verify_directory_stamp(directory, stamp)?;
        }
        // Recheck the root after all its descendants.
        verify_directory_stamp(&self.root, &self.root_stamp)?;
        ensure!(
            fs::canonicalize(&self.input)? == self.selected,
            "fast-img selected input changed while observing its inventory"
        );
        Ok(())
    }
}

impl FastImgInputScope {
    pub fn source_root(input: &Path) -> Result<PathBuf> {
        let selected = crate::media_conversion_gate::canonicalize_for_tool_input(input);
        let metadata = fs::metadata(&selected)
            .with_context(|| format!("inspect fast-img selected input {}", selected.display()))?;
        Ok(source_root(&selected, &metadata)?.to_path_buf())
    }

    /// Observe regular-file membership without reading media bytes.
    pub fn observe(input: &Path, recursive: bool) -> Result<Self> {
        let selected = fs::canonicalize(input)
            .with_context(|| format!("resolve fast-img selected input {}", input.display()))?;
        let metadata = fs::symlink_metadata(&selected)
            .with_context(|| format!("inspect fast-img selected input {}", selected.display()))?;
        let src_root = source_root(&selected, &metadata)?.to_path_buf();
        let selected_is_directory = metadata.is_dir();
        let root_stamp = if selected_is_directory {
            DirectoryStamp::from_metadata(&metadata)?
        } else {
            directory_stamp(&src_root)?
        };
        let mut descendants = Vec::new();
        let candidates = if metadata.is_file() {
            vec![AdmittedImageFile::from_metadata(&selected, &metadata)?]
        } else {
            observe_regular_files(&src_root, recursive, &mut descendants)?
        };
        descendants.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        let directory_witness = FastImgDirectoryWitness {
            input: input.to_path_buf(),
            selected,
            root: src_root.clone(),
            root_stamp,
            descendants,
        };
        directory_witness.verify_unchanged()?;
        let mut hasher =
            blake3::Hasher::new_derive_key("Modern Format Boost Fast IMG input scope v2");
        hash_path(&mut hasher, &directory_witness.selected)?;
        hash_path(&mut hasher, &src_root)?;
        hasher.update(&[u8::from(recursive), u8::from(selected_is_directory)]);
        directory_witness.root_stamp.hash_identity(&mut hasher);
        hasher.update(&u64::try_from(directory_witness.descendants.len())?.to_le_bytes());
        for (directory, stamp) in &directory_witness.descendants {
            hash_path(&mut hasher, directory.strip_prefix(&src_root)?)?;
            stamp.hash_identity(&mut hasher);
        }
        hasher.update(&u64::try_from(candidates.len())?.to_le_bytes());
        for candidate in &candidates {
            hash_path(&mut hasher, candidate.path().strip_prefix(&src_root)?)?;
            let identity = &candidate.identity;
            hasher.update(&identity.size.to_le_bytes());
            hash_modified_time(&mut hasher, identity.modified_time);
            #[cfg(unix)]
            {
                hasher.update(&identity.device.to_le_bytes());
                hasher.update(&identity.inode.to_le_bytes());
                hasher.update(&identity.changed_time.0.to_le_bytes());
                hasher.update(&identity.changed_time.1.to_le_bytes());
            }
        }
        Ok(Self {
            src_root,
            candidates,
            selected_is_directory,
            token: hasher.finalize().to_hex().to_string(),
            directory_witness,
        })
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Transfer the observed files and the witness for their containing directories.
    #[must_use]
    pub fn into_scan(self) -> (Vec<AdmittedImageFile>, FastImgDirectoryWitness) {
        (self.candidates, self.directory_witness)
    }
}

fn source_root<'a>(selected: &'a Path, metadata: &fs::Metadata) -> Result<&'a Path> {
    if metadata.is_file() {
        return selected
            .parent()
            .context("fast-img single-file input has no parent");
    }
    ensure!(
        metadata.is_dir(),
        "fast-img selected input is not a file or directory"
    );
    Ok(selected)
}

fn hash_path(hasher: &mut blake3::Hasher, path: &Path) -> Result<()> {
    let bytes = path.as_os_str().as_encoded_bytes();
    hasher.update(&u64::try_from(bytes.len())?.to_le_bytes());
    hasher.update(bytes);
    Ok(())
}

fn hash_modified_time(hasher: &mut blake3::Hasher, modified: std::time::SystemTime) {
    let (before_epoch, duration) = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => (false, duration),
        Err(error) => (true, error.duration()),
    };
    hasher.update(&[u8::from(before_epoch)]);
    hasher.update(&duration.as_secs().to_le_bytes());
    hasher.update(&duration.subsec_nanos().to_le_bytes());
}

fn observe_regular_files(
    root: &Path,
    recursive: bool,
    directories: &mut Vec<(PathBuf, DirectoryStamp)>,
) -> Result<Vec<AdmittedImageFile>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        crate::batch_control::checkpoint()?;
        if directory != root {
            directories.push((directory.clone(), directory_stamp(&directory)?));
        }
        let entries = fs::read_dir(&directory)
            .with_context(|| format!("fast-img scan read dir {}", directory.display()))?;
        for entry in entries {
            let entry = entry
                .with_context(|| format!("fast-img scan entry under {}", directory.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("fast-img scan file type {}", path.display()))?;
            if file_type.is_dir() && recursive {
                pending.push(path);
            } else if file_type.is_file() {
                let metadata = entry
                    .metadata()
                    .with_context(|| format!("observe fast-img scan file {}", path.display()))?;
                files.push(AdmittedImageFile::from_metadata(&path, &metadata)?);
            }
        }
    }
    files.sort_unstable_by(|left, right| left.path().cmp(right.path()));
    Ok(files)
}

fn verify_directory_stamp(directory: &Path, stamp: &DirectoryStamp) -> Result<()> {
    crate::batch_control::checkpoint()?;
    ensure!(
        directory_stamp(directory)? == *stamp,
        "fast-img scan directory changed while observing inventory: {}",
        directory.display()
    );
    Ok(())
}

#[derive(PartialEq, Eq)]
struct DirectoryStamp {
    modified: std::time::SystemTime,
    #[cfg(unix)]
    native: (u64, u64, i64, i64),
}

fn directory_stamp(path: &Path) -> Result<DirectoryStamp> {
    ensure!(
        fs::canonicalize(path)? == path,
        "fast-img scan directory resolved through a changed link: {}",
        path.display()
    );
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("observe fast-img scan directory {}", path.display()))?;
    ensure!(
        metadata.is_dir(),
        "fast-img scan path changed to a non-directory: {}",
        path.display()
    );
    DirectoryStamp::from_metadata(&metadata)
}

impl DirectoryStamp {
    fn hash_identity(&self, hasher: &mut blake3::Hasher) {
        #[cfg(unix)]
        {
            hasher.update(&self.native.0.to_le_bytes());
            hasher.update(&self.native.1.to_le_bytes());
        }
        #[cfg(not(unix))]
        hash_modified_time(hasher, self.modified);
    }

    fn from_metadata(metadata: &fs::Metadata) -> Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Ok(Self {
            modified: metadata.modified()?,
            #[cfg(unix)]
            native: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_img_input_scope_detects_added_and_changed_regular_files() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        let source = root.join("媒体\nsource");
        fs::write(&source, b"one")?;
        let first = FastImgInputScope::observe(root, true)?;
        assert_eq!(first.token().len(), 64);
        assert_eq!(
            first.token(),
            FastImgInputScope::observe(root, true)?.token()
        );

        fs::write(root.join("late"), b"new file")?;
        let second = FastImgInputScope::observe(root, true)?;
        assert_ne!(first.token(), second.token());
        assert_eq!(first.candidates.len(), 1);
        assert_eq!(second.candidates.len(), 2);

        fs::write(&source, b"changed source")?;
        let third = FastImgInputScope::observe(root, true)?;
        assert_ne!(second.token(), third.token());
        assert_eq!(third.candidates.len(), second.candidates.len());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fast_img_input_scope_refuses_replaced_ancestors_with_the_same_media() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path().canonicalize()?.join("source");
        let nested = root.join("nested");
        fs::create_dir_all(nested.join("media"))?;
        fs::write(nested.join("media/item.jpg"), b"synthetic media")?;
        let parked = root.with_extension("parked");
        let original = FastImgInputScope::observe(&root, true)?;
        let root_stamp = directory_stamp(&root)?;

        fs::rename(&root, &parked)?;
        fs::create_dir(&root)?;
        fs::rename(parked.join("nested"), &nested)?;
        assert!(verify_directory_stamp(&root, &root_stamp).is_err());
        let replaced_root = FastImgInputScope::observe(&root, true)?;
        assert!(original.candidates[0].identity == replaced_root.candidates[0].identity);
        assert_ne!(original.token(), replaced_root.token());
        fs::remove_dir(&parked)?;

        let shallow = FastImgInputScope::observe(&root, false)?;
        let nested_stamp = directory_stamp(&nested)?;
        fs::rename(&nested, &parked)?;
        fs::create_dir(&nested)?;
        fs::rename(parked.join("media"), nested.join("media"))?;
        assert!(verify_directory_stamp(&nested, &nested_stamp).is_err());
        let replaced_nested = FastImgInputScope::observe(&root, true)?;
        assert!(original.candidates[0].identity == replaced_nested.candidates[0].identity);
        assert_ne!(replaced_root.token(), replaced_nested.token());
        assert_eq!(
            shallow.token(),
            FastImgInputScope::observe(&root, false)?.token()
        );
        Ok(())
    }

    #[test]
    fn fast_img_input_scope_binds_single_file_and_recursive_selection() -> Result<()> {
        let scratch = tempfile::tempdir()?;
        let root = scratch.path();
        let source = root.join("source");
        fs::write(&source, b"top")?;
        fs::create_dir(root.join("nested"))?;
        fs::write(root.join("nested/item"), b"nested")?;
        let selected = FastImgInputScope::observe(&source, false)?;
        let direct = FastImgInputScope::observe(root, false)?;
        let recursive = FastImgInputScope::observe(root, true)?;
        assert!(!selected.selected_is_directory);
        assert!(direct.selected_is_directory);
        assert_eq!(selected.src_root, root.canonicalize()?);
        assert_eq!(selected.candidates[0].path(), source.canonicalize()?);
        assert_eq!(direct.candidates.len(), 1);
        assert_eq!(recursive.candidates.len(), 2);
        assert_ne!(selected.token(), direct.token());
        assert_ne!(direct.token(), recursive.token());
        Ok(())
    }
}

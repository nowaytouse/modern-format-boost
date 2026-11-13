//! Fast IMG namespace observation shared by the launcher and worker.

use super::AdmittedImageFile;
use anyhow::{Context, Result, ensure};
use std::fs;
use std::path::{Path, PathBuf};

/// The selected regular files and the native metadata observed during traversal.
/// Media content and delivery checks remain the responsibility of the worker.
pub struct FastImgInputScope {
    pub src_root: PathBuf,
    pub candidates: Vec<AdmittedImageFile>,
    pub selected_is_directory: bool,
    token: String,
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
        let candidates = if metadata.is_file() {
            vec![AdmittedImageFile::from_metadata(&selected, &metadata)?]
        } else {
            observe_regular_files(&src_root, recursive)?
        };
        ensure!(
            fs::canonicalize(input)? == selected,
            "fast-img selected input changed while observing its inventory"
        );
        let mut hasher =
            blake3::Hasher::new_derive_key("Modern Format Boost Fast IMG input scope v1");
        hash_path(&mut hasher, &selected)?;
        hash_path(&mut hasher, &src_root)?;
        hasher.update(&[u8::from(recursive), u8::from(selected_is_directory)]);
        hasher.update(&u64::try_from(candidates.len())?.to_le_bytes());
        for candidate in &candidates {
            hash_path(&mut hasher, candidate.path().strip_prefix(&src_root)?)?;
            let identity = &candidate.identity;
            hasher.update(&identity.size.to_le_bytes());
            let (before_epoch, modified) =
                match identity.modified_time.duration_since(std::time::UNIX_EPOCH) {
                    Ok(duration) => (false, duration),
                    Err(error) => (true, error.duration()),
                };
            hasher.update(&[u8::from(before_epoch)]);
            hasher.update(&modified.as_secs().to_le_bytes());
            hasher.update(&modified.subsec_nanos().to_le_bytes());
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
        })
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
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

fn observe_regular_files(root: &Path, recursive: bool) -> Result<Vec<AdmittedImageFile>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        crate::batch_control::checkpoint()?;
        let stamp = directory_stamp(&directory)?;
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
        ensure!(
            directory_stamp(&directory)? == stamp,
            "fast-img scan directory changed while observing inventory: {}",
            directory.display()
        );
    }
    files.sort_unstable_by(|left, right| left.path().cmp(right.path()));
    Ok(files)
}

#[derive(PartialEq, Eq)]
struct DirectoryStamp {
    modified: std::time::SystemTime,
    #[cfg(unix)]
    native: (u64, u64, i64, i64),
}

fn directory_stamp(path: &Path) -> Result<DirectoryStamp> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;

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
    Ok(DirectoryStamp {
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

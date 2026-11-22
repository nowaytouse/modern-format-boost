//! Descriptor-bound reads of Darwin quarantine metadata.

#![expect(
    clippy::redundant_pub_crate,
    reason = "Shared within the crate only. The unreachable_pub lint requires explicit restricted visibility."
)]

pub(crate) const ATTRIBUTE: &str = "com.apple.quarantine";

#[cfg(target_os = "macos")]
use std::{
    borrow::Cow,
    fs::{File, Metadata},
    io,
    os::unix::fs::MetadataExt as _,
    path::Path,
};

#[cfg(target_os = "macos")]
pub(crate) fn open_regular_quarantine_file(path: &Path) -> io::Result<File> {
    crate::io_utils::open_regular_nofollow(&native_metadata_path(path)?)
}

#[cfg(target_os = "macos")]
pub(super) fn native_metadata_path(path: &Path) -> io::Result<Cow<'_, Path>> {
    for (alias, target) in [("/var", "/private/var"), ("/tmp", "/private/tmp")] {
        let Ok(rest) = path.strip_prefix(alias) else {
            continue;
        };
        let link = std::fs::read_link(alias)?;
        if link != Path::new(target) && link != Path::new(target.trim_start_matches('/')) {
            return Err(io::Error::other("unexpected Darwin root directory alias"));
        }
        return Ok(Cow::Owned(Path::new(target).join(rest)));
    }
    Ok(Cow::Borrowed(path))
}

#[cfg(target_os = "macos")]
pub(crate) fn read_quarantine(file: &File) -> io::Result<Option<Vec<u8>>> {
    use xattr::FileExt as _;
    file.get_xattr(ATTRIBUTE)
}

#[cfg(target_os = "macos")]
pub(crate) fn verify_quarantine_file_identity(
    path: &Path,
    file: &File,
    before: &Metadata,
) -> io::Result<()> {
    let opened = file.metadata()?;
    let named = open_regular_quarantine_file(path)?.metadata()?;
    crate::io_utils::check_open_file_identity(before, &opened)?;
    crate::io_utils::check_open_file_identity(before, &named)?;
    if !opened.is_file() || opened.nlink() != before.nlink() || named.nlink() != before.nlink() {
        return Err(io::Error::other(
            "quarantine observation file identity changed",
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct UnverifiedMetadataCopy(io::Error);

#[cfg(target_os = "macos")]
impl std::fmt::Display for UnverifiedMetadataCopy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "could not verify quarantine during metadata copy: {}",
            self.0
        )
    }
}

#[cfg(target_os = "macos")]
impl std::error::Error for UnverifiedMetadataCopy {}

#[cfg(target_os = "macos")]
pub(super) fn unverified_copy(error: io::Error) -> io::Error {
    io::Error::new(error.kind(), UnverifiedMetadataCopy(error))
}

#[cfg(target_os = "macos")]
pub(super) fn is_unverified_copy(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(<dyn std::error::Error + Send + Sync>::is::<UnverifiedMetadataCopy>)
}

#[cfg(target_os = "macos")]
pub(super) fn preserving_quarantine(
    destination: &File,
    copy: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let before = read_quarantine(destination).map_err(unverified_copy)?;
    let result = copy();
    if read_quarantine(destination).map_err(unverified_copy)? != before {
        return Err(unverified_copy(io::Error::other(
            "destination quarantine changed during metadata copy",
        )));
    }
    result
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fs;
    use xattr::FileExt as _;

    #[test]
    fn quarantine_descriptor_rejects_symlinks_and_path_replacement() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let named = root.path().join("named");
        let detached = root.path().join("detached");
        fs::write(&named, b"fixture")?;
        let file = open_regular_quarantine_file(&named)?;
        let before = file.metadata()?;
        verify_quarantine_file_identity(&named, &file, &before)?;
        fs::rename(&named, &detached)?;
        fs::write(&named, b"fixture")?;
        assert!(verify_quarantine_file_identity(&named, &file, &before).is_err());
        let parent_link = root.path().join("parent-link");
        std::os::unix::fs::symlink(root.path().canonicalize()?, &parent_link)?;
        assert!(open_regular_quarantine_file(&parent_link.join("named")).is_err());
        Ok(())
    }

    #[test]
    fn quarantine_guard_preserves_unexpected_destination_change() -> io::Result<()> {
        let output = tempfile::NamedTempFile::new()?;
        let destination = open_regular_quarantine_file(output.path())?;
        destination.set_xattr(ATTRIBUTE, b"0081;61ee20ff;SyntheticBefore;")?;
        let changed = b"0081;61ee21ff;SyntheticConcurrent;";
        let result =
            preserving_quarantine(&destination, || destination.set_xattr(ATTRIBUTE, changed));
        assert!(is_unverified_copy(
            &result.expect_err("destination mark changed")
        ));
        assert_eq!(
            read_quarantine(&destination)?.as_deref(),
            Some(changed.as_slice())
        );
        Ok(())
    }
}

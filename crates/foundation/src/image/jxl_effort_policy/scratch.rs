//! Durable reservation for an identified worker's full JPEG effort search.

use anyhow::Result;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::{fs, io};

#[cfg(any(target_os = "macos", target_os = "linux"))]
const PREFIX: &str = "mfb-jpeg-full-effort-";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const OWNER_FILE: &str = ".mfb-jpeg-owner.json";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MAX_OWNER_BYTES: u64 = 2048;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MAX_ROOT_ENTRIES: usize = 4096;
#[cfg(any(target_os = "macos", target_os = "linux"))]
const MAX_INSPECTED_DIRECTORIES: usize = 128;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema_version: u8,
    role: String,
    run_id: Option<String>,
    invocation_id: String,
    source_blake3: String,
    directory_name: String,
    device: u64,
    inode: u64,
}

#[derive(Debug, Serialize)]
pub struct JpegScratchObservation {
    schema_version: u8,
    kind: &'static str,
    owner: Owner,
    candidate_count: usize,
    candidate_bytes: u64,
    candidate_integrity: &'static str,
    disposition: &'static str,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn directory_name(directory: &Path) -> Result<&str> {
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid JPEG scratch name")?;
    ensure!(
        name.strip_prefix(PREFIX).is_some_and(valid_id),
        "Invalid JPEG scratch name"
    );
    Ok(name)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn open_scratch_parent(path: &Path) -> Result<fs::File> {
    use std::os::unix::fs::MetadataExt;

    let parent = crate::io_utils::open_directory_nofollow(path)?;
    let metadata = parent.metadata()?;
    // SAFETY: geteuid takes no arguments and reads the process identity.
    let uid = unsafe { libc::geteuid() };
    ensure!(
        metadata.uid() == uid && metadata.mode() & 0o022 == 0,
        "JPEG effort scratch parent must be writable only by its owner"
    );
    Ok(parent)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn open_scratch(directory: &Path) -> Result<(fs::File, fs::File)> {
    use std::os::unix::fs::MetadataExt;

    let expected = crate::process_lock::inspect_mfb_root_path()?
        .canonicalize()?
        .join("tmp");
    ensure!(
        directory.is_absolute() && directory.parent() == Some(expected.as_path()),
        "JPEG effort scratch must be directly inside the formal temporary directory"
    );
    let name = directory_name(directory)?;
    let parent = open_scratch_parent(&expected)?;
    let parent_metadata = parent.metadata()?;
    let uid = parent_metadata.uid();
    let handle = crate::io_utils::open_entry_nofollow(&parent, std::ffi::OsStr::new(name), true)?;
    let metadata = handle.metadata()?;
    ensure!(
        metadata.uid() == uid
            && metadata.mode() & 0o777 == 0o700
            && metadata.dev() == parent_metadata.dev(),
        "JPEG effort scratch directory must be private and on its parent's filesystem"
    );
    Ok((parent, handle))
}

pub(super) fn reserve(
    directory: &Path,
    source_blake3: &str,
    run_id: Option<&str>,
    invocation_id: &str,
) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::io::Write;
        use std::os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::MetadataExt,
        };

        ensure!(
            valid_id(invocation_id) && run_id.is_none_or(valid_id),
            "Invalid JPEG scratch worker identity"
        );
        let root = crate::process_lock::inspect_mfb_root_path()?;
        ensure!(
            directory.parent() == Some(root.join("tmp").as_path()),
            "JPEG effort scratch must use the formal temporary directory"
        );
        let directory = root
            .canonicalize()?
            .join("tmp")
            .join(directory_name(directory)?);
        let (parent, handle) = open_scratch(&directory)?;
        let identity = handle.metadata()?;
        let owner = Owner {
            schema_version: 1,
            role: "jpeg_full_effort".into(),
            run_id: run_id.map(str::to_owned),
            invocation_id: invocation_id.to_owned(),
            source_blake3: source_blake3.to_owned(),
            directory_name: directory_name(&directory)?.to_owned(),
            device: identity.dev(),
            inode: identity.ino(),
        };
        let payload = serde_json::to_vec(&owner)?;
        ensure!(
            u64::try_from(payload.len())? <= MAX_OWNER_BYTES,
            "JPEG scratch ownership record is too large"
        );
        let name = c".mfb-jpeg-owner.json";
        // SAFETY: the directory handle and fixed NUL-terminated component are live.
        let descriptor = unsafe {
            libc::openat(
                handle.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error())
                .context("reserve JPEG scratch ownership record");
        }
        // SAFETY: successful openat returns a new owned descriptor.
        let mut record = unsafe { fs::File::from_raw_fd(descriptor) };
        record.write_all(&payload)?;
        record.sync_all()?;
        handle.sync_all()?;
        parent.sync_all()?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = (directory, source_blake3, run_id, invocation_id);
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn candidate_effort(name: &str) -> Option<u8> {
    let value = name.strip_prefix('e')?.strip_suffix(".jxl")?;
    let effort: u8 = value.parse().ok()?;
    ((1..=11).contains(&effort) && value == effort.to_string()).then_some(effort)
}

/// Inspect an exact existing scratch reservation without claiming candidate integrity.
/// # Errors
/// Rejects unavailable/foreign paths, malformed evidence and unexpected entries.
pub fn inspect_jpeg_effort_scratch(directory: &Path) -> Result<JpegScratchObservation> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::ffi::OsStr;
        use std::io::Read;
        use std::os::unix::fs::MetadataExt;

        let (_, handle) = open_scratch(directory)?;
        let before = handle.metadata()?;
        let record =
            crate::io_utils::open_entry_nofollow(&handle, std::ffi::OsStr::new(OWNER_FILE), false)?;
        let record_before = record.metadata()?;
        ensure!(
            record_before.is_file()
                && record_before.nlink() == 1
                && record_before.uid() == before.uid()
                && record_before.mode() & 0o777 == 0o600
                && record_before.len() <= MAX_OWNER_BYTES,
            "Invalid JPEG scratch ownership record"
        );
        let mut record = record.take(MAX_OWNER_BYTES + 1);
        let mut bytes = Vec::new();
        record.read_to_end(&mut bytes)?;
        ensure!(
            u64::try_from(bytes.len())? == record_before.len()
                && same_file_metadata(&record_before, &record.get_ref().metadata()?),
            "JPEG scratch ownership record changed during inspection"
        );
        crate::common_utils::validate_unique_json_members(&bytes)?;
        let owner: Owner = serde_json::from_slice(&bytes)?;
        ensure!(
            owner.schema_version == 1
                && owner.role == "jpeg_full_effort"
                && owner.directory_name == directory_name(directory)?
                && owner.device == before.dev()
                && owner.inode == before.ino()
                && valid_id(&owner.invocation_id)
                && owner.run_id.as_deref().is_none_or(valid_id)
                && owner.source_blake3.len() == 64
                && owner
                    .source_blake3
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "JPEG scratch ownership does not match the observed directory"
        );
        let mut candidate_count = 0;
        let mut candidate_bytes = 0_u64;
        for effort in 1..=11 {
            let name = format!("e{effort}.jxl");
            let candidate =
                match crate::io_utils::open_entry_nofollow(&handle, OsStr::new(&name), false) {
                    Ok(candidate) => candidate,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => {
                        return Err(error).context("JPEG scratch candidate inspection failed");
                    }
                };
            let metadata = candidate.metadata()?;
            ensure!(
                candidate_count < 2
                    && metadata.is_file()
                    && metadata.nlink() == 1
                    && metadata.dev() == before.dev()
                    && metadata.uid() == before.uid(),
                "JPEG scratch candidate has unexpected filesystem identity"
            );
            candidate_count += 1;
            candidate_bytes = candidate_bytes
                .checked_add(metadata.len())
                .context("JPEG scratch byte count overflow")?;
        }
        let mut entries = DirectoryEntries::open(&handle)?;
        let mut listed = 0;
        while let Some(name) = entries.next()? {
            ensure!(
                listed < 3
                    && (name == OWNER_FILE || name.to_str().and_then(candidate_effort).is_some()),
                "JPEG scratch has unexpected entries"
            );
            listed += 1;
        }
        ensure!(
            listed == candidate_count + 1,
            "JPEG scratch changed or has unexpected entries"
        );
        let (_, current) = open_scratch(directory)?;
        ensure!(
            same_file_metadata(&before, &current.metadata()?),
            "JPEG scratch directory changed during inspection"
        );
        let current_record = crate::io_utils::open_entry_nofollow(
            &current,
            std::ffi::OsStr::new(OWNER_FILE),
            false,
        )?;
        ensure!(
            same_file_metadata(&record_before, &current_record.metadata()?),
            "JPEG scratch ownership record was replaced during inspection"
        );
        Ok(JpegScratchObservation {
            schema_version: 1,
            kind: "fast_img_jpeg_scratch",
            owner,
            candidate_count,
            candidate_bytes,
            candidate_integrity: "unverified",
            disposition: "recovery_protected",
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = directory;
        anyhow::bail!("JPEG scratch ownership inspection is unavailable on this platform")
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn same_file_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct DirectoryEntries(*mut libc::DIR);

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl DirectoryEntries {
    fn open(handle: &fs::File) -> Result<Self> {
        use std::os::fd::{AsRawFd, IntoRawFd};

        let listing = handle.try_clone()?;
        // SAFETY: listing supplies a live, owned directory descriptor.
        let stream = unsafe { libc::fdopendir(listing.as_raw_fd()) };
        if stream.is_null() {
            return Err(io::Error::last_os_error()).context("open JPEG scratch directory stream");
        }
        let _ = listing.into_raw_fd();
        Ok(Self(stream))
    }

    fn next(&mut self) -> Result<Option<&std::ffi::OsStr>> {
        use std::ffi::{CStr, OsStr};
        use std::os::unix::ffi::OsStrExt;

        loop {
            #[cfg(target_os = "macos")]
            let errno = unsafe { libc::__error() };
            #[cfg(target_os = "linux")]
            let errno = unsafe { libc::__errno_location() };
            // SAFETY: errno is thread-local and this object owns the live stream.
            let entry = unsafe {
                *errno = 0;
                libc::readdir(self.0)
            };
            if entry.is_null() {
                if unsafe { *errno } != 0 {
                    return Err(io::Error::last_os_error()).context("enumerate JPEG scratch");
                }
                return Ok(None);
            }
            // SAFETY: the name is live and NUL-terminated until the next mutable access.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if !matches!(name, b"." | b"..") {
                return Ok(Some(OsStr::from_bytes(name)));
            }
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for DirectoryEntries {
    fn drop(&mut self) {
        // SAFETY: successful fdopendir transferred sole descriptor ownership to us.
        unsafe { libc::closedir(self.0) };
    }
}

#[derive(Debug, Serialize)]
pub struct JpegScratchInventory {
    schema_version: u8,
    kind: &'static str,
    root_state: &'static str,
    examined_entries: usize,
    recovery_protected: usize,
    unknown_reservations: usize,
    excluded_entries: usize,
    enumeration_limited: bool,
    validation_limited: bool,
    candidate_bytes_observed: u64,
    reservations: Vec<JpegScratchObservation>,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl JpegScratchInventory {
    fn empty() -> Self {
        Self {
            schema_version: 1,
            kind: "fast_img_jpeg_scratch_list",
            root_state: "absent",
            examined_entries: 0,
            recovery_protected: 0,
            unknown_reservations: 0,
            excluded_entries: 0,
            enumeration_limited: false,
            validation_limited: false,
            candidate_bytes_observed: 0,
            reservations: Vec::new(),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn observed_scratch_inventory(
    root: &Path,
    parent: &fs::File,
    max_entries: usize,
    max_directories: usize,
) -> Result<JpegScratchInventory> {
    let mut report = JpegScratchInventory::empty();
    report.root_state = "observed";
    let before = parent.metadata()?;
    let mut matching = Vec::<PathBuf>::new();
    let mut entries = DirectoryEntries::open(parent)?;
    while let Some(name) = entries.next()? {
        if report.examined_entries == max_entries {
            report.enumeration_limited = true;
            break;
        }
        report.examined_entries += 1;
        if name.as_encoded_bytes().starts_with(PREFIX.as_bytes()) {
            if matching.len() == max_directories {
                report.validation_limited = true;
                report.unknown_reservations += 1;
            } else {
                matching.push(root.join(name));
            }
        } else {
            report.excluded_entries += 1;
        }
    }

    matching.sort_unstable();
    for path in matching {
        match inspect_jpeg_effort_scratch(&path) {
            Ok(observation) => {
                report.candidate_bytes_observed = report
                    .candidate_bytes_observed
                    .checked_add(observation.candidate_bytes)
                    .context("JPEG scratch inventory byte count overflow")?;
                report.reservations.push(observation);
                report.recovery_protected += 1;
            }
            Err(_) => report.unknown_reservations += 1,
        }
    }
    let current = open_scratch_parent(root)?;
    ensure!(
        same_file_metadata(&before, &current.metadata()?),
        "JPEG scratch root changed during inspection"
    );
    Ok(report)
}

/// List bounded reservations in the configured formal temporary directory.
/// Only examined entries contribute to counts. Limit flags identify partial observations.
/// Unknown matching names have no recognized ownership and are never included in the bytes.
/// # Errors
/// Rejects unsupported platforms, unsafe roots, and directory changes during inspection.
pub fn list_jpeg_effort_scratch() -> Result<JpegScratchInventory> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let report = JpegScratchInventory::empty();
        let configured = crate::process_lock::inspect_mfb_root_path()?;
        let root = match configured.canonicalize() {
            Ok(root) => root.join("tmp"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(report),
            Err(error) => return Err(error).context("resolve formal JPEG scratch root"),
        };
        let parent = match open_scratch_parent(&root) {
            Ok(parent) => parent,
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
            {
                return Ok(report);
            }
            Err(error) => return Err(error).context("open formal JPEG scratch root"),
        };
        observed_scratch_inventory(&root, &parent, MAX_ROOT_ENTRIES, MAX_INSPECTED_DIRECTORIES)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    anyhow::bail!("JPEG scratch inventory is unavailable on this platform")
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use crate::fast_img_store::JpegExecutionAttempt;

    #[test]
    fn jpeg_scratch_reservation_precedes_encoder_and_rejects_foreign_directory() -> Result<()> {
        if crate::common_utils::isolated_test_process() {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let state = root.join("state");
        let _state = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            state.to_str().context("synthetic state")?,
        );
        let scratch_root = crate::process_lock::get_mfb_tmp_dir()?;
        let source = root.join("private-name.jpg");
        fs::write(&source, b"synthetic")?;
        let output = scratch_root.join("output.jxl");
        let attempt = JpegExecutionAttempt {
            run_id: Some("synthetic-run".into()),
            invocation_id: "synthetic-invocation".into(),
        };
        let mut first = true;
        let mut directory = None;
        super::super::run_jpeg_full_effort_search_for_worker(
            &source,
            &output,
            Some(&attempt),
            |effort, candidate| {
                let path = candidate.parent().context("candidate parent")?;
                if first {
                    first = false;
                    directory = Some(path.to_path_buf());
                    let before = fs::read(path.join(OWNER_FILE))?;
                    let inventory = inspect_jpeg_effort_scratch(path)?;
                    assert_eq!(inventory.owner.run_id.as_deref(), Some("synthetic-run"));
                    assert_eq!(inventory.owner.invocation_id, "synthetic-invocation");
                    assert_eq!(inventory.candidate_count, 0);
                    assert_eq!(inventory.candidate_integrity, "unverified");
                    assert_eq!(inventory.disposition, "recovery_protected");
                    assert!(!String::from_utf8_lossy(&before).contains("private-name"));
                    assert_eq!(fs::read(path.join(OWNER_FILE))?, before);
                }
                fs::write(candidate, vec![effort; usize::from(effort)])?;
                let inventory = inspect_jpeg_effort_scratch(path)?;
                assert!((1..=2).contains(&inventory.candidate_count));
                Ok(())
            },
            |()| true,
            |_| Ok(()),
        )?;
        assert!(!directory.context("missing observed scratch")?.exists());
        assert_eq!(fs::read(&output)?, [1]);

        let owned = tempfile::Builder::new()
            .prefix(PREFIX)
            .tempdir_in(&scratch_root)?;
        let foreign = tempfile::Builder::new()
            .prefix(PREFIX)
            .tempdir_in(&scratch_root)?;
        for path in [owned.path(), foreign.path()] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        reserve(owned.path(), &"a".repeat(64), None, "synthetic-invocation")?;
        fs::copy(
            owned.path().join(OWNER_FILE),
            foreign.path().join(OWNER_FILE),
        )?;
        assert!(inspect_jpeg_effort_scratch(foreign.path()).is_err());
        fs::write(owned.path().join("unowned"), b"leave untouched")?;
        assert!(inspect_jpeg_effort_scratch(owned.path()).is_err());
        assert_eq!(fs::read(owned.path().join("unowned"))?, b"leave untouched");
        fs::remove_file(owned.path().join("unowned"))?;
        std::os::unix::fs::symlink(&source, owned.path().join("e1.jxl"))?;
        assert!(inspect_jpeg_effort_scratch(owned.path()).is_err());
        assert_eq!(fs::read(&source)?, b"synthetic");
        Ok(())
    }

    #[test]
    fn jpeg_scratch_inventory_is_bounded_read_only_and_rejects_unowned_entries() -> Result<()> {
        if crate::common_utils::isolated_test_process() {
            return Ok(());
        }
        use std::os::unix::fs::{PermissionsExt, symlink};
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let state = root.join("state");
        let _state = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            state.to_str().context("synthetic state")?,
        );
        assert_eq!(list_jpeg_effort_scratch()?.root_state, "absent");
        assert!(!state.exists());
        let scratch = crate::process_lock::get_mfb_tmp_dir()?;
        let owned = scratch.join(format!("{PREFIX}z-good"));
        let other_owned = scratch.join(format!("{PREFIX}a-good"));
        let foreign = scratch.join(format!("{PREFIX}copied"));
        for dir in [&owned, &other_owned, &foreign] {
            fs::create_dir(dir)?;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        reserve(
            &owned,
            &"a".repeat(64),
            Some("synthetic-run"),
            "synthetic-invocation",
        )?;
        reserve(&other_owned, &"b".repeat(64), None, "synthetic-other")?;
        fs::copy(owned.join(OWNER_FILE), foreign.join(OWNER_FILE))?;
        fs::write(owned.join("e9.jxl"), b"synthetic")?;
        fs::write(scratch.join("unrelated.txt"), b"untouched")?;
        symlink(&owned, scratch.join(format!("{PREFIX}link")))?;
        let before = fs::metadata(&scratch)?;
        let owner_before = fs::read(owned.join(OWNER_FILE))?;

        let report = list_jpeg_effort_scratch()?;
        assert_eq!(report.root_state, "observed");
        assert_eq!(
            (
                report.examined_entries,
                report.recovery_protected,
                report.unknown_reservations,
                report.excluded_entries,
                report.candidate_bytes_observed
            ),
            (5, 2, 2, 1, 9)
        );
        assert_eq!(
            report
                .reservations
                .iter()
                .map(|item| item.owner.directory_name.as_str())
                .collect::<Vec<_>>(),
            [
                other_owned.file_name().unwrap().to_str().unwrap(),
                owned.file_name().unwrap().to_str().unwrap()
            ]
        );
        assert!(
            report
                .reservations
                .iter()
                .all(|item| item.candidate_integrity == "unverified"
                    && item.disposition == "recovery_protected")
        );
        assert!(!report.enumeration_limited && !report.validation_limited);

        let parent = open_scratch_parent(&scratch)?;
        let limited = observed_scratch_inventory(&scratch, &parent, 4, 0)?;
        assert!(limited.enumeration_limited && limited.validation_limited);
        assert_eq!(limited.examined_entries, 4);
        assert_eq!(limited.unknown_reservations + limited.excluded_entries, 4);
        assert!(limited.reservations.is_empty());
        assert!(same_file_metadata(&before, &fs::metadata(&scratch)?));
        assert_eq!(fs::read(owned.join(OWNER_FILE))?, owner_before);
        assert_eq!(fs::read(scratch.join("unrelated.txt"))?, b"untouched");

        let moved = state.join("held-tmp");
        fs::rename(&scratch, &moved)?;
        symlink(&moved, &scratch)?;
        assert!(list_jpeg_effort_scratch().is_err());
        Ok(())
    }
}

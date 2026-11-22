//! macOS native metadata preservation
//!
//! Uses `unsafe` only for FFI to Darwin filesystem APIs.
//! Invariants: `CStrings` and pointers are valid for the duration of each call;
//! paths come from Rust `Path`.

use super::quarantine::{
    open_regular_quarantine_file, preserving_quarantine, unverified_copy,
    verify_quarantine_file_identity,
};
use crate::builder_base::ToolBuilder;
use std::ffi::{CString, c_void};
use std::fs::{File, Metadata};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub(super) fn copy_native_acl_and_stat(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    let source = open_regular_quarantine_file(src)?;
    let destination = open_regular_quarantine_file(dst).map_err(unverified_copy)?;
    let metadata = destination.metadata().map_err(unverified_copy)?;
    if metadata.nlink() != 1 {
        return Err(unverified_copy(io::Error::new(
            io::ErrorKind::InvalidInput,
            "metadata destination must be a standalone file",
        )));
    }
    verify_quarantine_file_identity(dst, &destination, &metadata).map_err(unverified_copy)?;
    let result = preserving_quarantine(&destination, || {
        let source_metadata = source.metadata()?;
        copy_acl(&source, &destination)?;
        copy_stat(&source, &source_metadata, &destination, &metadata)
    });
    verify_quarantine_file_identity(dst, &destination, &metadata).map_err(unverified_copy)?;
    result
}

pub(super) fn copy_directory_acl(src: &Path, dst: &Path) -> io::Result<()> {
    let source = open_directory_for_acl(src)?;
    let destination = open_directory_for_acl(dst)?;
    let before = destination.metadata()?;
    let result = preserving_quarantine(&destination, || copy_acl(&source, &destination));
    crate::io_utils::check_open_file_identity(&before, &open_directory_for_acl(dst)?.metadata()?)?;
    result
}

fn open_directory_for_acl(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW_ANY | libc::O_NONBLOCK)
        .open(crate::io_utils::darwin_nofollow_path(path)?)
}

const ACL_TYPE_EXTENDED: u32 = 0x100;

unsafe extern "C" {
    fn acl_get_fd_np(fd: libc::c_int, kind: u32) -> *mut c_void;
    fn acl_set_fd_np(fd: libc::c_int, acl: *mut c_void, kind: u32) -> libc::c_int;
    fn acl_init(count: libc::c_int) -> *mut c_void;
    fn acl_free(acl: *mut c_void) -> libc::c_int;
    fn acl_get_entry(acl: *mut c_void, index: libc::c_int, entry: *mut *mut c_void) -> libc::c_int;
    fn acl_get_flagset_np(entry: *mut c_void, flags: *mut *mut c_void) -> libc::c_int;
    fn acl_get_flag_np(flags: *mut c_void, flag: u32) -> libc::c_int;
    fn acl_create_entry(acl: *mut *mut c_void, entry: *mut *mut c_void) -> libc::c_int;
    fn acl_copy_entry(destination: *mut c_void, source: *mut c_void) -> libc::c_int;
}

struct NativeAcl(*mut c_void);

impl Drop for NativeAcl {
    fn drop(&mut self) {
        // SAFETY: This is the only owner of the allocated Darwin ACL.
        if unsafe { acl_free(self.0) } != 0 {
            tracing::debug!(error = %io::Error::last_os_error(), "could not free native ACL");
        }
    }
}

fn read_acl(file: &File) -> io::Result<Option<NativeAcl>> {
    // SAFETY: The descriptor is open through the call.
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    Ok(Some(NativeAcl(acl)))
}

fn append_acl_entries(
    destination: &mut NativeAcl,
    source: Option<&NativeAcl>,
    inherited: bool,
) -> io::Result<()> {
    const ACL_FIRST_ENTRY: libc::c_int = 0;
    const ACL_NEXT_ENTRY: libc::c_int = -1;
    const ACL_ENTRY_INHERITED: u32 = 1 << 4;

    let Some(source) = source else {
        return Ok(());
    };
    let mut index = ACL_FIRST_ENTRY;
    loop {
        let mut from = std::ptr::null_mut();
        // SAFETY: The source ACL stays live and separate from the one
        // being grown. The entry is borrowed for only this iteration.
        if unsafe { acl_get_entry(source.0, index, &raw mut from) } != 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::EINVAL) {
                Ok(())
            } else {
                Err(error)
            };
        }
        index = ACL_NEXT_ENTRY;
        let mut flags = std::ptr::null_mut();
        // SAFETY: The entry belongs to the retained source ACL.
        if unsafe { acl_get_flagset_np(from, &raw mut flags) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: The flags are borrowed from the live source ACL entry.
        let actual = unsafe { acl_get_flag_np(flags, ACL_ENTRY_INHERITED) };
        if actual < 0 {
            return Err(io::Error::last_os_error());
        }
        if (actual != 0) != inherited {
            continue;
        }
        let mut to = std::ptr::null_mut();
        // SAFETY: acl_create_entry may update the owning pointer, and both
        // entries are valid for the copy after creation succeeds.
        if unsafe { acl_create_entry(&raw mut destination.0, &raw mut to) } != 0
            || unsafe { acl_copy_entry(to, from) } != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
}

fn copy_acl(source: &File, destination: &File) -> io::Result<()> {
    let source_acl = read_acl(source)?;
    let destination_acl = read_acl(destination)?;
    if source_acl.is_none() && destination_acl.is_none() {
        return Ok(());
    }
    // SAFETY: Zero entries creates a valid empty extended ACL.
    let empty = unsafe { acl_init(0) };
    if empty.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut merged = NativeAcl(empty);
    // Keep the source's explicit rules and the destination's inherited rules.
    append_acl_entries(&mut merged, source_acl.as_ref(), false)?;
    append_acl_entries(&mut merged, destination_acl.as_ref(), true)?;
    // SAFETY: Both the destination descriptor and the ACL remain live
    // through the kernel call. Setting an ACL cannot copy quarantine.
    if unsafe { acl_set_fd_np(destination.as_raw_fd(), merged.0, ACL_TYPE_EXTENDED) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn copy_stat(
    source: &File,
    input: &Metadata,
    destination: &File,
    output: &Metadata,
) -> io::Result<()> {
    use std::os::macos::fs::MetadataExt;
    // Darwin sys/stat.h flags for destination-specific kernel state.
    const UF_DATAVAULT: u32 = 0x0000_0080;
    const SF_RESTRICTED: u32 = 0x0008_0000;
    const SF_NOUNLINK: u32 = 0x0010_0000;
    const PRESERVE_FLAGS: u32 = UF_DATAVAULT | SF_RESTRICTED | SF_NOUNLINK | libc::UF_COMPRESSED;
    const OMIT_FLAGS: u32 = PRESERVE_FLAGS | libc::UF_TRACKED;

    let fd = destination.as_raw_fd();
    let times = [
        libc::timespec {
            tv_sec: input.st_atime(),
            tv_nsec: input.st_atime_nsec(),
        },
        libc::timespec {
            tv_sec: input.st_mtime(),
            tv_nsec: input.st_mtime_nsec(),
        },
    ];
    let mut mode =
        libc::mode_t::try_from(input.st_mode()).map_err(io::Error::other)? & !libc::S_IFMT;
    if volume_disallows_set_id(source) || volume_disallows_set_id(destination) {
        mode &= !(libc::S_ISUID | libc::S_ISGID);
    }
    // SAFETY: The descriptor is owned through all native operations. The
    // timestamps array has the two times expected by futimens.
    unsafe {
        if libc::futimens(fd, times.as_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        // Like Darwin's copyfile, ownership and filesystem-specific flags
        // remain best effort when the current user or volume cannot set them.
        if (input.st_uid() != output.st_uid() || input.st_gid() != output.st_gid())
            && libc::fchown(fd, input.st_uid(), input.st_gid()) != 0
        {
            tracing::debug!(error = %io::Error::last_os_error(), "could not copy native ownership");
        }
        if libc::fchmod(fd, mode) != 0 {
            return Err(io::Error::last_os_error());
        }
        let flags = (input.st_flags() & !OMIT_FLAGS) | (output.st_flags() & PRESERVE_FLAGS);
        if flags != output.st_flags() && libc::fchflags(fd, flags) != 0 {
            tracing::debug!(error = %io::Error::last_os_error(), "could not copy native file flags");
        }
    }
    Ok(())
}

fn volume_disallows_set_id(file: &File) -> bool {
    let mut volume = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: A successful fstatfs initializes the buffer.
    unsafe {
        libc::fstatfs(file.as_raw_fd(), volume.as_mut_ptr()) == 0
            && volume.assume_init().f_flags & libc::MNT_NOSUID.cast_unsigned() != 0
    }
}

pub(super) fn verify_directory_acl(src: &Path, dst: &Path) -> io::Result<()> {
    if directory_acl(src)? != directory_acl(dst)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directory ACL differs after native copy",
        ));
    }
    Ok(())
}

fn directory_acl(path: &Path) -> io::Result<Option<Vec<u8>>> {
    acl_bytes(&open_directory_for_acl(path)?)
}

fn acl_bytes(handle: &File) -> io::Result<Option<Vec<u8>>> {
    unsafe extern "C" {
        fn filesec_init() -> *mut std::ffi::c_void;
        fn filesec_free(security: *mut std::ffi::c_void);
        fn filesec_query_property(
            security: *mut std::ffi::c_void,
            property: i32,
            present: *mut i32,
        ) -> i32;
        fn filesec_get_property(
            security: *mut std::ffi::c_void,
            property: i32,
            value: *mut std::ffi::c_void,
        ) -> i32;
        #[cfg_attr(target_arch = "x86_64", link_name = "fstatx_np$INODE64")]
        fn fstatx_np(fd: i32, metadata: *mut libc::stat, security: *mut std::ffi::c_void) -> i32;
        fn acl_size(acl: *mut std::ffi::c_void) -> isize;
        fn acl_copy_ext(
            buffer: *mut std::ffi::c_void,
            acl: *mut std::ffi::c_void,
            size: isize,
        ) -> isize;
        fn acl_free(object: *mut std::ffi::c_void) -> i32;
    }
    const FILESEC_ACL: i32 = 5;
    // SAFETY: The open descriptor binds the native observation; every allocation is released below.
    unsafe {
        let security = filesec_init();
        if security.is_null() {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            if fstatx_np(handle.as_raw_fd(), metadata.as_mut_ptr(), security) != 0 {
                return Err(io::Error::last_os_error());
            }
            let mut present = 0;
            if filesec_query_property(security, FILESEC_ACL, std::ptr::from_mut(&mut present)) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if present == 0 {
                return Ok(None);
            }
            let mut acl: *mut std::ffi::c_void = std::ptr::null_mut();
            if filesec_get_property(security, FILESEC_ACL, std::ptr::from_mut(&mut acl).cast()) != 0
            {
                return Err(io::Error::last_os_error());
            }
            if acl.is_null() {
                return Err(io::Error::other("native ACL property has no value"));
            }
            let exported = (|| {
                let size = acl_size(acl);
                if size < 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut bytes = vec![0_u8; usize::try_from(size).map_err(io::Error::other)?];
                let written = acl_copy_ext(bytes.as_mut_ptr().cast(), acl, size);
                if written < 0 {
                    return Err(io::Error::last_os_error());
                }
                if usize::try_from(written).map_err(io::Error::other)? != bytes.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "native ACL export length differs",
                    ));
                }
                Ok(Some(bytes))
            })();
            if acl_free(acl) != 0 {
                return Err(io::Error::last_os_error());
            }
            exported
        })();
        filesec_free(security);
        result
    }
}

#[repr(C)]
struct Attrlist {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

const ATTR_CMN_CRTIME: u32 = 0x0000_0200;
const ATTR_CMN_ADDEDTIME: u32 = 0x1000_0000;
const ATTR_BIT_MAP_COUNT: u16 = 5;

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

const GETATTRLIST_LENGTH_BYTES: usize = std::mem::size_of::<u32>();
const TIMESPEC_BYTES: usize = std::mem::size_of::<i64>() * 2;
const GETATTRLIST_TIME_BUFFER_BYTES: usize = GETATTRLIST_LENGTH_BYTES + TIMESPEC_BYTES;
const NANOS_PER_SECOND: i64 = 1_000_000_000;
const MACOS_UNSET_TIME_SEC: i64 = i64::MAX / NANOS_PER_SECOND;
const MACOS_UNSET_TIME_NSEC: i64 = i64::MAX % NANOS_PER_SECOND;
const XATTR_CONTENT_CREATION_DATE: &str = "com.apple.metadata:kMDItemContentCreationDate";

pub(super) fn set_creation_time(path: &Path, time: std::time::SystemTime) -> io::Result<()> {
    set_time_attr(path, time, ATTR_CMN_CRTIME)
}

pub(super) fn set_added_time(path: &Path, time: std::time::SystemTime) -> io::Result<()> {
    set_time_attr(path, time, ATTR_CMN_ADDEDTIME)
}

pub(super) fn get_added_time(path: &Path) -> io::Result<std::time::SystemTime> {
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    read_added_time(|attributes, buffer| {
        // SAFETY: the path and buffers stay valid for this synchronous call.
        unsafe {
            libc::getattrlist(
                c_path.as_ptr(),
                attributes,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        }
    })
}

pub(super) fn get_added_time_on_file(file: &std::fs::File) -> io::Result<std::time::SystemTime> {
    read_added_time(|attributes, buffer| {
        // SAFETY: file owns the descriptor and the buffers stay valid for the call.
        unsafe {
            libc::fgetattrlist(
                file.as_raw_fd(),
                attributes,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        }
    })
}

fn read_added_time(
    get_attributes: impl FnOnce(*mut std::ffi::c_void, &mut [u8]) -> i32,
) -> io::Result<std::time::SystemTime> {
    let mut attr_list = Attrlist {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_ADDEDTIME,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let mut buf = [0_u8; GETATTRLIST_TIME_BUFFER_BYTES];
    let ret = get_attributes((&raw mut attr_list).cast(), &mut buf);
    if ret != 0_i32 {
        return Err(io::Error::last_os_error());
    }
    parse_getattrlist_time_buffer(&buf, "added_time")
}

pub(super) fn apply_spotlight_content_creation_date(src: &Path, dst: &Path) -> io::Result<()> {
    let (content_date, date_source) = source_content_creation_date(src)?;
    let payload = binary_plist_date(&content_date)?;
    xattr::set(dst, XATTR_CONTENT_CREATION_DATE, &payload).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "failed to set Spotlight content creation date xattr on {} from {}: {e}",
                dst.display(),
                src.display()
            ),
        )
    })?;
    match xattr::get(dst, XATTR_CONTENT_CREATION_DATE) {
        Ok(Some(actual)) if actual == payload => {
            tracing::debug!(
                target: "metadata",
                date_source,
                "Spotlight content date copied or derived from the recorded source field"
            );
            Ok(())
        }
        Ok(Some(_)) => Err(io::Error::other(format!(
            "Spotlight content creation date xattr verification mismatch on {}",
            dst.display()
        ))),
        Ok(None) => Err(io::Error::other(format!(
            "Spotlight content creation date xattr missing after set on {}",
            dst.display()
        ))),
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!(
                "failed to verify Spotlight content creation date xattr on {}: {e}",
                dst.display()
            ),
        )),
    }
}

fn source_content_creation_date(
    src: &Path,
) -> io::Result<(chrono::DateTime<chrono::Utc>, &'static str)> {
    if let Some(date) = read_mdls_content_creation_date(src)? {
        return Ok((date, "spotlight_content_creation"));
    }
    if let Some(date) = read_exif_datetime_original(src)? {
        return Ok((date, "exif_original_with_offset"));
    }
    let created = std::fs::metadata(src)
        .and_then(|metadata| metadata.created())
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "failed to read source file creation time fallback for {}: {e}",
                    src.display()
                ),
            )
        })?;
    Ok((created.into(), "filesystem_creation_fallback"))
}

fn read_mdls_content_creation_date(
    src: &Path,
) -> io::Result<Option<chrono::DateTime<chrono::Utc>>> {
    let output = match std::process::Command::new("mdls")
        .arg("-raw")
        .arg("-name")
        .arg("kMDItemContentCreationDate")
        .arg(src)
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(io::Error::new(
                e.kind(),
                format!("failed to run mdls for {}: {e}", src.display()),
            ));
        }
    };
    if !output.status.success() {
        return Ok(None);
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "(null)" {
        return Ok(None);
    }
    parse_spotlight_utc_date(trimmed).map(Some)
}

fn read_exif_datetime_original(src: &Path) -> io::Result<Option<chrono::DateTime<chrono::Utc>>> {
    if !crate::ExiftoolBuilder::check_available() {
        return Ok(None);
    }
    let output = exif_original_date_command(src).output().map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "failed to run exiftool DateTimeOriginal for {}: {e}",
                src.display()
            ),
        )
    })?;
    if !output.status.success() {
        return Ok(None);
    }
    parse_exif_original_date(&output.stdout)
}

fn exif_original_date_command(src: &Path) -> std::process::Command {
    // ExifTool's date formatter would supply the process timezone for a naive
    // timestamp. Read the raw EXIF date and its recorded offset instead.
    crate::ExiftoolBuilder::new()
        .arg("-j")
        .arg("-EXIF:DateTimeOriginal")
        .arg("-EXIF:OffsetTimeOriginal")
        .arg(crate::path_safety::exiftool_path_arg(src).as_ref())
        .build()
}

fn parse_spotlight_utc_date(raw: &str) -> io::Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S %z")
        .map(|date| date.with_timezone(&chrono::Utc))
        .map_err(|e| io::Error::other(format!("invalid Spotlight UTC date '{raw}': {e}")))
}

fn parse_exif_original_date(json: &[u8]) -> io::Result<Option<chrono::DateTime<chrono::Utc>>> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "PascalCase")]
    struct OriginalDate {
        date_time_original: Option<String>,
        offset_time_original: Option<String>,
    }

    let records: Vec<OriginalDate> = serde_json::from_slice(json).map_err(io::Error::other)?;
    let [record] = records.as_slice() else {
        return Err(io::Error::other("expected one EXIF original date record"));
    };
    let (Some(date), Some(offset)) = (
        record
            .date_time_original
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        record
            .offset_time_original
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) else {
        return Ok(None);
    };
    if !matches!(
        offset.as_bytes(),
        [
            b'+' | b'-',
            b'0'..=b'9',
            b'0'..=b'9',
            b':',
            b'0'..=b'9',
            b'0'..=b'9'
        ]
    ) {
        return Err(io::Error::other("invalid EXIF OffsetTimeOriginal"));
    }
    chrono::DateTime::parse_from_str(&format!("{date}{offset}"), "%Y:%m:%d %H:%M:%S%:z")
        .map(|date| date.with_timezone(&chrono::Utc))
        .map(Some)
        .map_err(|e| io::Error::other(format!("invalid EXIF original date or offset: {e}")))
}

fn binary_plist_date(date: &chrono::DateTime<chrono::Utc>) -> io::Result<Vec<u8>> {
    use chrono::TimeZone;

    let apple_epoch = chrono::Utc
        .with_ymd_and_hms(2001, 1, 1, 0, 0, 0)
        .single()
        .ok_or_else(|| io::Error::other("failed to construct Apple plist epoch"))?;
    let nanos = date
        .signed_duration_since(apple_epoch)
        .num_nanoseconds()
        .ok_or_else(|| io::Error::other("content creation date overflows plist duration"))?;
    let seconds = crate::numeric_cast::i64_to_f64(nanos) / 1_000_000_000.0;

    let mut plist_bytes = Vec::with_capacity(49);
    plist_bytes.extend_from_slice(b"bplist00");
    plist_bytes.push(0x33);
    plist_bytes.extend_from_slice(&seconds.to_be_bytes());
    plist_bytes.push(0x08);
    plist_bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    plist_bytes.push(0x01);
    plist_bytes.push(0x01);
    plist_bytes.extend_from_slice(&1_u64.to_be_bytes());
    plist_bytes.extend_from_slice(&0_u64.to_be_bytes());
    plist_bytes.extend_from_slice(&17_u64.to_be_bytes());
    Ok(plist_bytes)
}

fn parse_getattrlist_time_buffer(buf: &[u8], label: &str) -> io::Result<std::time::SystemTime> {
    if buf.len() < GETATTRLIST_TIME_BUFFER_BYTES {
        return Err(io::Error::other(format!(
            "Invalid {label}: getattrlist buffer too short"
        )));
    }
    let reported_len = u32::from_ne_bytes(
        buf[0..GETATTRLIST_LENGTH_BYTES]
            .try_into()
            .map_err(|e| io::Error::other(format!("Invalid {label} length bytes: {e}")))?,
    );
    let min_len = u32::try_from(GETATTRLIST_TIME_BUFFER_BYTES)
        .map_err(|e| io::Error::other(format!("Invalid {label} buffer size: {e}")))?;
    if reported_len < min_len {
        return Err(io::Error::other(format!(
            "Invalid {label}: getattrlist reported {reported_len} bytes, expected at least \
             {min_len}"
        )));
    }
    let sec_start = GETATTRLIST_LENGTH_BYTES;
    let nsec_start = sec_start + std::mem::size_of::<i64>();
    let sec = i64::from_ne_bytes(
        buf[sec_start..nsec_start]
            .try_into()
            .map_err(|e| io::Error::other(format!("Invalid {label} seconds bytes: {e}")))?,
    );
    let nsec = i64::from_ne_bytes(
        buf[nsec_start..GETATTRLIST_TIME_BUFFER_BYTES]
            .try_into()
            .map_err(|e| io::Error::other(format!("Invalid {label} nanoseconds bytes: {e}")))?,
    );
    system_time_from_macos_timespec(sec, nsec, label)
}

fn system_time_from_macos_timespec(
    sec: i64,
    nsec: i64,
    label: &str,
) -> io::Result<std::time::SystemTime> {
    if sec == MACOS_UNSET_TIME_SEC && nsec == MACOS_UNSET_TIME_NSEC {
        return Err(io::Error::other(format!(
            "Invalid {label}: macOS unset sentinel"
        )));
    }
    if !(0..NANOS_PER_SECOND).contains(&nsec) {
        return Err(io::Error::other(format!(
            "Invalid {label}: nanoseconds out of range ({nsec})"
        )));
    }
    let duration = std::time::Duration::new(
        crate::numeric_cast::i64_to_u64_strict(sec, "added_time_sec")
            .ok_or_else(|| io::Error::other(format!("Invalid {label}: negative seconds")))?,
        u32::try_from(nsec).map_err(|e| io::Error::other(format!("Invalid {label} nsec: {e}")))?,
    );
    Ok(std::time::SystemTime::UNIX_EPOCH + duration)
}

fn set_time_attr(path: &Path, time: std::time::SystemTime, attr: u32) -> io::Result<()> {
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    write_time_attr(time, attr, |attributes, buffer, size| {
        // SAFETY: the path and buffers stay valid for this synchronous call.
        unsafe { libc::setattrlist(c_path.as_ptr(), attributes, buffer, size, 0) }
    })
}

pub(super) fn set_creation_time_on_file(
    file: &std::fs::File,
    time: std::time::SystemTime,
) -> io::Result<()> {
    set_time_attr_on_file(file, time, ATTR_CMN_CRTIME)
}

pub(super) fn set_added_time_on_file(
    file: &std::fs::File,
    time: std::time::SystemTime,
) -> io::Result<()> {
    set_time_attr_on_file(file, time, ATTR_CMN_ADDEDTIME)
}

fn set_time_attr_on_file(
    file: &std::fs::File,
    time: std::time::SystemTime,
    attr: u32,
) -> io::Result<()> {
    write_time_attr(time, attr, |attributes, buffer, size| {
        // SAFETY: file owns the descriptor and the buffers stay valid for the call.
        unsafe { libc::fsetattrlist(file.as_raw_fd(), attributes, buffer, size, 0) }
    })
}

fn write_time_attr(
    time: std::time::SystemTime,
    attr: u32,
    set_attributes: impl FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void, usize) -> i32,
) -> io::Result<()> {
    let mut attr_list = Attrlist {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: attr,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    let duration = time
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map_err(io::Error::other)?;
    let mut buf = Timespec {
        tv_sec: crate::numeric_cast::u64_to_i64_strict(duration.as_secs(), "timestamp_sec")
            .ok_or_else(|| io::Error::other("Timestamp overflows i64"))?,
        tv_nsec: i64::from(duration.subsec_nanos()),
    };
    let ret = set_attributes(
        (&raw mut attr_list).cast(),
        (&raw mut buf).cast(),
        std::mem::size_of::<Timespec>(),
    );
    if ret != 0_i32 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Appends MFB branding to the macOS Finder comment (kMDItemFinderComment).
///
/// This uses `AppleScript` to ensure we interact properly with the Finder's
/// database, as raw xattr writes for 'com.apple.metadata:kMDItemFinderComment'
/// require complex binary plist encoding and may not trigger Spotlight index
/// updates correctly.
///
/// # Errors
/// Returns an `io::Result` if `AppleScript` execution fails.
pub fn append_mfb_branding(path: &Path) -> io::Result<()> {
    // Branding is disabled by default to minimize metadata pollution.
    // To enable, set the environment variable:
    // MODERN_FORMAT_BOOST_ENABLE_BRANDING=1
    if std::env::var(crate::constants::ENV_ENABLE_BRANDING).as_deref() != Ok("1") {
        return Ok(());
    }

    let path_str = path.to_string_lossy();
    let branding = crate::infra::static_logs::messages::MSG_BRANDING_DESCRIPTION;

    // AppleScript logic:
    // 1. Get existing comment.
    // 2. If it contains the branding, skip.
    // 3. Otherwise, prepend branding followed by a newline (if original comment
    //    existed).
    let script = format!(
        "tell application \"Finder\"
            set theFile to (POSIX file \"{path}\" as alias)
            set oldComment to (comment of theFile)
            if oldComment does not contain \"{branding}\" then
                if oldComment is \"\" then
                    set newComment to \"{branding}\"
                else
                    set newComment to \"{branding}\" & return & oldComment
                end if
                set comment of theFile to newComment
            end if
        end tell",
        path = path_str.replace('"', "\\\""),
        branding = branding
    );

    let output = crate::tool_builders::OsascriptBuilder::new()
        .script(&script)
        .build()
        .output()?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!("AppleScript failed: {err}")));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        XATTR_CONTENT_CREATION_DATE, apply_spotlight_content_creation_date, binary_plist_date,
        exif_original_date_command, get_added_time, parse_exif_original_date,
        parse_spotlight_utc_date, set_added_time, source_content_creation_date,
    };
    use crate::builder_base::ToolBuilder;

    #[test]
    fn native_metadata_acl_matches_darwin_explicit_and_inherited_rules() -> std::io::Result<()> {
        use std::fs;
        use std::os::fd::AsRawFd as _;
        use std::path::Path;

        fn add_rule(path: &Path, rule: &str) -> std::io::Result<()> {
            let status = std::process::Command::new("/bin/chmod")
                .args(["+a", rule])
                .arg(path)
                .status()?;
            assert!(status.success());
            Ok(())
        }

        let root = tempfile::tempdir()?;
        let from = root.path().join("from");
        let to = root.path().join("to");
        fs::create_dir(&from)?;
        fs::create_dir(&to)?;
        add_rule(
            &from,
            "everyone allow readattr,file_inherit,directory_inherit",
        )?;
        add_rule(
            &to,
            "everyone allow readextattr,file_inherit,directory_inherit",
        )?;
        for directory in [true, false] {
            let kind = if directory { "directory" } else { "file" };
            let source = from.join(kind);
            let destination = to.join(kind);
            let expected = to.join(format!("{kind}-darwin-copyfile"));
            for path in [&source, &destination, &expected] {
                if directory {
                    fs::create_dir(path)?;
                } else {
                    fs::write(path, b"synthetic fixture")?;
                }
            }
            add_rule(&source, "everyone allow readsecurity")?;
            let input = fs::File::open(&source)?;
            let actual = fs::File::open(&destination)?;
            let reference = fs::File::open(&expected)?;
            let inherited_before = super::acl_bytes(&actual)?;
            let source_before = super::acl_bytes(&input)?;
            assert!(inherited_before.is_some() && source_before.is_some());
            // SAFETY: All descriptors are owned synthetic fixtures and remain
            // open. This legacy call serves only as a behavioral reference.
            let status = unsafe {
                libc::fcopyfile(
                    input.as_raw_fd(),
                    reference.as_raw_fd(),
                    std::ptr::null_mut(),
                    if directory { 1 } else { 3 },
                )
            };
            assert_eq!(status, 0);
            if directory {
                super::copy_directory_acl(&source, &destination)?;
            } else {
                super::copy_native_acl_and_stat(&source, &destination)?;
            }
            let copied = super::acl_bytes(&actual)?;
            assert_eq!(copied, super::acl_bytes(&reference)?);
            assert_ne!(copied, inherited_before);
            assert_ne!(copied, source_before);
            assert_eq!(super::acl_bytes(&input)?, source_before);
        }
        Ok(())
    }

    #[test]
    fn native_metadata_copy_preserves_quarantine_on_files_and_directories() -> std::io::Result<()> {
        use super::super::quarantine::ATTRIBUTE;
        use std::fs;
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let destination = root.path().join("destination");
        let source_mark = b"0081;66bb3311;SyntheticSource;";
        let destination_mark = b"0081;66bb3301;SyntheticDestination;";
        fs::write(&source, b"source fixture")?;
        fs::write(&destination, b"destination fixture")?;
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640))?;
        xattr::set(&source, ATTRIBUTE, source_mark)?;
        xattr::set(&destination, "org.mfb.synthetic-fixture", b"keep")?;
        let modified = std::time::UNIX_EPOCH + std::time::Duration::new(1_777_000_000, 123_000_000);
        fs::File::open(&source)?.set_times(fs::FileTimes::new().set_modified(modified))?;
        super::copy_native_acl_and_stat(&source, &destination)?;
        assert_eq!(
            xattr::get(&source, ATTRIBUTE)?.as_deref(),
            Some(source_mark.as_slice())
        );
        assert_eq!(xattr::get(&destination, ATTRIBUTE)?, None);
        assert_eq!(
            xattr::get(&destination, "org.mfb.synthetic-fixture")?.as_deref(),
            Some(b"keep".as_slice())
        );
        assert_eq!(
            fs::metadata(&destination)?.permissions().mode() & 0o777,
            0o640
        );
        assert_eq!(fs::metadata(&destination)?.modified()?, modified);

        xattr::set(&destination, ATTRIBUTE, destination_mark)?;
        super::copy_native_acl_and_stat(&source, &destination)?;
        assert_eq!(
            xattr::get(&destination, ATTRIBUTE)?.as_deref(),
            Some(destination_mark.as_slice())
        );

        let source_dir = root.path().join("source-directory");
        let destination_dir = root.path().join("destination-directory");
        fs::create_dir(&source_dir)?;
        fs::create_dir(&destination_dir)?;
        let status = std::process::Command::new("/bin/chmod")
            .args(["+a", "everyone allow readattr"])
            .arg(&source_dir)
            .status()?;
        assert!(status.success());
        xattr::set(&source_dir, ATTRIBUTE, source_mark)?;
        xattr::set(&destination_dir, ATTRIBUTE, destination_mark)?;
        super::copy_directory_acl(&source_dir, &destination_dir)?;
        super::verify_directory_acl(&source_dir, &destination_dir)?;
        assert_eq!(
            xattr::get(&source_dir, ATTRIBUTE)?.as_deref(),
            Some(source_mark.as_slice())
        );
        assert_eq!(
            xattr::get(&destination_dir, ATTRIBUTE)?.as_deref(),
            Some(destination_mark.as_slice())
        );
        Ok(())
    }

    #[test]
    fn get_added_time_rejects_macos_unset_sentinel() {
        let tmp = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("tempfile: {e}"));
        let sentinel = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::new(9_223_372_036, 854_775_807);

        set_added_time(tmp.path(), sentinel)
            .unwrap_or_else(|e| panic!("set sentinel added time: {e}"));

        let err = get_added_time(tmp.path())
            .expect_err("macOS Date Added sentinel must not be accepted as a real timestamp");
        assert!(
            err.to_string().contains("unset sentinel"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn get_added_time_roundtrips_valid_macos_added_time() {
        let tmp = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("tempfile: {e}"));
        let expected = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::new(1_753_112_055, 343_210_578);

        set_added_time(tmp.path(), expected)
            .unwrap_or_else(|e| panic!("set valid added time: {e}"));

        let actual = get_added_time(tmp.path()).unwrap_or_else(|e| panic!("get added time: {e}"));
        assert_eq!(actual, expected);
    }

    #[test]
    fn apply_file_timestamps_repairs_invalid_destination_added_time() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let src = dir.path().join("source.jpg");
        let dst = dir.path().join("output.jxl");
        std::fs::write(&src, b"source").unwrap_or_else(|e| panic!("write source: {e}"));
        std::fs::write(&dst, b"output").unwrap_or_else(|e| panic!("write output: {e}"));
        let expected = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::new(1_753_112_055, 343_210_578);
        let sentinel = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::new(9_223_372_036, 854_775_807);

        set_added_time(&src, expected).unwrap_or_else(|e| panic!("set source added time: {e}"));
        set_added_time(&dst, sentinel).unwrap_or_else(|e| panic!("set destination sentinel: {e}"));

        super::super::apply_file_timestamps(&src, &dst)
            .unwrap_or_else(|e| panic!("apply timestamps: {e}"));

        let actual =
            get_added_time(&dst).unwrap_or_else(|e| panic!("get repaired added time: {e}"));
        assert_eq!(actual, expected);
    }

    #[test]
    fn spotlight_content_creation_date_plist_matches_macos_binary_date() {
        let date = parse_spotlight_utc_date("2024-04-06 07:53:35 +0000")
            .unwrap_or_else(|e| panic!("parse date: {e}"));

        assert_eq!(
            binary_plist_date(&date).unwrap_or_else(|e| panic!("plist date: {e}")),
            [
                0x62, 0x70, 0x6c, 0x69, 0x73, 0x74, 0x30, 0x30, 0x33, 0x41, 0xc5, 0xe0, 0x9b, 0x7f,
                0x80, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11,
            ]
        );
    }

    #[test]
    fn exif_original_date_requires_an_explicit_offset() -> std::io::Result<()> {
        let unknown = parse_exif_original_date(br#"[{"DateTimeOriginal":"2024:04:06 07:53:35"}]"#)?;
        assert_eq!(unknown, None);
        let date = parse_exif_original_date(
            br#"[{"DateTimeOriginal":"2024:04:06 07:53:35","OffsetTimeOriginal":"-07:00"}]"#,
        )?
        .ok_or_else(|| std::io::Error::other("missing explicit offset date"))?;
        assert_eq!(date.to_rfc3339(), "2024-04-06T14:53:35+00:00");
        assert!(
            parse_exif_original_date(
                br#"[{"DateTimeOriginal":"2024:04:06 07:53:35","OffsetTimeOriginal":"+99:00"}]"#
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn exiftool_original_date_is_independent_of_process_timezone() -> anyhow::Result<()> {
        if !crate::test_ci_contract::exiftool_available_or_ci_panic() {
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let jpeg = root.path().join("synthetic.jpg");
        image::RgbImage::from_pixel(8, 8, image::Rgb([23, 57, 101])).save(&jpeg)?;
        for offset in ["", "+08:00"] {
            let write = crate::ExiftoolBuilder::new()
                .overwrite_original()
                .arg("-EXIF:DateTimeOriginal=2024:04:06 07:53:35")
                .arg(format!("-EXIF:OffsetTimeOriginal={offset}"))
                .arg(crate::path_safety::exiftool_path_arg(&jpeg).as_ref())
                .build()
                .output()?;
            anyhow::ensure!(write.status.success(), "synthetic EXIF write failed");
            for timezone in ["UTC", "America/Los_Angeles"] {
                let read = exif_original_date_command(&jpeg)
                    .env("TZ", timezone)
                    .output()?;
                anyhow::ensure!(read.status.success(), "synthetic EXIF read failed");
                let original =
                    parse_exif_original_date(&read.stdout)?.map(|date| date.to_rfc3339());
                assert_eq!(
                    original.as_deref(),
                    (offset == "+08:00").then_some("2024-04-05T23:53:35+00:00")
                );
            }
        }
        Ok(())
    }

    #[test]
    fn spotlight_content_creation_date_xattr_is_written_from_resolved_source_date() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let src = dir.path().join("source.jpg");
        let dst = dir.path().join("output.jxl");
        std::fs::write(&src, b"source").unwrap_or_else(|e| panic!("write source: {e}"));
        std::fs::write(&dst, b"output").unwrap_or_else(|e| panic!("write output: {e}"));
        let (expected_date, _) =
            source_content_creation_date(&src).unwrap_or_else(|e| panic!("source date: {e}"));

        apply_spotlight_content_creation_date(&src, &dst)
            .unwrap_or_else(|e| panic!("apply Spotlight content creation date: {e}"));

        let actual = xattr::get(&dst, XATTR_CONTENT_CREATION_DATE)
            .unwrap_or_else(|e| panic!("read xattr: {e}"))
            .unwrap_or_else(|| panic!("content creation date xattr missing"));
        let expected_payload = binary_plist_date(&expected_date)
            .unwrap_or_else(|e| panic!("expected plist date: {e}"));
        assert_eq!(actual, expected_payload);
    }
}

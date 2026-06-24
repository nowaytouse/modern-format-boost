//! macOS native metadata preservation
//!
//! Uses `unsafe` only for FFI to system C APIs (`copyfile`, `getattrlist`).
//! Invariants: `CStrings` and pointers are valid for the duration of each call;
//! paths come from Rust `Path`.

use crate::builder_base::ToolBuilder;
use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

// copyfile.h constants
const COPYFILE_ACL: u32 = 1 << 0; // 0x1
const COPYFILE_STAT: u32 = 1 << 1; // 0x2
const COPYFILE_XATTR: u32 = 1 << 2; // 0x4
const COPYFILE_RECURSIVE: u32 = 1 << 15; // 0x8000

// COPYFILE_METADATA = COPYFILE_STAT | COPYFILE_ACL | COPYFILE_XATTR
const COPYFILE_FLAGS: u32 = COPYFILE_STAT | COPYFILE_ACL | COPYFILE_XATTR | COPYFILE_RECURSIVE;

pub(super) fn copy_native_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    unsafe extern "C" {
        fn copyfile(
            from: *const i8,
            to: *const i8,
            state: *mut std::ffi::c_void,
            flags: u32,
        ) -> i32;
    }
    let src_c = CString::new(src.as_os_str().as_bytes())?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())?;
    // SAFETY: CStrings are valid until the end of the block; copyfile does not
    // capture pointers.
    let ret = unsafe {
        copyfile(
            src_c.as_ptr(),
            dst_c.as_ptr(),
            std::ptr::null_mut(),
            COPYFILE_FLAGS,
        )
    };
    if ret < 0_i32 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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
    unsafe extern "C" {
        fn getattrlist(
            path: *const i8,
            attrList: *mut Attrlist,
            attrBuf: *mut std::ffi::c_void,
            attrBufSize: usize,
            options: u32,
        ) -> i32;
    }
    let c_path = CString::new(path.as_os_str().as_bytes())?;
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
    // SAFETY: c_path and &mut attr_list / &mut buf are valid; getattrlist is
    // synchronous and does not retain pointers.
    let ret = unsafe {
        getattrlist(
            c_path.as_ptr(),
            &raw mut attr_list,
            buf.as_mut_ptr().cast::<std::ffi::c_void>(),
            buf.len(),
            0,
        )
    };
    if ret != 0_i32 {
        return Err(io::Error::last_os_error());
    }
    parse_getattrlist_time_buffer(&buf, "added_time")
}

pub(super) fn apply_spotlight_content_creation_date(src: &Path, dst: &Path) -> io::Result<()> {
    let content_date = source_content_creation_date(src)?;
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
        Ok(Some(actual)) if actual == payload => Ok(()),
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

fn source_content_creation_date(src: &Path) -> io::Result<chrono::DateTime<chrono::Utc>> {
    if let Some(date) = read_mdls_content_creation_date(src)? {
        return Ok(date);
    }
    if let Some(date) = read_exif_datetime_original(src)? {
        return Ok(date);
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
    Ok(chrono::DateTime::<chrono::Utc>::from(created))
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
    let output = crate::ExiftoolBuilder::new()
        .arg("-s3")
        .arg("-d")
        .arg("%Y-%m-%d %H:%M:%S%z")
        .arg("-DateTimeOriginal")
        .arg(crate::path_safety::exiftool_path_arg(src).as_ref())
        .build()
        .output()
        .map_err(|e| {
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
    let raw = String::from_utf8_lossy(&output.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    parse_exiftool_datetime(trimmed).map(Some)
}

fn parse_spotlight_utc_date(raw: &str) -> io::Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S %z")
        .map(|date| date.with_timezone(&chrono::Utc))
        .map_err(|e| io::Error::other(format!("invalid Spotlight UTC date '{raw}': {e}")))
}

fn parse_exiftool_datetime(raw: &str) -> io::Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S%z")
        .map(|date| date.with_timezone(&chrono::Utc))
        .map_err(|e| io::Error::other(format!("invalid EXIF DateTimeOriginal '{raw}': {e}")))
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
    unsafe extern "C" {
        fn setattrlist(
            path: *const i8,
            attrList: *mut Attrlist,
            attrBuf: *mut std::ffi::c_void,
            attrBufSize: usize,
            options: u32,
        ) -> i32;
    }
    let c_path = CString::new(path.as_os_str().as_bytes())?;
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
    // SAFETY: c_path and local buffers are valid; setattrlist is synchronous and
    // does not retain pointers.
    let ret = unsafe {
        setattrlist(
            c_path.as_ptr(),
            &raw mut attr_list,
            (&raw mut buf).cast::<std::ffi::c_void>(),
            std::mem::size_of::<Timespec>(),
            0,
        )
    };
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
        get_added_time, parse_spotlight_utc_date, set_added_time, source_content_creation_date,
    };

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
    fn spotlight_content_creation_date_xattr_is_written_from_resolved_source_date() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let src = dir.path().join("source.jpg");
        let dst = dir.path().join("output.jxl");
        std::fs::write(&src, b"source").unwrap_or_else(|e| panic!("write source: {e}"));
        std::fs::write(&dst, b"output").unwrap_or_else(|e| panic!("write output: {e}"));
        let expected_date =
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

//! Network & cloud-related metadata preservation (macOS xattrs).
//!
//! Copies AND verifies network/download-related extended attributes from src to dst.
//!
//! **Handled xattrs:**
//! - `com.apple.metadata:kMDItemWhereFroms` — download source URL (copied)
//! - `com.apple.metadata:kMDItemUserTags`   — Finder tags (copied)
//! - `com.apple.quarantine`                 — quarantine flag (intentionally NOT copied)

use std::io;
use std::path::Path;

/// Copy critical network/cloud xattrs from `src` to `dst`, then verify.
/// `com.apple.quarantine` is intentionally skipped (security boundary).
/// All errors are tolerated — missing xattr support on the filesystem is not fatal.
pub fn preserve_network_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    // Keys to copy (quarantine intentionally excluded)
    const COPY_KEYS: &[&str] = &[
        "com.apple.metadata:kMDItemWhereFroms",
        "com.apple.metadata:kMDItemUserTags",
    ];

    for &key in COPY_KEYS {
        match xattr::get(src, key) {
            Ok(Some(value)) => {
                if let Err(e) = xattr::set(dst, key, &value) {
                    // Non-fatal: target filesystem may not support xattrs (e.g. FAT32, some network mounts)
                    eprintln!("⚠️ [metadata] Could not copy xattr '{}': {}", key, e);
                }
            }
            Ok(None) => {} // not present on source, nothing to do
            Err(_) => {}   // xattr not supported on this platform/fs, skip silently
        }
    }

    // Verify after copy
    for &key in COPY_KEYS {
        if let Ok(Some(_)) = xattr::get(src, key) {
            if xattr::get(dst, key).ok().flatten().is_none() {
                eprintln!("⚠️ [metadata] xattr '{}' present on source but missing on destination after copy attempt.", key);
            }
        }
    }

    Ok(())
}

/// Legacy alias kept for call-site compatibility.
#[allow(dead_code)]
#[inline]
pub fn verify_network_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    preserve_network_metadata(src, dst)
}

//! Linux-specific metadata preservation

use std::io;
use std::path::Path;
use std::process::Command;

pub fn preserve_linux_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    if which::which("getfacl").is_ok() && which::which("setfacl").is_ok() {
        let output = Command::new("getfacl")
            .arg("--absolute-names")
            .arg(src)
            .output()?;
        if output.status.success() {
            let acl_text = String::from_utf8_lossy(&output.stdout);
            // Apply each ACL entry directly to dst using setfacl -m
            let entries: Vec<&str> = acl_text
                .lines()
                .filter(|l| !l.starts_with('#') && !l.is_empty())
                .collect();
            for entry in entries {
                let _ = Command::new("setfacl")
                    .arg("-m")
                    .arg(entry)
                    .arg(dst)
                    .output();
            }
        }
    }
    Ok(())
}

/// Try to set birth time on Linux (best-effort, will silently fail on most systems)
/// Linux typically doesn't support setting birth time, but we try anyway for completeness
pub fn try_set_birth_time(_path: &Path, _time: std::time::SystemTime) -> io::Result<()> {
    // Linux doesn't provide a standard way to set birth time (btime)
    // Most filesystems (ext4, xfs, btrfs) track it but don't allow modification
    // This is a no-op placeholder for API consistency
    Ok(())
}

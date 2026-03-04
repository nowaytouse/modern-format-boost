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

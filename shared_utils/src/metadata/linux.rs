//! Linux-specific metadata preservation

use std::io;
use std::path::Path;
use std::process::Command;

pub fn preserve_linux_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    // ACL preservation via getfacl/setfacl --restore (more complete than -m per-entry)
    if which::which("getfacl").is_ok() && which::which("setfacl").is_ok() {
        let output = Command::new("getfacl")
            .arg("--absolute-names")
            .arg(src)
            .output();
        if let Ok(out) = output {
            if out.status.success() {
                // Rewrite the path header so setfacl --restore targets dst
                let acl_text = String::from_utf8_lossy(&out.stdout);
                let dst_str = dst.to_string_lossy();
                let rewritten: String = acl_text
                    .lines()
                    .map(|line| {
                        if line.starts_with("# file:") {
                            format!("# file: {}\n", dst_str)
                        } else {
                            format!("{}\n", line)
                        }
                    })
                    .collect();

                // Feed rewritten ACL to setfacl --restore via stdin
                use std::io::Write;
                let mut child = Command::new("setfacl")
                    .arg("--restore=-")
                    .stdin(std::process::Stdio::piped())
                    .spawn();
                if let Ok(ref mut child) = child {
                    if let Some(stdin) = child.stdin.take() {
                        let mut stdin = stdin;
                        let _ = stdin.write_all(rewritten.as_bytes());
                    }
                    let _ = child.wait();
                }
            }
        }
    }

    // Unix permission bits
    if let Ok(meta) = std::fs::metadata(src) {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode));
    }

    Ok(())
}

/// Try to set birth time on Linux (best-effort, no-op on most filesystems).
pub fn try_set_birth_time(_path: &Path, _time: std::time::SystemTime) -> io::Result<()> {
    // Linux doesn't provide a standard way to set birth time (btime).
    // Most filesystems track it but don't allow modification via userspace.
    Ok(())
}

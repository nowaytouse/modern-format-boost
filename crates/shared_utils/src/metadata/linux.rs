//! Linux-specific metadata preservation

use std::io;
use std::path::Path;

pub fn preserve_linux_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    // ACL preservation via getfacl/setfacl --restore (more complete than -m per-entry)
    if which::which("getfacl").is_ok() && which::which("setfacl").is_ok() {
        let output = crate::tool_builders::AclBuilder::getfacl()
            .input(src)
            .build()
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
                let mut child = crate::tool_builders::AclBuilder::restore()
                    .build()
                    .stdin(std::process::Stdio::piped())
                    .spawn()?;

                if let Some(mut stdin) = child.stdin.take() {
                    if let Err(e) = stdin.write_all(rewritten.as_bytes()) {
                        crate::log_anomaly!(
                            crate::static_logs::messages::LABEL_METADATA,
                            &format!(
                                "Failed to write ACL data to setfacl for {path}: {e}",
                                path = dst.display()
                            )
                        );
                    }
                }

                match child.wait() {
                    Ok(status) if !status.success() => {
                        crate::log_anomaly!(
                            crate::static_logs::messages::LABEL_METADATA,
                            &format!(
                                "setfacl --restore returned non-zero status for {path}",
                                path = dst.display()
                            )
                        );
                    }
                    Err(e) => {
                        crate::log_anomaly!(
                            crate::static_logs::messages::LABEL_METADATA,
                            &format!(
                                "Failed waiting for setfacl while restoring {path}: {e}",
                                path = dst.display()
                            )
                        );
                    }
                    _ => {}
                }
            } else {
                crate::log_anomaly!(
                    crate::static_logs::messages::LABEL_METADATA,
                    &format!(
                        "getfacl returned non-zero status for {path}: {err}",
                        path = src.display(),
                        err = String::from_utf8_lossy(&out.stderr).trim()
                    )
                );
            }
        } else if let Err(e) = output {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!(
                    "Failed to launch getfacl for {path}: {e}",
                    path = src.display()
                )
            );
        }
    }

    // Unix permission bits
    if let Ok(meta) = std::fs::metadata(src) {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if let Err(e) = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(mode)) {
            crate::log_anomaly!(
                crate::static_logs::messages::LABEL_METADATA,
                &format!(
                    "Failed to preserve Linux permission bits for {path}: {e}",
                    path = dst.display()
                )
            );
        }
    }

    Ok(())
}

/// Try to set birth time on Linux (best-effort, no-op on most filesystems).
pub fn try_set_birth_time(_path: &Path, _time: std::time::SystemTime) -> io::Result<()> {
    // Linux doesn't provide a standard way to set birth time (btime).
    // Most filesystems track it but don't allow modification via userspace.
    Ok(())
}

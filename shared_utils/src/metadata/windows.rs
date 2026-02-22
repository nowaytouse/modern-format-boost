//! Windows-specific metadata preservation

use std::io;
use std::path::Path;
use std::process::Command;

pub fn preserve_windows_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    if which::which(\"powershell\").is_ok() {
        // Use -LiteralPath to avoid command injection via filenames containing quotes
        let ps_script = format!(
            "Get-Acl -LiteralPath '{}' | Set-Acl -LiteralPath '{}'",
            src.to_string_lossy().replace('\'', "''"),
            dst.to_string_lossy().replace('\'', "''")
        );
        let _ = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(ps_script)
            .output();
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(src) {
            let file_attrs = meta.file_attributes();
            let is_hidden = (file_attrs & 0x2) != 0;
            let is_system = (file_attrs & 0x4) != 0;
            let mut cmd = Command::new("attrib");
            if is_hidden {
                cmd.arg("+h");
            }
            if is_system {
                cmd.arg("+s");
            }
            cmd.arg(dst);
            let _ = cmd.output();
        }
    }
    Ok(())
}

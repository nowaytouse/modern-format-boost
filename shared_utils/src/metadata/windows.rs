//! Windows-specific metadata preservation

use std::path::Path;
use std::process::Command;
use std::io;

pub fn preserve_windows_attributes(src: &Path, dst: &Path) -> io::Result<()> {
    // ACLs via PowerShell
    if which::which("powershell").is_ok() {
        let ps_script = format!("Get-Acl -Path '{}' | Set-Acl -Path '{}'", src.to_string_lossy(), dst.to_string_lossy());
        let _ = Command::new("powershell").arg("-Command").arg(ps_script).output();
    }

    // File attributes
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(src) {
            let file_attrs = meta.file_attributes();
            let is_hidden = (file_attrs & 0x2) != 0;
            let is_system = (file_attrs & 0x4) != 0;
            let mut cmd = Command::new("attrib");
            if is_hidden { cmd.arg("+h"); }
            if is_system { cmd.arg("+s"); }
            cmd.arg(dst);
            let _ = cmd.output();
        }
    }
    Ok(())
}

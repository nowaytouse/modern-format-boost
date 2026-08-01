fn main() -> Result<(), Box<dyn core::error::Error>> {
    // macOS Homebrew and Linker Workarounds
    if cfg!(target_os = "macos") {
        let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
            Ok(val) => val,
            Err(e) => return Err(Box::new(e)),
        };
        let Some(_workspace_root) = std::path::Path::new(&manifest_dir)
            .parent()
            .and_then(|path| path.parent())
        else {
            return Err(Box::from(
                "Could not find workspace root from manifest directory",
            ));
        };

        let home_root = std::env::var("MFB_HOME_ROOT")
            .map(std::path::PathBuf::from)
            .or_else(|_| {
                std::env::var("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".modern_format_boost"))
            })
            .unwrap_or_else(|_| std::env::temp_dir().join(".modern_format_boost"));
        let tmp_lib = home_root.join(".tmp_lib");
        if tmp_lib.exists() {
            println!("cargo:rustc-link-search=native={}", tmp_lib.display());
        }

        // Homebrew paths
        let homebrew_lib = if cfg!(target_arch = "aarch64") {
            "/opt/homebrew/lib"
        } else {
            "/usr/local/lib"
        };
        println!("cargo:rustc-link-search=native={homebrew_lib}");

        // Ensure we link to libc++ specifically on macOS
        println!("cargo:rustc-link-lib=c++");
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    // macOS Homebrew and Linker Workarounds
    if cfg!(target_os = "macos") {
        let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
            Ok(val) => val,
            Err(e) => return Err(Box::new(e)),
        };
        let Some(workspace_root) = std::path::Path::new(&manifest_dir)
            .parent()
            .and_then(|path| path.parent())
        else {
            return Err(Box::from(
                "Could not find workspace root from manifest directory",
            ));
        };

        // Link to the .tmp_lib directory which contains the libstdc++ -> libc++
        // workaround
        let tmp_lib = workspace_root.join("crates/.modern_format_boost/.tmp_lib");
        if tmp_lib.exists() {
            println!("cargo:rustc-link-search=native={}", tmp_lib.display());
        }

        // Add standard Homebrew paths as fallback
        let homebrew_lib = if cfg!(target_arch = "aarch64") {
            "/opt/homebrew/lib"
        } else {
            "/usr/local/lib"
        };
        println!("cargo:rustc-link-search=native={homebrew_lib}");

        // Ensure we link to libc++ specifically on macOS
        println!("cargo:rustc-link-lib=c++");

        // Link to encoders that libheif might depend on
        println!("cargo:rustc-link-lib=x264");
        println!("cargo:rustc-link-lib=vvenc");
        println!("cargo:rustc-link-lib=vvdec");
    }
    Ok(())
}

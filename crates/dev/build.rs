fn main() {
    // macOS Homebrew and Linker Workarounds
    if cfg!(target_os = "macos") {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let workspace_root = std::path::Path::new(&manifest_dir)
            .parent()
            .and_then(|p| p.parent())
            .unwrap();
        
        // Link to the .tmp_lib directory which contains the libstdc++ -> libc++ workaround
        let tmp_lib = workspace_root.join(".tmp_lib");
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

        // Transitive dependencies for libheif on macOS (Homebrew)
        println!("cargo:rustc-link-lib=x264");
        println!("cargo:rustc-link-lib=vvenc");
        println!("cargo:rustc-link-lib=vvdec");
        println!("cargo:rustc-link-lib=aom");
        println!("cargo:rustc-link-lib=dav1d");
        println!("cargo:rustc-link-lib=rav1e");
    }
}

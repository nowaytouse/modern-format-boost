// Build script for imgquality-hevc
// Dynamically detect system library paths for dav1d and libheif

fn main() {
    // macOS Homebrew paths
    if cfg!(target_os = "macos") {
        // Use pkg-config or fallback to common paths
        let homebrew_lib = if cfg!(target_arch = "aarch64") {
            "/opt/homebrew/lib"
        } else {
            "/usr/local/lib"
        };

        println!("cargo:rustc-link-search=native={}", homebrew_lib);

        // Also check for libheif
        let homebrew_opt = if cfg!(target_arch = "aarch64") {
            "/opt/homebrew/opt"
        } else {
            "/usr/local/opt"
        };

        println!(
            "cargo:rustc-link-search=native={}/libheif/lib",
            homebrew_opt
        );
    }
}

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

        // Homebrew openh264 2.6.0 currently publishes `-lstdc++` in its
        // static pkg-config metadata even on Apple targets. libheif's embedded
        // build consumes that metadata under `ci-static-build`; provide the
        // expected name as a private SDK stub without downgrading dependencies
        // or mutating the system/Homebrew installation.
        if std::env::var_os("CARGO_FEATURE_CI_STATIC_BUILD").is_some() {
            let sdk_root = if let Some(root) = std::env::var_os("SDKROOT") {
                std::path::PathBuf::from(root)
            } else {
                let output = std::process::Command::new("xcrun")
                    .args(["--sdk", "macosx", "--show-sdk-path"])
                    .output()?;
                if !output.status.success() {
                    return Err(Box::from(
                        "xcrun could not resolve the macOS SDK for the libstdc++ compatibility stub",
                    ));
                }
                std::path::PathBuf::from(String::from_utf8(output.stdout)?.trim())
            };
            let libcxx_stub = sdk_root.join("usr/lib/libc++.tbd");
            if !libcxx_stub.is_file() {
                return Err(Box::from(format!(
                    "macOS libc++ SDK stub is missing: {}",
                    libcxx_stub.display()
                )));
            }
            let out_dir = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or(
                "OUT_DIR is unavailable while preparing the libstdc++ compatibility stub",
            )?);
            let compatibility_stub = out_dir.join("libstdc++.tbd");
            std::fs::copy(&libcxx_stub, &compatibility_stub)?;
            println!("cargo:rustc-link-search=native={}", out_dir.display());
        }
    }
    Ok(())
}

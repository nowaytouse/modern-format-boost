//! Modern Format Boost - Dependency Installer in Rust.
//! Installs macOS and Linux system dependencies, Rust utilities, Python
//! packages, and Node tools.

use anyhow::{Context, Result, anyhow};
use dev::infra::ui_tokens::pick_symbol;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// Colors
const GREEN: &str = "\x1b[0;32m";
const BLUE: &str = "\x1b[0;34m";
const YELLOW: &str = "\x1b[1;33m";
const RED: &str = "\x1b[0;31m";
const DIM: &str = "\x1b[2m";
const NC: &str = "\x1b[0m";

fn print_c(color: &str, text: &str) {
    println!("{color}{text}{NC}");
}

fn command_exists(cmd: &str) -> bool {
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            let candidate = path.join(cmd);
            if is_executable_file(&candidate) {
                return true;
            }
        }
    }
    false
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        dev::infra::hardening::path_is_executable(path)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

const fn post_install_check_hint() -> &'static str {
    "You can now run 'cargo run --locked -p dev --bin check_all' to verify the workspace."
}

fn run_cmd(cmd: &str, check: bool) -> Result<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("Failed to execute command: {cmd}"))?;

    if check && !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(anyhow!(
            "Command '{}' failed with status: {} ({})",
            cmd,
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn command_status_success(cmd: &str) -> Result<bool> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("Failed to execute command: {cmd}"))?;
    Ok(output.status.success())
}

fn main() -> Result<()> {
    print_c(
        BLUE,
        &format!(
            "{} Modern Format Boost - Dependency Installer v0.11.3",
            pick_symbol("🚀", "[LAUNCH]")
        ),
    );
    println!("--------------------------------------------------------");
    print_c(
        DIM,
        "💡 For advanced FFmpeg setup (FDK-AAC, AI filters, etc.), see script header.",
    );
    println!("--------------------------------------------------------\n");

    let os_type = std::env::consts::OS;

    if os_type == "macos" {
        print_c(
            YELLOW,
            &format!("{} Detected macOS", pick_symbol("🍎", "[APPLE]")),
        );
        if !command_exists("brew") {
            println!("Installing Homebrew...");
            run_cmd(
                r#"/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)""#,
                true,
            )?;
        }
        println!("Updating Homebrew...");
        run_cmd("brew update", true)?;
        println!("Checking and installing system dependencies via Homebrew...");

        let deps = vec![
            "jpeg-xl",       // cjxl, djxl, jxlinfo
            "jpeginfo",      // JPEG structural validator
            "pngcheck",      // PNG chunk/CRC validator
            "exiftool",      // Metadata preservation
            "exiv2",         // Metadata inspection/validation
            "imagemagick",   // Image format conversion (magick)
            "webp",          // WebP support (dwebp, cwebp)
            "libavif",       // AVIF decoder/validator
            "libheif",       // HEIF/HEIC support
            "coreutils",     // GNU core utilities
            "node",          // Node.js for prettier/markdownlint
            "shellcheck",    // Shell script linting
            "shfmt",         // Shell script formatting
            "postgresql@14", // Database for ML training
            "pgvector",      // Vector similarity search extension
            "chromaprint",   // Audio fingerprinting
            "libvmaf",       // Video quality metrics
            "x264",          // H.264 encoder
            "vvdec",         // VVC decoder
            "vvenc",         // VVC encoder
        ];

        // Handle ffmpeg specially to avoid tap conflicts
        if command_exists("ffmpeg") {
            let ffmpeg_info = run_cmd("which ffmpeg", false)?;
            print_c(
                GREEN,
                &format!(
                    "{} ffmpeg already installed at: {}",
                    pick_symbol("✅", "[OK]"),
                    ffmpeg_info.trim()
                ),
            );
            print_c(DIM, "   Skipping to preserve existing installation.");
        } else {
            println!("Installing ffmpeg (standard version)...");
            print_c(
                DIM,
                "   💡 For full-featured ffmpeg, see script header for homebrew-ffmpeg tap \
                 instructions.",
            );
            run_cmd("brew install ffmpeg", true)?;
        }

        for dep in deps {
            let mut binary = dep;
            if dep == "postgresql@14" {
                binary = "psql";
            } else if dep == "jpeg-xl" {
                binary = "cjxl";
            } else if dep == "libheif" {
                binary = "heif-convert";
            } else if dep == "libavif" {
                binary = "avifdec";
            } else if dep == "libvmaf" {
                // libvmaf is a library, check via pkg-config
                if command_status_success("pkg-config --exists libvmaf")? {
                    print_c(
                        GREEN,
                        &format!("{} {} already installed.", pick_symbol("✅", "[OK]"), dep),
                    );
                    continue;
                }
                binary = "";
            }

            if !binary.is_empty() && command_exists(binary) {
                print_c(
                    GREEN,
                    &format!("{} {} already installed.", pick_symbol("✅", "[OK]"), dep),
                );
            } else {
                println!("Installing {dep}...");
                run_cmd(&format!("brew install {dep}"), true)?;
            }
        }

        // --- macOS Linker Workaround for libstdc++ ---
        print_c(
            BLUE,
            &format!(
                "{} Applying macOS linker workaround for libstdc++...",
                pick_symbol("🔧", "[TOOL]")
            ),
        );
        let tmp_lib_dir = Path::new("crates/.modern_format_boost/.tmp_lib");
        if !tmp_lib_dir.exists() {
            fs::create_dir_all(tmp_lib_dir)?;
        }

        // 1. Create libstdc++.tbd pointing to system libc++.tbd in the SDK
        let sdk_path = run_cmd("xcrun --show-sdk-path", true)?;
        let sdk_path_trimmed = sdk_path.trim();
        let libcxx_tbd = PathBuf::from(sdk_path_trimmed).join("usr/lib/libc++.tbd");
        let target_tbd = tmp_lib_dir.join("libstdc++.tbd");
        if libcxx_tbd.exists() {
            run_cmd(
                &format!(
                    "ln -sf \"{}\" \"{}\"",
                    libcxx_tbd.display(),
                    target_tbd.display()
                ),
                true,
            )?;
            print_c(
                GREEN,
                &format!(
                    "   {} Linked libstdc++.tbd -> {}",
                    pick_symbol("✅", "[OK]"),
                    libcxx_tbd.display()
                ),
            );
        } else {
            print_c(
                YELLOW,
                "   ⚠️  System libc++.tbd not found in SDK. Doctests might fail.",
            );
        }

        // 2. Create libstdc++.dylib pointing to system libc++.dylib
        let target_dylib = tmp_lib_dir.join("libstdc++.dylib");
        run_cmd(
            &format!(
                "ln -sf \"/usr/lib/libc++.dylib\" \"{}\"",
                target_dylib.display()
            ),
            true,
        )?;
        print_c(
            GREEN,
            &format!(
                "   {} Linked libstdc++.dylib -> /usr/lib/libc++.dylib",
                pick_symbol("✅", "[OK]")
            ),
        );
    } else if os_type == "linux" {
        print_c(
            YELLOW,
            &format!("{} Detected Linux", pick_symbol("🐧", "[LINUX]")),
        );
        if command_exists("apt-get") {
            println!("Installing system dependencies via apt...");
            run_cmd("sudo apt-get update", true)?;
            run_cmd(
                "sudo apt-get install -y ffmpeg libimage-exiftool-perl imagemagick webp \
                 libheif-dev libavif-bin jpeginfo pngcheck exiv2 coreutils nodejs npm shellcheck \
                 shfmt curl git build-essential postgresql postgresql-contrib libchromaprint-dev \
                 libvmaf-dev pkg-config",
                true,
            )?;

            // Check for libjxl (JPEG XL)
            if !command_exists("cjxl") {
                print_c(
                    YELLOW,
                    &format!(
                        "{} JPEG XL tools not found in apt.",
                        pick_symbol("⚠️", "[WARN]")
                    ),
                );
                println!("   You may need to build from source or use a PPA:");
                println!("   https://github.com/libjxl/libjxl");
            }
        } else {
            print_c(
                RED,
                "❌ Unsupported Linux distribution (apt not found). Please install dependencies \
                 manually.",
            );
            std::process::exit(1);
        }
    } else {
        print_c(
            RED,
            &format!(
                "{} Unsupported OS: {}",
                pick_symbol("❌", "[ERROR]"),
                os_type
            ),
        );
        std::process::exit(1);
    }

    if command_exists("rustup") {
        print_c(GREEN, &format!("{} Rust found.", pick_symbol("✅", "[OK]")));
    } else {
        print_c(
            YELLOW,
            &format!(
                "{} Rust not found. Installing via rustup...",
                pick_symbol("🦀", "[RUST]")
            ),
        );
        run_cmd(
            "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y",
            true,
        )?;
        let home = match std::env::var("HOME") {
            Ok(h) => h,
            Err(e) => return Err(anyhow!("HOME environment variable not set: {e}")),
        };
        let path_var = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{home}/.cargo/bin:{path_var}");
        unsafe {
            std::env::set_var("PATH", new_path);
        }
    }

    println!("Updating Rust and adding components...");
    run_cmd("rustup update", true)?;
    run_cmd("rustup component add clippy rustfmt", true)?;

    print_c(
        BLUE,
        &format!(
            "{} Installing Cargo utilities...",
            pick_symbol("📦", "[PKG]")
        ),
    );
    let cargo_tools = vec![
        ("cargo-nextest", "cargo-nextest"),
        ("taplo-cli", "taplo"),
        ("cargo-bloat", "cargo-bloat"),
        ("cargo-hack", "cargo-hack"),
        ("cargo-audit", "cargo-audit"),
        ("dovi_tool", "dovi_tool"),
        ("hdr10plus_tool", "hdr10plus_tool"),
        ("kondo", "kondo"),
    ];

    for (package, binary) in cargo_tools {
        if command_exists(binary) {
            print_c(
                GREEN,
                &format!(
                    "{} {} already installed.",
                    pick_symbol("✅", "[OK]"),
                    package
                ),
            );
        } else {
            println!("Installing {package}...");
            let _ = run_cmd(&format!("cargo install {package}"), false);
        }
    }

    print_c(
        BLUE,
        &format!(
            "{} Installing Python utilities...",
            pick_symbol("🐍", "[PYTHON]")
        ),
    );
    if command_exists("pip3") {
        let python_packages = [
            "ruff",
            "rich",
            "psycopg2-binary",
            "tabulate",
            "numpy",
            "pandas",
            "scikit-learn",
            "Pillow",
        ];
        println!("   Installing: {}", python_packages.join(", "));
        let _ = run_cmd(
            &format!("pip3 install --upgrade {}", python_packages.join(" ")),
            false,
        );
    } else {
        print_c(
            RED,
            &format!(
                "{} pip3 not found. Skipping Python tools.",
                pick_symbol("⚠️", "[WARN]")
            ),
        );
    }

    print_c(
        BLUE,
        &format!(
            "{} Installing Node.js utilities...",
            pick_symbol("🟢", "[NODE]")
        ),
    );
    if command_exists("npm") {
        println!("Installing prettier and markdownlint-cli2 globally...");
        if os_type == "linux" {
            let _ = run_cmd("sudo npm install -g prettier markdownlint-cli2", false);
        } else {
            let _ = run_cmd("npm install -g prettier markdownlint-cli2", false);
        }
    } else {
        print_c(
            RED,
            &format!(
                "{} npm not found. Skipping Node tools.",
                pick_symbol("⚠️", "[WARN]")
            ),
        );
    }

    println!("--------------------------------------------------------");
    print_c(
        GREEN,
        &format!(
            "{} All dependencies installed successfully!",
            pick_symbol("🌟", "[STAR]")
        ),
    );
    println!("{}", post_install_check_hint());
    print_c(
        DIM,
        "\n💡 Tip: For advanced FFmpeg features, see the script header for tap instructions.",
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_command_exists() {
        // "sh" is standard on all macOS/Linux systems and should exist.
        assert!(command_exists("sh") || command_exists("bash"));

        // A randomly named binary should not exist.
        assert!(!command_exists("non_existent_binary_foo_bar_12345"));
    }

    #[test]
    fn test_command_status_success_preserves_nonzero_status() -> Result<()> {
        assert!(command_status_success("exit 0")?);
        assert!(!command_status_success("exit 7")?);
        Ok(())
    }

    #[test]
    fn test_command_exists_requires_executable_like_shutil_which() -> Result<()> {
        let tempdir = tempfile::tempdir()?;
        let fake_tool = tempdir.path().join("fake-tool");
        let mut file = fs::File::create(&fake_tool)?;
        writeln!(file, "#!/bin/sh")?;

        let old_path = std::env::var_os("PATH").unwrap_or_default();
        unsafe {
            std::env::set_var("PATH", tempdir.path());
        }
        let exists = command_exists("fake-tool");
        unsafe {
            std::env::set_var("PATH", old_path);
        }

        assert!(!exists, "non-executable files must not count as commands");
        Ok(())
    }

    #[test]
    fn test_post_install_hint_no_longer_points_to_deleted_python_auditor() {
        assert!(!post_install_check_hint().contains("check_all.py"));
        assert!(post_install_check_hint().contains("cargo run --locked -p dev --bin check_all"));
    }
}

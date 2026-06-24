//! Build and install swift-corelibs-libdispatch for non-Apple CI hosts.

use anyhow::{Context, Result, bail};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

fn env_path(key: &str, default: &str) -> PathBuf {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(default), PathBuf::from)
}

fn run(program: &str, args: &[String]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("run {program} {}", args.join(" ")))?;
    if !status.success() {
        bail!("{program} {} failed with status {status}", args.join(" "));
    }
    Ok(())
}

fn command_exists(cmd: &str) -> bool {
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            if path.join(cmd).is_file() {
                return true;
            }
        }
    }
    false
}

fn installed(prefix: &Path) -> bool {
    prefix.join("lib/libdispatch.so").is_file() || prefix.join("lib/libdispatch.dylib").is_file()
}

const fn help_text() -> &'static str {
    "Build and install swift-corelibs-libdispatch for non-Apple CI hosts.\n\nUsage: \
     build_libdispatch\n\nEnvironment:\n  LIBDISPATCH_PREFIX    Install prefix [default: \
     /usr/local]\n  LIBDISPATCH_SRC_DIR   Source checkout dir [default: \
     /tmp/swift-corelibs-libdispatch]\n  LIBDISPATCH_REF       Git ref/branch [default: main]\n  \
     CC                    C compiler [default: clang]\n  CXX                   C++ compiler \
     [default: clang++]"
}

fn print_help() {
    println!("{}", help_text());
}

fn main() -> Result<()> {
    if std::env::args_os()
        .skip(1)
        .any(|arg| arg == std::ffi::OsStr::new("--help") || arg == std::ffi::OsStr::new("-h"))
    {
        print_help();
        return Ok(());
    }

    let prefix = env_path("LIBDISPATCH_PREFIX", "/usr/local");
    let src_dir = env_path("LIBDISPATCH_SRC_DIR", "/tmp/swift-corelibs-libdispatch");
    let reference = std::env::var("LIBDISPATCH_REF").unwrap_or_else(|_| String::from("main"));

    if installed(&prefix) {
        println!("libdispatch already installed under {}", prefix.display());
        return Ok(());
    }

    if !src_dir.join(".git").is_dir() {
        run(
            "git",
            &[
                "clone".into(),
                "--depth".into(),
                "1".into(),
                "--branch".into(),
                reference,
                "https://github.com/apple/swift-corelibs-libdispatch.git".into(),
                src_dir.display().to_string(),
            ],
        )?;
    }

    let cc = std::env::var("CC").unwrap_or_else(|_| String::from("clang"));
    let cxx = std::env::var("CXX").unwrap_or_else(|_| String::from("clang++"));
    let build_dir = src_dir.join("build");
    fs::create_dir_all(&build_dir)
        .with_context(|| format!("create build dir {}", build_dir.display()))?;
    run(
        "cmake",
        &[
            "-S".into(),
            src_dir.display().to_string(),
            "-B".into(),
            build_dir.display().to_string(),
            "-DCMAKE_BUILD_TYPE=Release".into(),
            format!("-DCMAKE_INSTALL_PREFIX={}", prefix.display()),
            format!("-DCMAKE_C_COMPILER={cc}"),
            format!("-DCMAKE_CXX_COMPILER={cxx}"),
            "-DENABLE_SWIFT=OFF".into(),
            "-DENABLE_TESTS=OFF".into(),
        ],
    )?;
    let jobs = std::thread::available_parallelism()
        .map_or_else(|_| String::from("2"), |count| count.get().to_string());
    run(
        "cmake",
        &[
            "--build".into(),
            build_dir.display().to_string(),
            "--parallel".into(),
            jobs,
        ],
    )?;
    run(
        "sudo",
        &[
            "cmake".into(),
            "--install".into(),
            build_dir.display().to_string(),
        ],
    )?;
    if command_exists("ldconfig") {
        let _ = Command::new("sudo").arg("ldconfig").status();
    }

    if let Some(github_env) = std::env::var_os("GITHUB_ENV").map(PathBuf::from)
        && github_env.exists()
    {
        let mut file = OpenOptions::new()
            .append(true)
            .open(&github_env)
            .with_context(|| format!("open GITHUB_ENV {}", github_env.display()))?;
        let pkg_config = std::env::var("PKG_CONFIG_PATH").unwrap_or_else(|_| String::new());
        let ld_library = std::env::var("LD_LIBRARY_PATH").unwrap_or_else(|_| String::new());
        writeln!(
            file,
            "PKG_CONFIG_PATH={}/lib/pkgconfig:{pkg_config}",
            prefix.display()
        )?;
        writeln!(
            file,
            "LD_LIBRARY_PATH={}/lib:{ld_library}",
            prefix.display()
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_documents_env_contract() {
        let text = help_text();
        assert!(text.contains("LIBDISPATCH_PREFIX"));
        assert!(text.contains("LIBDISPATCH_SRC_DIR"));
        assert!(text.contains("LIBDISPATCH_REF"));
        assert!(text.contains("clang++"));
    }
}

//! Modern Format Boost - standalone Rust media dependency bootstrapper.
//! Uses only the standard library so CI can compile it with `rustc` before the
//! workspace's native media dependencies exist.

use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

const APT_PACKAGES: &[&str] = &[
    "libwebkit2gtk-4.1-dev",
    "libxdo-dev",
    "libssl-dev",
    "libayatana-appindicator3-dev",
    "librsvg2-dev",
    "libglib2.0-dev",
    "pkg-config",
    "clang",
    "cmake",
    "nasm",
    "ninja-build",
    "meson",
    "libgmp-dev",
    "libmpfr-dev",
    "libmpc-dev",
    "libjxl-dev",
    "libjxl-tools",
    "libnuma-dev",
    "libde265-dev",
    "libx264-dev",
    "libx265-dev",
    "libaom-dev",
    "libdav1d-dev",
    "libopus-dev",
    "libvpx-dev",
    "exiftool",
    "imagemagick",
    "jpeginfo",
    "pngcheck",
    "exiv2",
    "jhead",
    "libjpeg-turbo-progs",
    "libavif-bin",
    "curl",
    "build-essential",
];

struct TempDir(PathBuf);

impl TempDir {
    fn create() -> Result<Self> {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path = env::temp_dir().join(format!(
            "mfb-media-dependencies-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run<I, S>(program: &str, args: I, cwd: Option<&Path>) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    if let Some(directory) = cwd {
        command.current_dir(directory);
    }
    let rendered = format!("{command:?}");
    println!("Executing: {rendered}");
    let status = command.status().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to execute {rendered}: {error}"),
        )
    })?;
    if !status.success() {
        return Err(
            io::Error::other(format!("command failed with status {status}: {rendered}")).into(),
        );
    }
    Ok(())
}

fn capture<I, S>(program: &str, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args);
    let rendered = format!("{command:?}");
    println!("Executing: {rendered}");
    let output = command.output().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to execute {rendered}: {error}"),
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail = stderr
            .chars()
            .rev()
            .take(8_192)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>();
        return Err(io::Error::other(format!(
            "command failed with status {}: {rendered}\n{tail}",
            output.status
        ))
        .into());
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn require_output(output: &str, needle: &str, command: &str) -> Result<()> {
    if output.contains(needle) {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "{command} output is missing required capability: {needle}"
    ))
    .into())
}

fn download(url: &str, output: &Path, cwd: &Path) -> Result<()> {
    run(
        "curl",
        [
            OsStr::new("--fail"),
            OsStr::new("--location"),
            OsStr::new("--retry"),
            OsStr::new("3"),
            OsStr::new("--retry-all-errors"),
            OsStr::new("--connect-timeout"),
            OsStr::new("30"),
            OsStr::new("--output"),
            output.as_os_str(),
            OsStr::new(url),
        ],
        Some(cwd),
    )
}

fn prepend_media_paths() -> Result<()> {
    let pkg_paths = "/usr/local/lib/pkgconfig:/usr/local/lib/x86_64-linux-gnu/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig";
    let library_path = "/usr/local/lib";
    let existing_pkg = env::var("PKG_CONFIG_PATH").unwrap_or_default();
    let existing_ld = env::var("LD_LIBRARY_PATH").unwrap_or_default();
    let new_pkg = if existing_pkg.is_empty() {
        pkg_paths.to_string()
    } else {
        format!("{pkg_paths}:{existing_pkg}")
    };
    let new_ld = if existing_ld.is_empty() {
        library_path.to_string()
    } else {
        format!("{library_path}:{existing_ld}")
    };

    unsafe {
        env::set_var("PKG_CONFIG_PATH", &new_pkg);
        env::set_var("LD_LIBRARY_PATH", &new_ld);
    }

    if let Some(path) = env::var_os("GITHUB_ENV").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("failed to open GITHUB_ENV at {}: {error}", path.display()),
                )
            })?;
        writeln!(file, "PKG_CONFIG_PATH={new_pkg}")?;
        writeln!(file, "LD_LIBRARY_PATH={new_ld}")?;
    }
    Ok(())
}

fn install_gnu_mpc(workdir: &Path, workspace: &Path) -> Result<()> {
    let mpc_archive = match env::var_os("MFB_MPC_ARCHIVE").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => {
            let archive = workdir.join("mpc.tar.xz");
            let manifest = workspace.join("Cargo.toml");
            run(
                "cargo",
                [
                    OsStr::new("run"),
                    OsStr::new("--locked"),
                    OsStr::new("--manifest-path"),
                    manifest.as_os_str(),
                    OsStr::new("-p"),
                    OsStr::new("dev"),
                    OsStr::new("--bin"),
                    OsStr::new("download_gnu_mpc"),
                    OsStr::new("--"),
                    archive.as_os_str(),
                ],
                Some(workdir),
            )?;
            archive
        }
    };

    if !mpc_archive.is_file() || fs::metadata(&mpc_archive)?.len() == 0 {
        return Err(io::Error::other(format!(
            "MFB_MPC_ARCHIVE is missing or empty: {}",
            mpc_archive.display()
        ))
        .into());
    }

    run(
        "tar",
        [OsStr::new("xf"), mpc_archive.as_os_str()],
        Some(workdir),
    )?;
    let mpc_dir = workdir.join("mpc-1.4.1");
    run(
        "./configure",
        ["--prefix=/usr/local", "--with-gmp=/usr", "--with-mpfr=/usr"],
        Some(&mpc_dir),
    )?;
    let jobs = format!("-j{}", std::thread::available_parallelism()?.get());
    run("make", [jobs.as_str()], Some(&mpc_dir))?;
    run("sudo", ["make", "install"], Some(&mpc_dir))?;
    run("sudo", ["ldconfig"], None)?;
    prepend_media_paths()
}

fn main() -> Result<()> {
    let workspace = env::var_os("GITHUB_WORKSPACE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map_or_else(env::current_dir, Ok)?;
    let temp_dir = TempDir::create()?;
    let workdir = temp_dir.path();

    if env::var("MFB_MPC_ONLY").unwrap_or_default() == "1" {
        return install_gnu_mpc(workdir, &workspace);
    }

    println!("--- Installing System APT Dependencies ---");
    run("sudo", ["apt-get", "update", "-qq"], None)?;
    run(
        "sudo",
        ["apt-get", "install", "-y"]
            .into_iter()
            .chain(APT_PACKAGES.iter().copied()),
        None,
    )?;

    println!("--- Building Netflix VMAF ---");
    run(
        "git",
        [
            "clone",
            "--depth",
            "1",
            "https://github.com/Netflix/vmaf.git",
            "vmaf",
        ],
        Some(workdir),
    )?;
    run(
        "meson",
        [
            "setup",
            "vmaf-build",
            "vmaf/libvmaf",
            "--prefix=/usr/local",
            "--buildtype=release",
            "-Denable_docs=false",
            "-Denable_tests=false",
        ],
        Some(workdir),
    )?;
    run("ninja", ["-C", "vmaf-build"], Some(workdir))?;
    run(
        "sudo",
        ["ninja", "-C", "vmaf-build", "install"],
        Some(workdir),
    )?;
    run("sudo", ["ldconfig"], None)?;
    prepend_media_paths()?;

    println!("--- Building SVT-AV1 ---");
    run(
        "git",
        [
            "clone",
            "--depth",
            "1",
            "https://gitlab.com/AOMediaCodec/SVT-AV1.git",
            "svt-av1",
        ],
        Some(workdir),
    )?;
    run(
        "cmake",
        [
            "-S",
            "svt-av1",
            "-B",
            "svt-av1-build",
            "-G",
            "Ninja",
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_INSTALL_PREFIX=/usr/local",
            "-DBUILD_SHARED_LIBS=ON",
        ],
        Some(workdir),
    )?;
    run(
        "cmake",
        ["--build", "svt-av1-build", "--parallel"],
        Some(workdir),
    )?;
    run(
        "sudo",
        ["cmake", "--install", "svt-av1-build"],
        Some(workdir),
    )?;
    run("sudo", ["ldconfig"], None)?;
    prepend_media_paths()?;
    run("pkg-config", ["--exists", "SvtAv1Enc >= 0.9.0"], None)?;

    println!("--- Building FFmpeg Development Snapshot ---");
    let ffmpeg_archive = workdir.join("ffmpeg-snapshot.tar.bz2");
    download(
        "https://ffmpeg.org/releases/ffmpeg-snapshot.tar.bz2",
        &ffmpeg_archive,
        workdir,
    )?;
    run(
        "tar",
        [OsStr::new("xjf"), ffmpeg_archive.as_os_str()],
        Some(workdir),
    )?;
    let ffmpeg_dir = workdir.join("ffmpeg");
    run(
        "./configure",
        [
            "--prefix=/usr/local",
            "--enable-gpl",
            "--enable-version3",
            "--enable-libx264",
            "--enable-libx265",
            "--enable-libaom",
            "--enable-libdav1d",
            "--enable-libopus",
            "--enable-libvpx",
            "--enable-libsvtav1",
            "--enable-libvmaf",
            "--disable-debug",
            "--disable-doc",
        ],
        Some(&ffmpeg_dir),
    )?;
    let jobs = format!("-j{}", std::thread::available_parallelism()?.get());
    run("make", [jobs.as_str()], Some(&ffmpeg_dir))?;
    run("sudo", ["make", "install"], Some(&ffmpeg_dir))?;
    run("sudo", ["ldconfig"], None)?;

    let filters = capture("ffmpeg", ["-hide_banner", "-filters"])?;
    require_output(&filters, "libvmaf", "ffmpeg -filters")?;
    let encoders = capture("ffmpeg", ["-hide_banner", "-encoders"])?;
    for encoder in [
        "libx264",
        "libx265",
        "libaom-av1",
        "libopus",
        "libvpx-vp9",
        "libsvtav1",
    ] {
        require_output(&encoders, encoder, "ffmpeg -encoders")?;
    }

    println!("--- Building libheif 1.23.1 ---");
    let libheif_archive = workdir.join("libheif-src.tar.gz");
    download(
        "https://github.com/strukturag/libheif/releases/download/v1.23.1/libheif-1.23.1.tar.gz",
        &libheif_archive,
        workdir,
    )?;
    run(
        "tar",
        [OsStr::new("xzf"), libheif_archive.as_os_str()],
        Some(workdir),
    )?;
    run(
        "cmake",
        [
            "-S",
            "libheif-1.23.1",
            "-B",
            "libheif-build",
            "-G",
            "Ninja",
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_INSTALL_PREFIX=/usr/local",
            "-DCMAKE_WARN_DEPRECATED=OFF",
            "-DWITH_EXAMPLES=OFF",
            "-DBUILD_TESTING=OFF",
            "-DWITH_OpenH264_DECODER=OFF",
        ],
        Some(workdir),
    )?;
    run(
        "cmake",
        ["--build", "libheif-build", "--parallel"],
        Some(workdir),
    )?;
    run(
        "sudo",
        ["cmake", "--install", "libheif-build"],
        Some(workdir),
    )?;
    run("sudo", ["ldconfig"], None)?;

    if env::var("MFB_SKIP_MPC_INSTALL").unwrap_or_default() == "1" {
        println!("Skipping GNU MPC installation until the CI downloader completes.");
    } else {
        install_gnu_mpc(workdir, &workspace)?;
    }

    println!("Media dependencies successfully installed.");
    Ok(())
}

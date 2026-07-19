use anyhow::{Context, Result, bail};
use chrono::Local;
use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
#[command(
    name = "create_live_photo",
    about = "Convert video to Apple Live Photo format"
)]
struct Args {
    /// Input video file path
    video: PathBuf,

    /// Output directory
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Image format
    #[arg(short = 'f', long = "format", value_enum, default_value = "jpg")]
    photo_format: PhotoFormat,

    /// Enable high-quality encoding mode
    #[arg(long)]
    hq: bool,

    /// Disable makelive metadata injection
    #[arg(long)]
    no_meta: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum PhotoFormat {
    Jpg,
    Heic,
}

fn main() -> Result<()> {
    let args = Args::parse();
    create_live_photo(&args)
}

fn create_live_photo(args: &Args) -> Result<()> {
    let video_arg = expand_tilde(&args.video);
    let video_path = video_arg
        .canonicalize()
        .with_context(|| format!("file not found: {}", args.video.display()))?;
    let output_dir = match &args.output {
        Some(dir) => {
            let dir = expand_tilde(dir);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("create output directory {}", dir.display()))?;
            dir.canonicalize()
                .with_context(|| format!("canonicalize output directory {}", dir.display()))?
        }
        None => video_path
            .parent()
            .context("input video has no parent directory")?
            .to_path_buf(),
    };
    let inject_metadata = !args.no_meta;
    check_dependencies(args.photo_format == PhotoFormat::Heic, inject_metadata)?;

    let (duration, resolution) = get_video_info(&video_path)?;
    let video_duration = duration.min(3.0);
    let stamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let base_name = format!("IMG_{stamp}");
    let img_ext = match args.photo_format {
        PhotoFormat::Jpg => "JPG",
        PhotoFormat::Heic => "HEIC",
    };
    let img_path = output_dir.join(format!("{base_name}.{img_ext}"));
    let mov_path = output_dir.join(format!("{base_name}.MOV"));

    println!("\n[1/3] Preparing to create Live Photo");
    println!(
        "  Input Video: {} ({resolution}, {duration:.2}s)",
        video_path
            .file_name()
            .unwrap_or(std::ffi::OsStr::new("<video>"))
            .to_string_lossy()
    );
    println!("  Output Format: {img_ext} + MOV");
    println!(
        "  Quality Mode: {}",
        if args.hq {
            "High Quality (HQ)"
        } else {
            "Standard"
        }
    );
    println!(
        "  Metadata Injection: {}",
        if inject_metadata {
            "Enabled"
        } else {
            "Disabled"
        }
    );

    println!("\n[2/3] Extracting cover image ({img_ext})...");
    match args.photo_format {
        PhotoFormat::Heic => {
            extract_heic_cover(&video_path, &output_dir, &base_name, &img_path, args.hq)?;
        }
        PhotoFormat::Jpg => extract_jpg_cover(&video_path, &img_path, args.hq)?,
    }
    println!("  Image generated: {}", file_name(&img_path));

    println!("\n[3/3] Converting video component...");
    convert_mov(&video_path, video_duration, &mov_path, args.hq)?;
    println!("  Video generated: {}", file_name(&mov_path));

    if inject_metadata {
        println!("\nInjecting metadata using makelive...");
        match run_quiet(
            Command::new(command_path("makelive")?)
                .args(["-p", "-v"])
                .arg(&img_path)
                .arg(&mov_path),
        ) {
            Ok(()) => println!("  Live Photo package created: {base_name}.pvt"),
            Err(err) => {
                println!("  ! Metadata injection failed (makelive): {err}");
                println!("  ! Keeping original image and video files.");
            }
        }
    }

    println!("\n==================================================");
    println!(
        "{} Live Photo creation process complete!",
        dev::infra::ui_tokens::pick_symbol("✨", "[*]")
    );
    println!("==================================================");
    println!("File Location: {}", output_dir.display());
    println!(" - {}", file_name(&img_path));
    println!(" - {}", file_name(&mov_path));
    if inject_metadata && output_dir.join(format!("{base_name}.pvt")).is_file() {
        println!(" - {base_name}.pvt (Ready for import)");
    }
    println!("==================================================\n");

    Ok(())
}

fn check_dependencies(needs_heif: bool, needs_makelive: bool) -> Result<()> {
    let mut deps = vec!["ffmpeg", "ffprobe"];
    if needs_heif {
        deps.push("heif-enc");
    }
    if needs_makelive {
        deps.push("makelive");
    }
    let missing = deps
        .into_iter()
        .filter(|dep| command_exists(dep).is_err())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        println!(
            "Error: Missing required dependencies: {}",
            missing.join(", ")
        );
        if missing.contains(&"heif-enc") {
            println!("Hint: Install heif-enc via 'brew install libheif'");
        }
        if missing.contains(&"makelive") {
            println!("Hint: Install makelive or related tools");
        }
        bail!("missing required dependencies: {}", missing.join(", "));
    }
    Ok(())
}

fn command_exists(program: &str) -> Result<()> {
    command_path(program).map(|_| ())
}

fn command_path(program: &str) -> Result<PathBuf> {
    foundation::common_utils::resolve_tool_path(program)
        .with_context(|| format!("{program} was not found or failed its runtime health check"))
}

fn get_video_info(video_path: &Path) -> Result<(f64, String)> {
    let ffprobe = command_path("ffprobe")?;
    let duration = command_output(
        Command::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            .arg(video_path),
    )?
    .trim()
    .parse::<f64>()
    .context("parse ffprobe duration")?;

    let resolution = command_output(
        Command::new(ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "csv=p=0",
            ])
            .arg(video_path),
    )?
    .trim()
    .to_owned();

    Ok((duration, resolution))
}

fn extract_heic_cover(
    video_path: &Path,
    output_dir: &Path,
    base_name: &str,
    img_path: &Path,
    hq: bool,
) -> Result<()> {
    let temp_png = output_dir.join(format!("{base_name}_temp.png"));
    run_quiet(
        Command::new(command_path("ffmpeg")?)
            .args(["-y", "-i"])
            .arg(video_path)
            .args(["-ss", "00:00:01", "-vframes", "1", "-pix_fmt", "rgb24"])
            .arg(&temp_png),
    )?;

    let mut cmd = Command::new(command_path("heif-enc")?);
    if hq {
        cmd.arg("--lossless");
    } else {
        cmd.args(["-q", "85"]);
    }
    run_quiet(cmd.args(["-o"]).arg(img_path).arg(&temp_png))?;
    let _ = std::fs::remove_file(temp_png);
    Ok(())
}

fn extract_jpg_cover(video_path: &Path, img_path: &Path, hq: bool) -> Result<()> {
    run_quiet(
        Command::new(command_path("ffmpeg")?)
            .args(["-y", "-i"])
            .arg(video_path)
            .args(["-ss", "00:00:01", "-vframes", "1", "-q:v"])
            .arg(if hq { "1" } else { "2" })
            .arg(img_path),
    )
}

fn convert_mov(video_path: &Path, duration: f64, mov_path: &Path, hq: bool) -> Result<()> {
    let duration_s = if duration.fract().abs() < f64::EPSILON {
        format!("{duration:.1}")
    } else {
        duration.to_string()
    };
    let mut cmd = Command::new(command_path("ffmpeg")?);
    cmd.args(["-y", "-i"]).arg(video_path).args([
        "-t",
        &duration_s,
        "-c:v",
        "h264",
        "-c:a",
        "aac",
        "-pix_fmt",
        "yuv420p",
        "-movflags",
        "+faststart",
    ]);
    if hq {
        cmd.args(["-crf", "18", "-preset", "slow"]);
    } else {
        cmd.args(["-q:v", "2"]);
    }
    run_quiet(cmd.arg(mov_path))
}

fn command_output(cmd: &mut Command) -> Result<String> {
    let output = cmd
        .output()
        .with_context(|| format!("run {:?}", cmd.get_program()))?;
    if !output.status.success() {
        bail!("command failed with status {}", output.status);
    }
    String::from_utf8(output.stdout).context("command output utf8")
}

fn run_quiet(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("run {:?}", cmd.get_program()))?;
    if !status.success() {
        bail!("command failed with status {status}");
    }
    Ok(())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(std::ffi::OsStr::new("<file>"))
        .to_string_lossy()
        .into_owned()
}

fn expand_tilde(path: &Path) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path.to_path_buf();
    };
    if raw == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    path.to_path_buf()
}

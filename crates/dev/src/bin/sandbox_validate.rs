//! Modern Format Boost - Ephemeral Validation Sandbox in Rust.
//! Never writes under the repo tree or user bundles. Creates temp fixtures,
//! runs release vid/img binaries, greps logs for contract signals, and runs
//! verify.

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Parser, Debug)]
#[command(name = "sandbox_validate", about = "MFB Ephemeral Sandbox Validator")]
struct Args {
    #[arg(long = "keep", help = "Print sandbox path and do not delete it.")]
    keep: bool,
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

fn get_project_root() -> Result<PathBuf> {
    let exe_path = std::env::current_exe().context("get current exe path")?;
    let mut dir = exe_path.parent();
    while let Some(d) = dir {
        if d.join("Cargo.toml").is_file() && d.join("crates").is_dir() {
            return Ok(d.to_path_buf());
        }
        dir = d.parent();
    }
    let cwd = std::env::current_dir().context("get current dir")?;
    Ok(cwd)
}

fn path_arg(path: &Path) -> OsString {
    path.as_os_str().to_os_string()
}

fn string_arg(value: &str) -> OsString {
    OsString::from(value)
}

/// Keep the synthetic source lossless so the sandbox measures HEVC conversion,
/// not whether HEVC can beat an already compressed default-x264 fixture.
fn lossless_h264_fixture_args(output: &Path) -> Vec<OsString> {
    vec![
        string_arg("-hide_banner"),
        string_arg("-loglevel"),
        string_arg("error"),
        string_arg("-y"),
        string_arg("-f"),
        string_arg("lavfi"),
        string_arg("-i"),
        string_arg("testsrc=duration=8:size=640x360:rate=30"),
        string_arg("-c:v"),
        string_arg("libx264"),
        string_arg("-preset"),
        string_arg("ultrafast"),
        string_arg("-crf"),
        string_arg("0"),
        string_arg("-pix_fmt"),
        string_arg("yuv420p"),
        path_arg(output),
    ]
}

fn render_command(program: &OsStr, args: &[OsString]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(program.to_string_lossy().into_owned());
    parts.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
    parts.join(" ")
}

fn verify_command(_repo_root: &Path, verify_bin: &Path) -> (OsString, Vec<OsString>) {
    if verify_bin.is_file() {
        return (verify_bin.as_os_str().to_os_string(), Vec::new());
    }
    (
        OsString::from("cargo"),
        vec![
            string_arg("run"),
            string_arg("--locked"),
            string_arg("-p"),
            string_arg("dev"),
            string_arg("--bin"),
            string_arg("verify"),
            string_arg("--"),
        ],
    )
}

fn run_cmd(program: &OsStr, args: &[OsString], log_path: Option<&Path>) -> Result<Output> {
    let rendered = render_command(program, args);
    println!("+ {rendered}");
    let mut command = Command::new(program);
    command.args(args);
    let output = command.output().context("run command")?;
    if let Some(log) = log_path {
        let mut file = fs::File::create(log).context("create log file")?;
        use std::io::Write;
        file.write_all(&output.stdout).context("write stdout")?;
        file.write_all(&output.stderr).context("write stderr")?;
    }
    if !output.status.success() {
        let err_text = String::from_utf8_lossy(&output.stderr);
        let out_text = String::from_utf8_lossy(&output.stdout);
        eprintln!("Command failed. Stderr:\n{err_text}\nStdout:\n{out_text}");
        bail!("command failed with status {}: {rendered}", output.status);
    }
    Ok(output)
}

struct GrepChecks {
    gate_3d_passed: bool,
    no_ssim_enforce_reject: bool,
    ultimate_mode_logged: bool,
    ignore_class_static: bool,
    ignore_class_img_anim: bool,
    confidence_skip: bool,
}

fn grep_checks(log_text: &str) -> GrepChecks {
    let low = log_text.to_lowercase();
    GrepChecks {
        gate_3d_passed: low.contains("3d quality gate: passed"),
        no_ssim_enforce_reject: !low.contains("enforce_ssim_presence")
            && !low.contains("ssim below target"),
        ultimate_mode_logged: low.contains("ultimate mode") || low.contains("3d quality gate"),
        ignore_class_static: low.contains("ignore_class=vid_static_single_frame")
            || low.contains("vid ignores static media"),
        ignore_class_img_anim: low.contains("ignore_class=img_animated_handoff"),
        confidence_skip: low.contains("exploration confidence missing"),
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    for tool in &["ffmpeg", "ffprobe"] {
        if !command_exists(tool) {
            eprintln!("Missing required tool: {tool}");
            std::process::exit(1);
        }
    }

    let repo_root = get_project_root()?;
    let vid_bin = repo_root.join("target/release/vid");
    let img_bin = repo_root.join("target/release/img");
    let verify_bin = repo_root.join("target/release/verify");

    if !vid_bin.is_file() {
        eprintln!(
            "Build vid first: cargo build -p vid --release ({})",
            vid_bin.display()
        );
        std::process::exit(1);
    }
    if !img_bin.is_file() {
        eprintln!(
            "Build img first: cargo build -p img --release ({})",
            img_bin.display()
        );
        std::process::exit(1);
    }

    let sandbox = tempfile::Builder::new()
        .prefix("mfb_sandbox_")
        .tempdir()
        .context("create temp sandbox dir")?;
    let sandbox_path = sandbox.path().to_path_buf();
    let src = sandbox_path.join("src");
    let opt = sandbox_path.join("opt");
    let logs = sandbox_path.join("logs");

    fs::create_dir_all(&src)?;
    fs::create_dir_all(&opt)?;
    fs::create_dir_all(&logs)?;

    println!("Sandbox: {}", sandbox_path.display());

    // Make fixtures
    let video_fixture = src.join("video9.mp4");
    run_cmd(
        OsStr::new("ffmpeg"),
        &lossless_h264_fixture_args(&video_fixture),
        None,
    )?;

    run_cmd(
        OsStr::new("ffmpeg"),
        &[
            string_arg("-hide_banner"),
            string_arg("-loglevel"),
            string_arg("error"),
            string_arg("-y"),
            string_arg("-f"),
            string_arg("lavfi"),
            string_arg("-i"),
            string_arg("color=red:s=64x64:d=1"),
            string_arg("-frames:v"),
            string_arg("1"),
            path_arg(&src.join("static.webp")),
        ],
        None,
    )?;

    // Anim webp
    let anim_path = src.join("anim.webp");
    run_cmd(
        OsStr::new("ffmpeg"),
        &[
            string_arg("-hide_banner"),
            string_arg("-loglevel"),
            string_arg("error"),
            string_arg("-y"),
            string_arg("-f"),
            string_arg("lavfi"),
            string_arg("-i"),
            string_arg("testsrc=duration=1:size=64x64:rate=10"),
            path_arg(&anim_path),
        ],
        None,
    )?;

    // Run tests
    let vlog = logs.join("video9.txt");
    run_cmd(
        vid_bin.as_os_str(),
        &[
            string_arg("run"),
            string_arg("--codec"),
            string_arg("hevc"),
            string_arg("--force"),
            string_arg("--ultimate"),
            string_arg("--explore"),
            string_arg("--compress"),
            string_arg("--match-quality"),
            string_arg("--plain"),
            string_arg("--no-resume"),
            string_arg("-o"),
            path_arg(&opt),
            path_arg(&video_fixture),
        ],
        Some(&vlog),
    )?;
    let v_text = fs::read_to_string(&vlog)?;
    let vchecks = grep_checks(&v_text);

    let slog = logs.join("static_webp.txt");
    run_cmd(
        vid_bin.as_os_str(),
        &[
            string_arg("run"),
            string_arg("--plain"),
            string_arg("--no-resume"),
            string_arg("-o"),
            path_arg(&opt),
            path_arg(&src.join("static.webp")),
        ],
        Some(&slog),
    )?;
    let s_text = fs::read_to_string(&slog)?;
    let schecks = grep_checks(&s_text);

    let alog = logs.join("anim_webp.txt");
    run_cmd(
        vid_bin.as_os_str(),
        &[
            string_arg("run"),
            string_arg("--plain"),
            string_arg("--no-resume"),
            string_arg("-o"),
            path_arg(&opt),
            path_arg(&src.join("anim.webp")),
        ],
        Some(&alog),
    )?;
    let anim_text = fs::read_to_string(&alog)?;
    let anim_not_static_ignore = !anim_text.contains("ignore_class=vid_static_unknown_frames")
        && !anim_text.contains("ignore_class=vid_static_single_frame");

    let ilog = logs.join("anim_img.txt");
    run_cmd(
        img_bin.as_os_str(),
        &[
            string_arg("run"),
            string_arg("--plain"),
            string_arg("--no-resume"),
            string_arg("-o"),
            path_arg(&opt),
            path_arg(&src.join("anim.webp")),
        ],
        Some(&ilog),
    )?;
    let i_text = fs::read_to_string(&ilog)?;
    let ichecks = grep_checks(&i_text);

    // verify integrity
    let verify_out = logs.join("verify_report.txt");
    let mut verify_args = vec![
        string_arg("--verify"),
        path_arg(&src),
        path_arg(&opt),
        string_arg("--mode"),
        string_arg("both"),
    ];
    let has_any_logs = fs::read_dir(&logs)?.next().is_some();
    if has_any_logs {
        verify_args.push(string_arg("--session-audit"));
        verify_args.push(path_arg(&logs));
    }
    let (verify_program, mut verify_prefix_args) = verify_command(&repo_root, &verify_bin);
    verify_prefix_args.extend(verify_args);
    let verify_output = run_cmd(
        verify_program.as_os_str(),
        &verify_prefix_args,
        Some(&verify_out),
    )?;

    println!("\n── Sandbox checks ──");
    println!("  video9 3D gate passed:     {}", vchecks.gate_3d_passed);
    println!(
        "  video9 ultimate/3D signal: {}",
        vchecks.ultimate_mode_logged
    );
    println!(
        "  video9 no SSIM reject:     {}",
        vchecks.no_ssim_enforce_reject
    );
    println!(
        "  video9 confidence skip:    {} (want False after backfill)",
        vchecks.confidence_skip
    );
    println!(
        "  static ignore_class/heur:  {}",
        schecks.ignore_class_static
    );
    println!(
        "  anim log has 0x0 probe:    {}",
        anim_text.contains("0x0") || anim_text.contains("image data not found")
    );
    println!("  anim not static-ignore:    {anim_not_static_ignore}");
    println!(
        "  img anim handoff class:    {}",
        ichecks.ignore_class_img_anim
    );
    println!(
        "  verify exit:               {}",
        verify_output.status.code().unwrap_or(-1)
    );

    let ok = vchecks.gate_3d_passed
        && vchecks.ultimate_mode_logged
        && vchecks.no_ssim_enforce_reject
        && !vchecks.confidence_skip
        && schecks.ignore_class_static
        && anim_not_static_ignore
        && ichecks.ignore_class_img_anim;

    if !ok {
        eprintln!("\nSANDBOX FAILED — see logs under {logs:?}");
        std::process::exit(1);
    }
    println!("\nSANDBOX OK");

    if args.keep {
        println!("Kept: {}", sandbox_path.display());
        let _kept_path = sandbox.keep();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grep_checks() {
        let sample_log = "\
        [info] ultimate mode initialized\n[info] 3D Quality Gate: passed\n[info] \
                          ignore_class=vid_static_single_frame\n[info] \
                          ignore_class=img_animated_handoff\n";
        let checks = grep_checks(sample_log);
        assert!(checks.gate_3d_passed);
        assert!(checks.no_ssim_enforce_reject);
        assert!(checks.ultimate_mode_logged);
        assert!(checks.ignore_class_static);
        assert!(checks.ignore_class_img_anim);
        assert!(!checks.confidence_skip);
    }

    #[test]
    fn test_verify_command_falls_back_to_cargo_run() {
        let root = Path::new("/workspace");
        let verify_bin = root.join("target/release/verify");
        let command = verify_command(root, &verify_bin);
        assert_eq!(command.0, OsString::from("cargo"));
        assert_eq!(
            command.1,
            vec![
                OsString::from("run"),
                OsString::from("--locked"),
                OsString::from("-p"),
                OsString::from("dev"),
                OsString::from("--bin"),
                OsString::from("verify"),
                OsString::from("--"),
            ]
        );
    }

    #[test]
    fn h264_fixture_is_explicitly_lossless() {
        let args = lossless_h264_fixture_args(Path::new("/tmp/video9.mp4"));

        assert!(args.windows(2).any(|pair| {
            pair[0].as_os_str() == OsStr::new("-preset")
                && pair[1].as_os_str() == OsStr::new("ultrafast")
        }));
        assert!(args.windows(2).any(|pair| {
            pair[0].as_os_str() == OsStr::new("-crf") && pair[1].as_os_str() == OsStr::new("0")
        }));
    }

    #[test]
    fn test_run_cmd_propagates_nonzero_status() {
        let error = run_cmd(
            OsStr::new("sh"),
            &[OsString::from("-c"), OsString::from("exit 7")],
            None,
        )
        .expect_err("sandbox validation commands must not ignore failure");

        assert!(error.to_string().contains("status"));
    }
}

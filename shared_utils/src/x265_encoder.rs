//! x265 Direct CPU Encoder Module
//!
//! 🔥 v6.9.17: CPU编码架构 - 使用x265命令行工具直接编码
//!
//! ## 架构设计
//!
//! 由于系统FFmpeg缺少libx265支持，采用三步编码流程：
//! 1. FFmpeg解码 → Y4M (raw YUV)
//! 2. x265编码 → HEVC bitstream
//! 3. FFmpeg封装 → MP4容器
//!
//! ## 优势
//! - 不依赖FFmpeg编译选项
//! - 完整的CRF控制（sub-integer精度）
//! - 更高的SSIM质量（≥0.98 vs VideoToolbox ~0.95）
//! - 严格的CPU编码路径（无GPU fallback）

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct X265Config {
    pub crf: f32,
    pub preset: String,
    pub threads: usize,
    pub container: String,
    pub preserve_audio: bool,
}

impl Default for X265Config {
    fn default() -> Self {
        Self {
            crf: 23.0,
            preset: "medium".to_string(),
            threads: crate::thread_manager::get_optimal_threads(),
            container: "mp4".to_string(),
            preserve_audio: true,
        }
    }
}

pub fn encode_with_x265(
    input: &Path,
    output: &Path,
    config: &X265Config,
    vf_args: &[String],
) -> Result<u64> {
    info!(
        input = ?input,
        output = ?output,
        crf = config.crf,
        preset = %config.preset,
        "🖥️  Starting CPU encoding with x265 CLI"
    );

    use crate::universal_heartbeat::{HeartbeatConfig, HeartbeatGuard};
    let _heartbeat = HeartbeatGuard::new(
        HeartbeatConfig::medium("x265 CLI Encoding").with_info(format!("CRF {:.1}", config.crf)),
    );

    let hevc_temp = tempfile::Builder::new()
        .suffix(".hevc")
        .tempfile()
        .context("Failed to create temporary HEVC file")?;
    let hevc_file = hevc_temp.path().to_path_buf();

    debug!(hevc_temp_file = ?hevc_file, "Using temporary HEVC file");

    info!("Step 1/2: Decode + x265 encode...");
    let encode_result = encode_to_hevc(input, &hevc_file, config, vf_args)?;

    if !encode_result {
        error!("x265 encoding failed");
        bail!("x265 encoding failed");
    }

    info!("Step 2/2: Mux HEVC + audio...");
    mux_hevc_to_container(input, &hevc_file, output, config)?;

    drop(hevc_temp);

    let output_size = std::fs::metadata(output)
        .context("Failed to get output file size")?
        .len();

    info!(
        output_size = output_size,
        output_path = ?output,
        "✅ x265 CPU encoding complete"
    );

    Ok(output_size)
}

fn encode_to_hevc(
    input: &Path,
    hevc_output: &Path,
    config: &X265Config,
    vf_args: &[String],
) -> Result<bool> {
    let start_time = std::time::Instant::now();

    let mut ffmpeg_cmd = Command::new("ffmpeg");
    ffmpeg_cmd
        .arg("-y")
        .arg("-i")
        .arg(crate::safe_path_arg(input).as_ref())
        .arg("-f")
        .arg("yuv4mpegpipe");

    for arg in vf_args {
        ffmpeg_cmd.arg(arg);
    }

    ffmpeg_cmd
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let ffmpeg_cmd_str = format!(
        "ffmpeg -y -i {} -f yuv4mpegpipe {} -pix_fmt yuv420p -",
        crate::safe_path_arg(input),
        vf_args.join(" ")
    );
    info!(command = %ffmpeg_cmd_str, "Executing FFmpeg decode command");

    let mut x265_cmd = Command::new("x265");
    x265_cmd
        .arg("--y4m")
        .arg("--input")
        .arg("-")
        .arg("--output")
        .arg(crate::safe_path_arg(hevc_output).as_ref())
        .arg("--crf")
        .arg(format!("{:.1}", config.crf))
        .arg("--preset")
        .arg(&config.preset)
        .arg("--pools")
        .arg(config.threads.to_string())
        .arg("--log-level")
        .arg("error")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let x265_cmd_str = format!(
        "x265 --y4m --input - --output {:?} --crf {:.1} --preset {} --pools {} --log-level error",
        hevc_output, config.crf, config.preset, config.threads
    );
    info!(command = %x265_cmd_str, "Executing x265 encode command");

    let mut ffmpeg_child = ffmpeg_cmd
        .spawn()
        .context("Failed to spawn ffmpeg decode process")?;

    let mut x265_child = x265_cmd
        .spawn()
        .context("Failed to spawn x265 encode process")?;

    let ffmpeg_stderr_thread = ffmpeg_child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stderr);
            let mut output = String::new();
            for line in reader.lines().map_while(Result::ok) {
                output.push_str(&line);
                output.push('\n');
            }
            output
        })
    });

    let x265_stderr_thread = x265_child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let reader = BufReader::new(stderr);
            let mut output = String::new();
            for line in reader.lines().map_while(Result::ok) {
                output.push_str(&line);
                output.push('\n');
            }
            output
        })
    });

    if let (Some(mut ffmpeg_out), Some(mut x265_in)) =
        (ffmpeg_child.stdout.take(), x265_child.stdin.take())
    {
        let transfer_thread =
            std::thread::spawn(move || std::io::copy(&mut ffmpeg_out, &mut x265_in));

        // Join pipe-copy thread first so we see BrokenPipe before process exit codes.
        let copy_result: Result<Result<u64, std::io::Error>, _> = transfer_thread.join();
        let pipe_io_error = copy_result.as_ref().ok().and_then(|r| r.as_ref().err());
        let is_broken_pipe = pipe_io_error.map_or(false, |e| {
            use std::io::ErrorKind;
            matches!(e.kind(), ErrorKind::BrokenPipe | ErrorKind::ConnectionReset)
        });

        let x265_status = x265_child.wait().context("Failed to wait for x265")?;
        let ffmpeg_status = ffmpeg_child.wait().context("Failed to wait for ffmpeg")?;

        let duration = start_time.elapsed();

        let ffmpeg_stderr = ffmpeg_stderr_thread
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let x265_stderr = x265_stderr_thread
            .and_then(|h| h.join().ok())
            .unwrap_or_default();

        if !ffmpeg_status.success() {
            error!(
                command = %ffmpeg_cmd_str,
                exit_code = ?ffmpeg_status.code(),
                duration_secs = duration.as_secs_f64(),
                stderr = %ffmpeg_stderr,
                pipe_broken = is_broken_pipe,
                "FFmpeg decode failed"
            );
            if is_broken_pipe {
                warn!("Pipe broken: decoder (ffmpeg) likely exited first; check FFmpeg stderr above");
            }
            if !ffmpeg_stderr.is_empty() {
                eprintln!("FFmpeg error output:\n{}", ffmpeg_stderr);
            }
            bail!("FFmpeg decode failed");
        }

        if !x265_status.success() {
            error!(
                command = %x265_cmd_str,
                exit_code = ?x265_status.code(),
                duration_secs = duration.as_secs_f64(),
                stderr = %x265_stderr,
                pipe_broken = is_broken_pipe,
                "x265 encode failed"
            );
            if is_broken_pipe {
                warn!("Pipe broken: encoder (x265) likely exited first; check x265 stderr above");
            }
            if !x265_stderr.is_empty() {
                eprintln!("x265 error output:\n{}", x265_stderr);
            }
            bail!("x265 encode failed with exit code {:?}", x265_status.code());
        }

        if let Ok(Err(io_err)) = &copy_result {
            error!(
                io_error = %io_err,
                kind = ?io_err.kind(),
                "Pipe copy failed (decoder and encoder both reported success)"
            );
            if is_broken_pipe {
                bail!("Pipe broken during copy (ffmpeg→x265): {}", io_err);
            }
            bail!("Pipe I/O error: {}", io_err);
        }
        if let Err(join_err) = copy_result {
            error!("Pipe copy thread panicked: {:?}", join_err);
            bail!("Pipe copy thread panicked: {:?}", join_err);
        }

        info!(
            duration_secs = duration.as_secs_f64(),
            output_file = ?hevc_output,
            "x265 encoding completed successfully"
        );

        Ok(true)
    } else {
        error!("Failed to connect ffmpeg and x265 pipes");
        bail!("Failed to connect ffmpeg and x265 pipes");
    }
}

fn mux_hevc_to_container(
    original_input: &Path,
    hevc_file: &Path,
    output: &Path,
    config: &X265Config,
) -> Result<()> {
    let start_time = std::time::Instant::now();

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-i")
        .arg(crate::safe_path_arg(hevc_file).as_ref());

    if config.preserve_audio {
        cmd.arg("-i")
            .arg(crate::safe_path_arg(original_input).as_ref());
        cmd.arg("-map")
            .arg("0:v:0")
            .arg("-map")
            .arg("1:a:0?")
            .arg("-c:v")
            .arg("copy")
            .arg("-c:a")
            .arg("aac")
            .arg("-b:a")
            .arg("256k");
    } else {
        cmd.arg("-c:v").arg("copy").arg("-an");
    }

    if config.container == "mp4" || config.container == "mov" {
        cmd.arg("-tag:v").arg("hvc1");
        cmd.arg("-movflags").arg("+faststart");
    }

    cmd.arg(crate::safe_path_arg(output).as_ref())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let cmd_str = format!(
        "ffmpeg -y -i {:?} {} -c:v copy {} {:?}",
        hevc_file,
        if config.preserve_audio {
            format!(
                "-i {:?} -map 0:v:0 -map 1:a:0? -c:a aac -b:a 256k",
                original_input
            )
        } else {
            "-an".to_string()
        },
        if config.container == "mp4" || config.container == "mov" {
            "-tag:v hvc1 -movflags +faststart"
        } else {
            ""
        },
        output
    );
    info!(command = %cmd_str, "Executing FFmpeg mux command");

    let output_result = cmd.output().context("Failed to execute ffmpeg mux")?;

    let duration = start_time.elapsed();

    if !output_result.status.success() {
        let stderr = String::from_utf8_lossy(&output_result.stderr);
        error!(
            command = %cmd_str,
            exit_code = ?output_result.status.code(),
            duration_secs = duration.as_secs_f64(),
            stderr = %stderr,
            "FFmpeg mux failed"
        );
        bail!("FFmpeg mux failed: {}", stderr);
    }

    info!(
        duration_secs = duration.as_secs_f64(),
        output_file = ?output,
        "FFmpeg mux completed successfully"
    );

    Ok(())
}

pub fn is_x265_available() -> bool {
    let result = Command::new("x265")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if result {
        debug!("x265 tool is available");
    } else {
        warn!("x265 tool is not available - install with: brew install x265");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_x265_available() {
        if is_x265_available() {
            println!("✅ x265 is available");
        } else {
            println!("⚠️  x265 not found - install with: brew install x265");
        }
    }
}

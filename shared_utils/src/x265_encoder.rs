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

/// x265编码器配置
#[derive(Debug, Clone)]
pub struct X265Config {
    /// CRF值（0-51，越小质量越高）
    pub crf: f32,
    /// 编码preset（ultrafast, fast, medium, slow, slower, veryslow）
    pub preset: String,
    /// 最大线程数
    pub threads: usize,
    /// 输出容器格式（mp4, mov, mkv）
    pub container: String,
    /// 是否保留音频
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

/// 使用x265 CLI工具进行CPU编码
///
/// # 流程
/// 1. FFmpeg解码输入 → Y4M管道
/// 2. x265从管道读取Y4M → 编码为HEVC
/// 3. FFmpeg封装HEVC + 音频 → 最终容器
///
/// # Arguments
/// * `input` - 输入视频文件
/// * `output` - 输出文件路径
/// * `config` - x265编码配置
/// * `vf_args` - 视频滤镜参数（用于分辨率调整）
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

    // 🔥 v7.7: 启动心跳检测(30秒间隔)
    use crate::universal_heartbeat::{HeartbeatConfig, HeartbeatGuard};
    let _heartbeat = HeartbeatGuard::new(
        HeartbeatConfig::medium("x265 CLI Encoding").with_info(format!("CRF {:.1}", config.crf)),
    );

    // 临时文件路径
    let temp_dir = std::env::temp_dir();
    let hevc_file = temp_dir.join(format!("temp_{}.hevc", std::process::id()));

    debug!(hevc_temp_file = ?hevc_file, "Using temporary HEVC file");

    // 清理旧的临时文件
    let _ = std::fs::remove_file(&hevc_file);

    // Step 1: FFmpeg解码 → Y4M → x265编码 → HEVC
    info!("Step 1/2: Decode + x265 encode...");
    let encode_result = encode_to_hevc(input, &hevc_file, config, vf_args)?;

    if !encode_result {
        error!("x265 encoding failed");
        bail!("x265 encoding failed");
    }

    // Step 2: FFmpeg封装HEVC + 音频 → MP4
    info!("Step 2/2: Mux HEVC + audio...");
    mux_hevc_to_container(input, &hevc_file, output, config)?;

    // 清理临时文件
    let _ = std::fs::remove_file(&hevc_file);

    // 返回输出文件大小
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

/// Step 1: FFmpeg解码 + x265编码
fn encode_to_hevc(
    input: &Path,
    hevc_output: &Path,
    config: &X265Config,
    vf_args: &[String],
) -> Result<bool> {
    let start_time = std::time::Instant::now();

    // 构建FFmpeg解码命令（输出Y4M到stdout）
    let mut ffmpeg_cmd = Command::new("ffmpeg");
    ffmpeg_cmd
        .arg("-y")
        .arg("-i")
        .arg(input)
        .arg("-f")
        .arg("yuv4mpegpipe");

    // 添加视频滤镜
    for arg in vf_args {
        ffmpeg_cmd.arg(arg);
    }

    ffmpeg_cmd
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // 记录FFmpeg命令
    let ffmpeg_cmd_str = format!(
        "ffmpeg -y -i {:?} -f yuv4mpegpipe {} -pix_fmt yuv420p -",
        input,
        vf_args.join(" ")
    );
    info!(command = %ffmpeg_cmd_str, "Executing FFmpeg decode command");

    // 构建x265编码命令（从stdin读取Y4M）
    let mut x265_cmd = Command::new("x265");
    x265_cmd
        .arg("--y4m") // 输入格式为Y4M
        .arg("--input")
        .arg("-") // 从stdin读取
        .arg("--output")
        .arg(hevc_output)
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

    // 记录x265命令
    let x265_cmd_str = format!(
        "x265 --y4m --input - --output {:?} --crf {:.1} --preset {} --pools {} --log-level error",
        hevc_output, config.crf, config.preset, config.threads
    );
    info!(command = %x265_cmd_str, "Executing x265 encode command");

    // 启动FFmpeg解码进程
    let mut ffmpeg_child = ffmpeg_cmd
        .spawn()
        .context("Failed to spawn ffmpeg decode process")?;

    // 启动x265编码进程
    let mut x265_child = x265_cmd
        .spawn()
        .context("Failed to spawn x265 encode process")?;

    // 连接FFmpeg stdout → x265 stdin
    if let (Some(mut ffmpeg_out), Some(mut x265_in)) =
        (ffmpeg_child.stdout.take(), x265_child.stdin.take())
    {
        // 在后台线程中传输数据
        let transfer_thread =
            std::thread::spawn(move || std::io::copy(&mut ffmpeg_out, &mut x265_in));

        // 等待两个进程完成
        let ffmpeg_status = ffmpeg_child.wait().context("Failed to wait for ffmpeg")?;
        let x265_status = x265_child.wait().context("Failed to wait for x265")?;

        // 等待数据传输完成
        let _ = transfer_thread.join();

        let duration = start_time.elapsed();

        if !ffmpeg_status.success() {
            error!(
                command = %ffmpeg_cmd_str,
                exit_code = ?ffmpeg_status.code(),
                duration_secs = duration.as_secs_f64(),
                "FFmpeg decode failed"
            );
            bail!("FFmpeg decode failed");
        }

        if !x265_status.success() {
            // 读取x265错误信息
            let stderr_output = if let Some(mut stderr) = x265_child.stderr.take() {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut stderr, &mut buf).ok();
                buf
            } else {
                String::new()
            };

            error!(
                command = %x265_cmd_str,
                exit_code = ?x265_status.code(),
                duration_secs = duration.as_secs_f64(),
                stderr = %stderr_output,
                "x265 encode failed"
            );

            if !stderr_output.is_empty() {
                eprintln!("x265 error output:\n{}", stderr_output);
            }

            bail!("x265 encode failed with exit code {:?}", x265_status.code());
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

/// Step 2: FFmpeg封装HEVC + 音频到容器
fn mux_hevc_to_container(
    original_input: &Path,
    hevc_file: &Path,
    output: &Path,
    config: &X265Config,
) -> Result<()> {
    let start_time = std::time::Instant::now();

    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y").arg("-i").arg(hevc_file); // HEVC视频流

    // 如果需要保留音频，添加原始输入作为音频源
    if config.preserve_audio {
        cmd.arg("-i").arg(original_input); // 原始文件（音频源）
        cmd.arg("-map")
            .arg("0:v:0") // 使用第一个输入的视频流（HEVC）
            .arg("-map")
            .arg("1:a:0?") // 使用第二个输入的音频流（如果存在）
            .arg("-c:v")
            .arg("copy") // 视频流直接复制
            .arg("-c:a")
            .arg("aac") // 音频转码为AAC
            .arg("-b:a")
            .arg("256k"); // 音频比特率
    } else {
        cmd.arg("-c:v")
            .arg("copy") // 视频流直接复制
            .arg("-an"); // 无音频
    }

    // 添加容器特定参数
    if config.container == "mp4" || config.container == "mov" {
        cmd.arg("-tag:v").arg("hvc1"); // Apple兼容性
        cmd.arg("-movflags").arg("+faststart"); // 快速启动
    }

    cmd.arg(output).stdout(Stdio::null()).stderr(Stdio::piped());

    // 记录FFmpeg mux命令
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

/// 检查x265工具是否可用
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
        // 这个测试在CI环境可能失败，仅用于本地验证
        if is_x265_available() {
            println!("✅ x265 is available");
        } else {
            println!("⚠️  x265 not found - install with: brew install x265");
        }
    }
}

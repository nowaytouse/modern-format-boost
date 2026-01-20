//! 🔥 v6.4.7: FFmpeg 进程管理模块 - 防止管道死锁
//!
//! ## 问题背景
//!
//! 当同时 pipe stdout 和 stderr 但只读取 stdout 时，如果 FFmpeg 输出大量
//! stderr 日志（超过 64KB 缓冲区），会导致死锁：
//! - FFmpeg 因 stderr 缓冲区满而阻塞
//! - Rust 程序因等待 stdout 而阻塞
//! - 两者互相等待，程序卡死
//!
//! ## 解决方案
//!
//! 使用独立线程并发消耗 stderr，确保缓冲区不会满。
//!
//! ## 使用示例
//!
//! ```ignore
//! use shared_utils::ffmpeg_process::FfmpegProcess;
//! use std::process::Command;
//!
//! let mut cmd = Command::new("ffmpeg");
//! cmd.arg("-i").arg("input.mp4").arg("output.mp4");
//!
//! let mut process = FfmpegProcess::spawn(&mut cmd)?;
//!
//! // 读取 stdout 进度
//! if let Some(stdout) = process.stdout() {
//!     // 处理进度...
//! }
//!
//! // 等待完成
//! let (status, stderr) = process.wait_with_output()?;
//! ```

use anyhow::{Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use tracing::{debug, error, info};

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.7: FfmpegProcess - 防死锁的 FFmpeg 进程包装器
// ═══════════════════════════════════════════════════════════════

/// FFmpeg 进程包装器 - 自动处理 stderr 消耗，防止管道死锁
///
/// # 设计原理
///
/// 操作系统管道缓冲区通常只有 64KB。如果 FFmpeg 输出大量 stderr
/// 而程序只读取 stdout，stderr 缓冲区会满，导致 FFmpeg 阻塞，
/// 进而导致 stdout 也停止输出，形成死锁。
///
/// 本结构体通过独立线程持续消耗 stderr 来解决这个问题。
pub struct FfmpegProcess {
    child: Child,
    stderr_thread: Option<JoinHandle<String>>,
}

impl FfmpegProcess {
    /// 启动 FFmpeg 进程（自动处理 stderr 消耗）
    ///
    /// # Arguments
    /// * `cmd` - 已配置好参数的 Command（会自动设置 stdout/stderr 为 piped）
    ///
    /// # Returns
    /// 包装后的 FfmpegProcess，可安全读取 stdout 而不会死锁
    ///
    /// # Errors
    /// - 进程启动失败
    /// - 无法捕获 stderr
    pub fn spawn(cmd: &mut Command) -> Result<Self> {
        // 记录即将执行的FFmpeg命令
        let command_str = format!("{:?}", cmd);
        info!(
            command = %command_str,
            "Executing FFmpeg command"
        );

        // 设置管道
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().context("Failed to spawn FFmpeg process")?;

        // 🔥 关键：独立线程消耗 stderr，防止缓冲区满死锁
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to capture FFmpeg stderr"))?;

        let stderr_thread = thread::spawn(move || {
            let mut buf = String::new();
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    buf.push_str(&line);
                    buf.push('\n');
                }
            }
            buf
        });

        Ok(Self {
            child,
            stderr_thread: Some(stderr_thread),
        })
    }

    /// 获取 stdout 用于读取进度
    ///
    /// # Returns
    /// stdout 的可变引用，如果已被 take 则返回 None
    pub fn stdout(&mut self) -> Option<&mut ChildStdout> {
        self.child.stdout.as_mut()
    }

    /// Take stdout（转移所有权）
    ///
    /// # Returns
    /// stdout，如果已被 take 则返回 None
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    /// 等待进程完成并获取输出
    ///
    /// # Returns
    /// (ExitStatus, stderr_content) - 退出状态和 stderr 内容
    ///
    /// # Errors
    /// - 等待进程失败
    pub fn wait_with_output(mut self) -> Result<(ExitStatus, String)> {
        let status = self.child.wait().context("Failed to wait for FFmpeg")?;
        let stderr = self
            .stderr_thread
            .take()
            .map(|t| t.join().unwrap_or_default())
            .unwrap_or_default();

        // 记录FFmpeg执行结果
        if status.success() {
            info!(
                exit_code = status.code(),
                "FFmpeg process completed successfully"
            );
            debug!(
                stderr_output = %stderr,
                "FFmpeg stderr output"
            );
        } else {
            error!(
                exit_code = status.code(),
                stderr_output = %stderr,
                "FFmpeg process failed"
            );
        }

        Ok((status, stderr))
    }

    /// 检查进程是否仍在运行
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.child
            .try_wait()
            .context("Failed to check FFmpeg status")
    }

    /// 强制终止进程
    pub fn kill(&mut self) -> Result<()> {
        self.child.kill().context("Failed to kill FFmpeg process")
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.7: FfmpegProgressParser - 统一的进度解析器
// ═══════════════════════════════════════════════════════════════

/// FFmpeg 进度解析器 - 统一解析 FFmpeg 输出的进度信息
///
/// # 支持的格式
///
/// - `frame=  123` - 当前帧数
/// - `fps=24.5` - 当前帧率
/// - `time=00:01:23.45` - 当前时间
/// - `speed=1.5x` - 编码速度
///
/// # 使用示例
///
/// ```ignore
/// let mut parser = FfmpegProgressParser::new(Some(1000)); // 总帧数
///
/// for line in stdout.lines() {
///     if let Some(progress) = parser.parse_line(&line?) {
///         println!("Progress: {:.1}%", progress * 100.0);
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct FfmpegProgressParser {
    /// 总帧数（如果已知）
    total_frames: Option<u64>,
    /// 总时长（秒，如果已知）
    total_duration: Option<f64>,
    /// 当前帧数
    current_frame: u64,
    /// 当前时间（秒）
    current_time: f64,
    /// 当前帧率
    current_fps: f64,
    /// 编码速度
    current_speed: f64,
}

impl FfmpegProgressParser {
    /// 创建新的进度解析器
    ///
    /// # Arguments
    /// * `total_frames` - 总帧数（如果已知）
    pub fn new(total_frames: Option<u64>) -> Self {
        Self {
            total_frames,
            total_duration: None,
            current_frame: 0,
            current_time: 0.0,
            current_fps: 0.0,
            current_speed: 0.0,
        }
    }

    /// 创建带时长的进度解析器
    ///
    /// # Arguments
    /// * `total_duration` - 总时长（秒）
    pub fn with_duration(total_duration: f64) -> Self {
        Self {
            total_frames: None,
            total_duration: Some(total_duration),
            current_frame: 0,
            current_time: 0.0,
            current_fps: 0.0,
            current_speed: 0.0,
        }
    }

    /// 解析 FFmpeg 进度行
    ///
    /// # Arguments
    /// * `line` - FFmpeg 输出的一行
    ///
    /// # Returns
    /// 进度百分比 (0.0 - 1.0)，如果无法计算则返回 None
    pub fn parse_line(&mut self, line: &str) -> Option<f64> {
        // 解析 frame=
        if let Some(frame_str) = line.strip_prefix("frame=") {
            if let Ok(frame) = frame_str.trim().split_whitespace().next()?.parse::<u64>() {
                self.current_frame = frame;
            }
        }

        // 解析 fps=
        if let Some(fps_str) = line.strip_prefix("fps=") {
            if let Ok(fps) = fps_str.trim().split_whitespace().next()?.parse::<f64>() {
                self.current_fps = fps;
            }
        }

        // 解析 time=
        if let Some(time_str) = line.strip_prefix("time=") {
            if let Some(time) = Self::parse_time(time_str.trim().split_whitespace().next()?) {
                self.current_time = time;
            }
        }

        // 解析 speed=
        if let Some(speed_str) = line.strip_prefix("speed=") {
            let speed_str = speed_str.trim().trim_end_matches('x');
            if let Ok(speed) = speed_str.parse::<f64>() {
                self.current_speed = speed;
            }
        }

        // 计算进度
        self.calculate_progress()
    }

    /// 解析时间字符串 (HH:MM:SS.ms)
    fn parse_time(time_str: &str) -> Option<f64> {
        let parts: Vec<&str> = time_str.split(':').collect();
        if parts.len() != 3 {
            return None;
        }

        let hours: f64 = parts[0].parse().ok()?;
        let minutes: f64 = parts[1].parse().ok()?;
        let seconds: f64 = parts[2].parse().ok()?;

        Some(hours * 3600.0 + minutes * 60.0 + seconds)
    }

    /// 计算当前进度
    fn calculate_progress(&self) -> Option<f64> {
        // 优先使用帧数计算
        if let Some(total) = self.total_frames {
            if total > 0 && self.current_frame > 0 {
                return Some((self.current_frame as f64 / total as f64).min(1.0));
            }
        }

        // 其次使用时长计算
        if let Some(total) = self.total_duration {
            if total > 0.0 && self.current_time > 0.0 {
                return Some((self.current_time / total).min(1.0));
            }
        }

        None
    }

    /// 获取当前帧数
    pub fn current_frame(&self) -> u64 {
        self.current_frame
    }

    /// 获取当前时间（秒）
    pub fn current_time(&self) -> f64 {
        self.current_time
    }

    /// 获取当前帧率
    pub fn current_fps(&self) -> f64 {
        self.current_fps
    }

    /// 获取编码速度
    pub fn current_speed(&self) -> f64 {
        self.current_speed
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.7: FFmpeg 错误格式化
// ═══════════════════════════════════════════════════════════════

/// 统一的 FFmpeg 错误格式化
///
/// 从 stderr 输出中提取最有意义的错误信息。
///
/// # Arguments
/// * `stderr` - FFmpeg 的 stderr 输出
///
/// # Returns
/// 格式化后的错误消息
///
/// # 提取逻辑
///
/// 1. 跳过空行和进度行（frame=...）
/// 2. 优先查找包含 "Error" 或 "error" 的行
/// 3. 如果没有，返回最后一行非空内容
/// 4. 如果全空，返回 "Unknown FFmpeg error"
pub fn format_ffmpeg_error(stderr: &str) -> String {
    // 优先查找包含 Error 的行
    if let Some(error_line) = stderr
        .lines()
        .rev()
        .find(|line| line.contains("Error") || line.contains("error"))
    {
        return error_line.trim().to_string();
    }

    // 其次返回最后一行有意义的内容
    stderr
        .lines()
        .rev()
        .find(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("frame=")
                && !trimmed.starts_with("fps=")
                && !trimmed.starts_with("size=")
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Unknown FFmpeg error".to_string())
}

/// 检查 FFmpeg 错误是否为可恢复的临时错误
pub fn is_recoverable_error(stderr: &str) -> bool {
    let recoverable_patterns = [
        "Resource temporarily unavailable",
        "Cannot allocate memory",
        "Too many open files",
        "Connection reset",
        "Broken pipe",
    ];
    recoverable_patterns
        .iter()
        .any(|pattern| stderr.contains(pattern))
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.5: 详细的 FFmpeg 错误报告
// ═══════════════════════════════════════════════════════════════

/// FFmpeg 错误详情
#[derive(Debug, Clone)]
pub struct FfmpegError {
    /// 完整命令行
    pub command: String,
    /// stdout 输出
    pub stdout: String,
    /// stderr 输出
    pub stderr: String,
    /// 退出码
    pub exit_code: Option<i32>,
    /// 可操作的建议
    pub suggestion: Option<String>,
}

impl std::fmt::Display for FfmpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "❌ FFMPEG ERROR")?;
        writeln!(f, "   Command: {}", self.command)?;
        if let Some(code) = self.exit_code {
            writeln!(f, "   Exit code: {}", code)?;
        }
        writeln!(f, "   Error: {}", format_ffmpeg_error(&self.stderr))?;
        if let Some(ref suggestion) = self.suggestion {
            writeln!(f, "   💡 Suggestion: {}", suggestion)?;
        }
        Ok(())
    }
}

impl std::error::Error for FfmpegError {}

/// 🔥 v6.5: 解析常见错误模式并提供建议
pub fn get_error_suggestion(stderr: &str) -> Option<String> {
    let patterns = [
        ("No such file or directory", "检查输入文件路径是否正确"),
        ("Invalid data found", "输入文件可能已损坏，尝试重新下载"),
        ("Encoder", "安装对应的编码器 (如 libx265, libsvtav1)"),
        ("not found", "检查 FFmpeg 是否正确安装"),
        ("Permission denied", "检查文件权限，确保有读写权限"),
        ("Output file is empty", "编码失败，尝试降低质量参数"),
        ("Avi header", "AVI 文件头损坏，尝试使用 -fflags +genpts"),
        (
            "moov atom not found",
            "MP4 文件不完整，尝试使用 -movflags faststart",
        ),
        (
            "Invalid NAL unit size",
            "视频流损坏，尝试使用 -err_detect ignore_err",
        ),
        ("Discarding", "部分帧被丢弃，可能是时间戳问题"),
        (
            "Too many packets buffered",
            "增加 -max_muxing_queue_size 参数",
        ),
    ];

    for (pattern, suggestion) in patterns {
        if stderr.contains(pattern) {
            return Some(suggestion.to_string());
        }
    }
    None
}

/// 🔥 v6.5: 运行 FFmpeg 并返回详细错误报告
pub fn run_ffmpeg_with_error_report(args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(args);

    let command_str = format!("ffmpeg {}", args.join(" "));

    // 记录即将执行的命令
    info!(
        command = %command_str,
        "Executing FFmpeg command"
    );

    let output = cmd.output().context("Failed to execute FFmpeg")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        let error = FfmpegError {
            command: command_str,
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            exit_code: output.status.code(),
            suggestion: get_error_suggestion(&stderr),
        };

        // 🔥 响亮报错 - 使用tracing记录详细错误信息
        error!(
            command = %error.command,
            exit_code = ?error.exit_code,
            stderr = %error.stderr,
            stdout = %error.stdout,
            suggestion = ?error.suggestion,
            "FFmpeg command failed"
        );

        // 同时输出到stderr供用户查看
        eprintln!("{}", error);

        return Err(anyhow::anyhow!(error));
    }

    // 记录成功执行
    info!(
        exit_code = output.status.code(),
        "FFmpeg command completed successfully"
    );
    debug!(
        stdout_length = output.stdout.len(),
        stderr_length = output.stderr.len(),
        "FFmpeg output captured"
    );

    Ok(output)
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.7: 单元测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ffmpeg_error_with_error_line() {
        let stderr = r#"
frame=  100 fps=25.0 q=28.0 size=    1024kB time=00:00:04.00 bitrate=2097.2kbits/s
[libx265 @ 0x7f8b8c000000] Error: invalid parameter
"#;
        let error = format_ffmpeg_error(stderr);
        assert!(error.contains("Error"));
        assert!(error.contains("invalid parameter"));
    }

    #[test]
    fn test_format_ffmpeg_error_no_error_line() {
        let stderr = r#"
frame=  100 fps=25.0 q=28.0 size=    1024kB time=00:00:04.00
Conversion failed!
"#;
        let error = format_ffmpeg_error(stderr);
        assert_eq!(error, "Conversion failed!");
    }

    #[test]
    fn test_format_ffmpeg_error_empty() {
        let error = format_ffmpeg_error("");
        assert_eq!(error, "Unknown FFmpeg error");
    }

    #[test]
    fn test_progress_parser_frame() {
        let mut parser = FfmpegProgressParser::new(Some(1000));
        let progress = parser.parse_line("frame=  500");
        assert_eq!(progress, Some(0.5));
        assert_eq!(parser.current_frame(), 500);
    }

    #[test]
    fn test_progress_parser_time() {
        let mut parser = FfmpegProgressParser::with_duration(120.0);
        let progress = parser.parse_line("time=00:01:00.00");
        assert_eq!(progress, Some(0.5));
        assert!((parser.current_time() - 60.0).abs() < 0.01);
    }

    #[test]
    fn test_progress_parser_fps() {
        let mut parser = FfmpegProgressParser::new(None);
        parser.parse_line("fps=29.97");
        assert!((parser.current_fps() - 29.97).abs() < 0.01);
    }

    #[test]
    fn test_is_recoverable_error() {
        assert!(is_recoverable_error("Resource temporarily unavailable"));
        assert!(is_recoverable_error("Cannot allocate memory"));
        assert!(!is_recoverable_error("Invalid input file"));
    }
}

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.4.7: 属性测试
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// **Feature: code-quality-v6.4.7, Property 4: FFmpeg 进度解析正确性**
        /// *对于任意*有效的帧数，进度解析应返回正确的百分比
        /// **验证: Requirements 3.1, 3.2, 3.3**
        #[test]
        fn prop_progress_parser_frame_accuracy(
            current in 0u64..10000,
            total in 1u64..10000
        ) {
            let mut parser = FfmpegProgressParser::new(Some(total));
            let line = format!("frame={}", current);
            let progress = parser.parse_line(&line);

            if current > 0 {
                let expected = (current as f64 / total as f64).min(1.0);
                prop_assert!(progress.is_some());
                let actual = progress.unwrap();
                prop_assert!((actual - expected).abs() < 0.001,
                    "Expected {}, got {} for frame {}/{}", expected, actual, current, total);
            }
        }

        /// **Feature: code-quality-v6.4.7, Property 4b: 时间解析正确性**
        /// *对于任意*有效的时间，进度解析应返回正确的百分比
        #[test]
        fn prop_progress_parser_time_accuracy(
            hours in 0u32..24,
            minutes in 0u32..60,
            seconds in 0u32..60,
            total_duration in 1.0f64..86400.0
        ) {
            let mut parser = FfmpegProgressParser::with_duration(total_duration);
            let line = format!("time={:02}:{:02}:{:02}.00", hours, minutes, seconds);
            let progress = parser.parse_line(&line);

            let current_seconds = hours as f64 * 3600.0 + minutes as f64 * 60.0 + seconds as f64;
            if current_seconds > 0.0 {
                let expected = (current_seconds / total_duration).min(1.0);
                prop_assert!(progress.is_some());
                let actual = progress.unwrap();
                prop_assert!((actual - expected).abs() < 0.01,
                    "Expected {}, got {} for time {}:{}:{}", expected, actual, hours, minutes, seconds);
            }
        }

        /// **Feature: code-quality-v6.4.7, Property 4c: 错误格式化非空**
        /// *对于任意*非空 stderr，format_ffmpeg_error 应返回非空字符串
        #[test]
        fn prop_format_error_non_empty(
            content in "[a-zA-Z0-9 ]{1,100}"
        ) {
            let error = format_ffmpeg_error(&content);
            prop_assert!(!error.is_empty(), "Error message should not be empty");
        }

        /// **Feature: code-quality-v6.4.7, Property 4d: 错误格式化优先 Error 行**
        /// 如果 stderr 包含 "Error"，应优先返回该行
        #[test]
        fn prop_format_error_prefers_error_line(
            prefix in "[a-zA-Z ]{0,50}",
            suffix in "[a-zA-Z ]{0,50}"
        ) {
            let stderr = format!("{}\nError: test error message\n{}", prefix, suffix);
            let error = format_ffmpeg_error(&stderr);
            prop_assert!(error.contains("Error"),
                "Should contain 'Error', got: {}", error);
        }
    }
}

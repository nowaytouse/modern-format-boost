//! Common Utilities Module
//!
//! 🔥 v7.8: 通用工具函数集合
//!
//! 本模块提取了项目中重复出现的常见模式，包括：
//! - 文件操作辅助函数
//! - 字符串处理工具
//! - 命令执行辅助函数
//! - 路径处理工具
//!
//! ## 设计原则
//! - 单一职责：每个函数只做一件事
//! - 可复用性：函数设计通用，不依赖特定上下文
//! - 错误透明：所有错误都包含详细上下文
//! - 完整文档：每个函数都有清晰的文档和示例

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tracing::{debug, error, info};

// ═══════════════════════════════════════════════════════════════
// 文件操作工具 (File Operations)
// ═══════════════════════════════════════════════════════════════

/// 安全地获取文件扩展名（小写）
///
/// 从文件路径中提取扩展名，自动转换为小写，如果没有扩展名则返回空字符串。
///
/// # Arguments
/// * `path` - 文件路径
///
/// # Returns
/// 小写的文件扩展名，如果没有扩展名则返回空字符串
///
/// # Examples
/// ```
/// use std::path::Path;
/// use shared_utils::common_utils::get_extension_lowercase;
///
/// assert_eq!(get_extension_lowercase(Path::new("test.JPG")), "jpg");
/// assert_eq!(get_extension_lowercase(Path::new("test.mp4")), "mp4");
/// assert_eq!(get_extension_lowercase(Path::new("noext")), "");
/// ```
pub fn get_extension_lowercase(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

/// 检查文件扩展名是否在给定列表中（不区分大小写）
///
/// # Arguments
/// * `path` - 文件路径
/// * `extensions` - 扩展名列表（不需要包含点号）
///
/// # Returns
/// 如果文件扩展名在列表中返回 true，否则返回 false
///
/// # Examples
/// ```
/// use std::path::Path;
/// use shared_utils::common_utils::has_extension;
///
/// let extensions = &["jpg", "png", "gif"];
/// assert!(has_extension(Path::new("photo.JPG"), extensions));
/// assert!(has_extension(Path::new("image.png"), extensions));
/// assert!(!has_extension(Path::new("video.mp4"), extensions));
/// ```
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    let ext = get_extension_lowercase(path);
    extensions.contains(&ext.as_str())
}

/// 检查文件是否为隐藏文件（以点号开头）
///
/// # Arguments
/// * `path` - 文件路径
///
/// # Returns
/// 如果是隐藏文件返回 true，否则返回 false
///
/// # Examples
/// ```
/// use std::path::Path;
/// use shared_utils::common_utils::is_hidden_file;
///
/// assert!(is_hidden_file(Path::new(".DS_Store")));
/// assert!(is_hidden_file(Path::new(".gitignore")));
/// assert!(!is_hidden_file(Path::new("normal.txt")));
/// ```
pub fn is_hidden_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

/// 安全地创建目录（包括父目录）
///
/// 如果目录已存在则不报错，自动创建所有必需的父目录。
/// 所有错误都包含目录路径上下文。
///
/// # Arguments
/// * `dir` - 要创建的目录路径
///
/// # Returns
/// 成功返回 Ok(())，失败返回包含上下文的错误
///
/// # Examples
/// ```no_run
/// use std::path::Path;
/// use shared_utils::common_utils::ensure_dir_exists;
///
/// ensure_dir_exists(Path::new("/tmp/test/nested/dir")).unwrap();
/// ```
pub fn ensure_dir_exists(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))
}

/// 安全地创建文件的父目录
///
/// 从文件路径中提取父目录并创建，如果文件没有父目录则不执行任何操作。
///
/// # Arguments
/// * `file_path` - 文件路径
///
/// # Returns
/// 成功返回 Ok(())，失败返回包含上下文的错误
///
/// # Examples
/// ```no_run
/// use std::path::Path;
/// use shared_utils::common_utils::ensure_parent_dir_exists;
///
/// ensure_parent_dir_exists(Path::new("/tmp/test/file.txt")).unwrap();
/// ```
pub fn ensure_parent_dir_exists(file_path: &Path) -> Result<()> {
    if let Some(parent) = file_path.parent() {
        ensure_dir_exists(parent)?;
    }
    Ok(())
}

/// 计算相对路径
///
/// 计算从 base 到 path 的相对路径，如果无法计算则返回原路径。
///
/// # Arguments
/// * `path` - 目标路径
/// * `base` - 基准路径
///
/// # Returns
/// 相对路径，如果无法计算则返回原路径
///
/// # Examples
/// ```
/// use std::path::{Path, PathBuf};
/// use shared_utils::common_utils::compute_relative_path;
///
/// let base = Path::new("/home/user/project");
/// let path = Path::new("/home/user/project/src/main.rs");
/// let rel = compute_relative_path(path, base);
/// assert_eq!(rel, PathBuf::from("src/main.rs"));
/// ```
pub fn compute_relative_path(path: &Path, base: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| path.to_path_buf())
}

/// 安全地复制文件（带上下文错误）
///
/// 复制文件并在错误时提供详细的源和目标路径信息。
///
/// # Arguments
/// * `source` - 源文件路径
/// * `dest` - 目标文件路径
///
/// # Returns
/// 成功返回复制的字节数，失败返回包含上下文的错误
///
/// # Examples
/// ```no_run
/// use std::path::Path;
/// use shared_utils::common_utils::copy_file_with_context;
///
/// let bytes = copy_file_with_context(
///     Path::new("source.txt"),
///     Path::new("dest.txt")
/// ).unwrap();
/// println!("Copied {} bytes", bytes);
/// ```
pub fn copy_file_with_context(source: &Path, dest: &Path) -> Result<u64> {
    std::fs::copy(source, dest).with_context(|| {
        format!(
            "Failed to copy file from {} to {}",
            source.display(),
            dest.display()
        )
    })
}

// ═══════════════════════════════════════════════════════════════
// 字符串处理工具 (String Processing)
// ═══════════════════════════════════════════════════════════════

/// 规范化路径字符串
///
/// 将路径中的反斜杠转换为正斜杠，移除多余的斜杠。
///
/// # Arguments
/// * `path_str` - 路径字符串
///
/// # Returns
/// 规范化后的路径字符串
///
/// # Examples
/// ```
/// use shared_utils::common_utils::normalize_path_string;
///
/// assert_eq!(normalize_path_string("C:\\Users\\test"), "C:/Users/test");
/// assert_eq!(normalize_path_string("path//to///file"), "path/to/file");
/// ```
pub fn normalize_path_string(path_str: &str) -> String {
    let mut result = path_str.replace('\\', "/");
    // 移除连续的斜杠
    while result.contains("//") {
        result = result.replace("//", "/");
    }
    result
}

/// 截断字符串到指定长度（添加省略号）
///
/// 如果字符串长度超过 max_len，则截断并添加 "..." 后缀。
///
/// # Arguments
/// * `s` - 要截断的字符串
/// * `max_len` - 最大长度（包括省略号）
///
/// # Returns
/// 截断后的字符串
///
/// # Examples
/// ```
/// use shared_utils::common_utils::truncate_string;
///
/// assert_eq!(truncate_string("Hello, World!", 10), "Hello, ...");
/// assert_eq!(truncate_string("Short", 10), "Short");
/// ```
pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        "...".to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

/// 从字符串中提取数字
///
/// 提取字符串中的所有数字字符并组合成一个字符串。
///
/// # Arguments
/// * `s` - 输入字符串
///
/// # Returns
/// 只包含数字的字符串
///
/// # Examples
/// ```
/// use shared_utils::common_utils::extract_digits;
///
/// assert_eq!(extract_digits("abc123def456"), "123456");
/// assert_eq!(extract_digits("no digits here"), "");
/// ```
pub fn extract_digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// 安全地解析浮点数
///
/// 尝试将字符串解析为 f64，失败时返回默认值。
///
/// # Arguments
/// * `s` - 要解析的字符串
/// * `default` - 解析失败时的默认值
///
/// # Returns
/// 解析的浮点数或默认值
///
/// # Examples
/// ```
/// use shared_utils::common_utils::parse_float_or_default;
///
/// assert_eq!(parse_float_or_default("3.14", 0.0), 3.14);
/// assert_eq!(parse_float_or_default("invalid", 1.0), 1.0);
/// ```
pub fn parse_float_or_default(s: &str, default: f64) -> f64 {
    s.parse::<f64>().unwrap_or(default)
}

// ═══════════════════════════════════════════════════════════════
// 命令执行工具 (Command Execution)
// ═══════════════════════════════════════════════════════════════

/// 执行命令并记录日志
///
/// 执行外部命令，记录命令行、执行结果和输出到日志。
/// 所有错误都包含命令和输出的完整上下文。
///
/// # Arguments
/// * `cmd` - 要执行的命令
///
/// # Returns
/// 成功返回命令输出，失败返回包含上下文的错误
///
/// # Examples
/// ```no_run
/// use std::process::Command;
/// use shared_utils::common_utils::execute_command_with_logging;
///
/// let mut cmd = Command::new("echo");
/// cmd.arg("Hello, World!");
/// let output = execute_command_with_logging(&mut cmd).unwrap();
/// ```
pub fn execute_command_with_logging(cmd: &mut Command) -> Result<Output> {
    let command_str = format!("{:?}", cmd);
    
    info!(
        command = %command_str,
        "Executing external command"
    );
    
    let output = cmd
        .output()
        .with_context(|| format!("Failed to execute command: {}", command_str))?;
    
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    
    if output.status.success() {
        info!(
            command = %command_str,
            exit_code = output.status.code(),
            "Command completed successfully"
        );
        debug!(
            stdout = %stdout,
            stderr = %stderr,
            "Command output"
        );
    } else {
        error!(
            command = %command_str,
            exit_code = output.status.code(),
            stdout = %stdout,
            stderr = %stderr,
            "Command failed"
        );
    }
    
    Ok(output)
}

/// 检查命令是否可用
///
/// 尝试执行命令的 --version 或 -version 参数来检查命令是否存在。
///
/// # Arguments
/// * `command_name` - 命令名称
///
/// # Returns
/// 如果命令可用返回 true，否则返回 false
///
/// # Examples
/// ```no_run
/// use shared_utils::common_utils::is_command_available;
///
/// // Check if a command is available
/// if is_command_available("ffmpeg") {
///     println!("ffmpeg is available");
/// }
/// ```
pub fn is_command_available(command_name: &str) -> bool {
    Command::new(command_name)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(command_name).arg("-version").output())
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 获取命令版本字符串
///
/// 执行命令的 --version 或 -version 参数并返回第一行输出。
///
/// # Arguments
/// * `command_name` - 命令名称
///
/// # Returns
/// 版本字符串，如果无法获取则返回 None
///
/// # Examples
/// ```
/// use shared_utils::common_utils::get_command_version;
///
/// if let Some(version) = get_command_version("rustc") {
///     println!("Rust version: {}", version);
/// }
/// ```
pub fn get_command_version(command_name: &str) -> Option<String> {
    let output = Command::new(command_name)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(command_name).arg("-version").output())
        .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.lines().next().map(|s| s.to_string())
    } else {
        None
    }
}

/// 构建命令参数字符串（用于日志）
///
/// 将命令和参数格式化为易读的字符串，用于日志记录。
///
/// # Arguments
/// * `command` - 命令名称
/// * `args` - 参数列表
///
/// # Returns
/// 格式化的命令字符串
///
/// # Examples
/// ```
/// use shared_utils::common_utils::format_command_string;
///
/// let cmd_str = format_command_string("ffmpeg", &["-i", "input.mp4", "output.mp4"]);
/// assert_eq!(cmd_str, "ffmpeg -i input.mp4 output.mp4");
/// ```
pub fn format_command_string(command: &str, args: &[&str]) -> String {
    if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    }
}

// ═══════════════════════════════════════════════════════════════
// 测试 (Tests)
// ═══════════════════════════════════════════════════════════════

// 🔥 v7.9: Validate file integrity (size checks)
// 防止处理空文件或过小的损坏文件导致 panic
pub fn validate_file_integrity(path: &std::path::Path) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();

    // 1. 空文件检查
    if size == 0 {
        anyhow::bail!("File is empty (0 bytes)");
    }

    // 2. 过小文件检查 (最小 GIF 头是 13 字节)
    // 很多图片格式头至少都有几十字节
    if size < 12 {
        anyhow::bail!("File is too small (< 12 bytes) to be a valid image");
    }

    Ok(())
}

// 🔥 v7.9: Validate max file size (prevent OOM)
pub fn validate_file_size_limit(path: &std::path::Path, max_bytes: u64) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();

    if size > max_bytes {
        anyhow::bail!(
            "File is too large ({} bytes > {} max allowed)",
            size,
            max_bytes
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // 文件操作测试
    #[test]
    fn test_get_extension_lowercase() {
        assert_eq!(get_extension_lowercase(Path::new("test.JPG")), "jpg");
        assert_eq!(get_extension_lowercase(Path::new("test.mp4")), "mp4");
        assert_eq!(get_extension_lowercase(Path::new("noext")), "");
        assert_eq!(get_extension_lowercase(Path::new(".hidden")), "");
    }

    #[test]
    fn test_has_extension() {
        let extensions = &["jpg", "png", "gif"];
        assert!(has_extension(Path::new("photo.JPG"), extensions));
        assert!(has_extension(Path::new("image.png"), extensions));
        assert!(!has_extension(Path::new("video.mp4"), extensions));
    }

    #[test]
    fn test_is_hidden_file() {
        assert!(is_hidden_file(Path::new(".DS_Store")));
        assert!(is_hidden_file(Path::new(".gitignore")));
        assert!(!is_hidden_file(Path::new("normal.txt")));
    }

    #[test]
    fn test_ensure_dir_exists() {
        let temp = TempDir::new().unwrap();
        let nested = temp.path().join("a/b/c");
        
        ensure_dir_exists(&nested).unwrap();
        assert!(nested.exists());
        assert!(nested.is_dir());
        
        // 再次调用应该成功（幂等性）
        ensure_dir_exists(&nested).unwrap();
    }

    #[test]
    fn test_ensure_parent_dir_exists() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("a/b/c/file.txt");
        
        ensure_parent_dir_exists(&file_path).unwrap();
        assert!(file_path.parent().unwrap().exists());
    }

    #[test]
    fn test_compute_relative_path() {
        let base = Path::new("/home/user/project");
        let path = Path::new("/home/user/project/src/main.rs");
        let rel = compute_relative_path(path, base);
        assert_eq!(rel, PathBuf::from("src/main.rs"));
        
        // 无法计算相对路径时返回原路径
        let unrelated = Path::new("/tmp/file.txt");
        let rel2 = compute_relative_path(unrelated, base);
        assert_eq!(rel2, unrelated);
    }

    #[test]
    fn test_copy_file_with_context() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source.txt");
        let dest = temp.path().join("dest.txt");
        
        fs::write(&source, "test content").unwrap();
        
        let bytes = copy_file_with_context(&source, &dest).unwrap();
        assert_eq!(bytes, 12); // "test content" 的长度
        assert_eq!(fs::read_to_string(&dest).unwrap(), "test content");
    }

    // 字符串处理测试
    #[test]
    fn test_normalize_path_string() {
        assert_eq!(normalize_path_string("C:\\Users\\test"), "C:/Users/test");
        assert_eq!(normalize_path_string("path//to///file"), "path/to/file");
        assert_eq!(normalize_path_string("normal/path"), "normal/path");
    }

    #[test]
    fn test_truncate_string() {
        assert_eq!(truncate_string("Hello, World!", 10), "Hello, ...");
        assert_eq!(truncate_string("Short", 10), "Short");
        assert_eq!(truncate_string("Exact", 5), "Exact");
        assert_eq!(truncate_string("Too long", 3), "...");
    }

    #[test]
    fn test_extract_digits() {
        assert_eq!(extract_digits("abc123def456"), "123456");
        assert_eq!(extract_digits("no digits here"), "");
        assert_eq!(extract_digits("2024-01-15"), "20240115");
    }

    #[test]
    fn test_parse_float_or_default() {
        assert_eq!(parse_float_or_default("5.67", 0.0), 5.67);
        assert_eq!(parse_float_or_default("invalid", 1.0), 1.0);
        assert_eq!(parse_float_or_default("", 2.5), 2.5);
    }

    // 命令执行测试
    #[test]
    fn test_is_command_available() {
        // 测试一个肯定存在的命令（跨平台兼容）
        #[cfg(unix)]
        {
            // Unix系统上测试sh（更可靠）
            assert!(is_command_available("sh"));
        }
        
        #[cfg(windows)]
        {
            // Windows系统上测试cmd
            assert!(is_command_available("cmd"));
        }
        
        // 测试一个不存在的命令
        assert!(!is_command_available("nonexistent_command_xyz_123"));
    }

    #[test]
    fn test_format_command_string() {
        assert_eq!(
            format_command_string("ffmpeg", &["-i", "input.mp4", "output.mp4"]),
            "ffmpeg -i input.mp4 output.mp4"
        );
        assert_eq!(format_command_string("ls", &[]), "ls");
    }

    #[test]
    fn test_execute_command_with_logging() {
        let mut cmd = Command::new("echo");
        cmd.arg("test");
        
        let output = execute_command_with_logging(&mut cmd).unwrap();
        assert!(output.status.success());
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test"));
    }
}

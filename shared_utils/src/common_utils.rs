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


#[inline]
pub fn get_extension_lowercase(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default()
}

#[inline]
pub fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| extensions.iter().any(|ext| ext.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

#[inline]
pub fn is_hidden_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

pub fn extract_suggested_extension(error_msg: &str) -> Option<String> {
    if let Some(start) = error_msg.find("looks more like a ") {
        let rest = &error_msg[start + "looks more like a ".len()..];
        if let Some(end) = rest.find(')') {
            return Some(rest[..end].trim().to_lowercase());
        }
    }
    None
}

pub fn ensure_dir_exists(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create directory: {}", dir.display()))
}

pub fn ensure_parent_dir_exists(file_path: &Path) -> Result<()> {
    if let Some(parent) = file_path.parent() {
        ensure_dir_exists(parent)?;
    }
    Ok(())
}

pub fn compute_relative_path(path: &Path, base: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| path.to_path_buf())
}

pub fn copy_file_with_context(source: &Path, dest: &Path) -> Result<u64> {
    std::fs::copy(source, dest).with_context(|| {
        format!(
            "Failed to copy file from {} to {}",
            source.display(),
            dest.display()
        )
    })
}

pub fn detect_real_extension(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = [0u8; 12];
    let bytes_read = file.read(&mut buffer).ok()?;

    if bytes_read < 4 {
        return None;
    }

    if buffer[0] == 0xFF && buffer[1] == 0xD8 && buffer[2] == 0xFF {
        return Some("jpg");
    }

    if buffer[0] == 0x89 && buffer[1] == 0x50 && buffer[2] == 0x4E && buffer[3] == 0x47 {
        return Some("png");
    }

    if buffer[0] == 0x47 && buffer[1] == 0x49 && buffer[2] == 0x46 && buffer[3] == 0x38 {
        return Some("gif");
    }

    if (buffer[0] == 0x49 && buffer[1] == 0x49 && buffer[2] == 0x2A && buffer[3] == 0x00)
        || (buffer[0] == 0x4D && buffer[1] == 0x4D && buffer[2] == 0x00 && buffer[3] == 0x2A)
    {
        return Some("tif");
    }

    if buffer[0] == 0x52
        && buffer[1] == 0x49
        && buffer[2] == 0x46
        && buffer[3] == 0x46
        && bytes_read >= 12
        && buffer[8] == 0x57
        && buffer[9] == 0x45
        && buffer[10] == 0x42
        && buffer[11] == 0x50
    {
        return Some("webp");
    }

    if bytes_read >= 2 && buffer[0] == 0xFF && buffer[1] == 0x0A {
        return Some("jxl");
    }

    if bytes_read >= 12
        && buffer[0] == 0x00
        && buffer[1] == 0x00
        && buffer[2] == 0x00
        && buffer[3] == 0x0C
        && buffer[4] == 0x4A
        && buffer[5] == 0x58
        && buffer[6] == 0x4C
        && buffer[7] == 0x20
        && buffer[8] == 0x0D
        && buffer[9] == 0x0A
        && buffer[10] == 0x87
        && buffer[11] == 0x0A
    {
        return Some("jxl");
    }

    if bytes_read >= 12
        && buffer[4] == 0x66
        && buffer[5] == 0x74
        && buffer[6] == 0x79
        && buffer[7] == 0x70
    {
        let brand = &buffer[8..12];
        if matches!(
            brand,
            b"heic" | b"heix" | b"heim" | b"heis" | b"mif1" | b"msf1"
        ) {
            return Some("heic");
        }
        if matches!(brand, b"avif" | b"avis") {
            return Some("avif");
        }
        return Some("mov");
    }

    None
}


pub fn normalize_path_string(path_str: &str) -> String {
    let mut result = path_str.replace('\\', "/");
    while result.contains("//") {
        result = result.replace("//", "/");
    }
    result
}

pub fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        "...".to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

pub fn extract_digits(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

pub fn parse_float_or_default(s: &str, default: f64) -> f64 {
    s.parse::<f64>().unwrap_or(default)
}


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

pub fn is_command_available(command_name: &str) -> bool {
    Command::new(command_name)
        .arg("--version")
        .output()
        .or_else(|_| Command::new(command_name).arg("-version").output())
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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

pub fn format_command_string(command: &str, args: &[&str]) -> String {
    if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    }
}


pub fn validate_file_integrity(path: &std::path::Path) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)?;
    let size = metadata.len();

    if size == 0 {
        anyhow::bail!("File is empty (0 bytes)");
    }

    if size < 12 {
        anyhow::bail!("File is too small (< 12 bytes) to be a valid image");
    }

    Ok(())
}

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
        assert_eq!(bytes, 12);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "test content");
    }

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

    #[test]
    fn test_is_command_available() {
        #[cfg(unix)]
        {
            assert!(is_command_available("sh"));
        }

        #[cfg(windows)]
        {
            assert!(is_command_available("cmd"));
        }

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

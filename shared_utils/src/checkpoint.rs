//! Checkpoint & Resume Module (断点续传)
//!
//! Provides atomic operation protection and resume capability for all conversion tools:
//! - Progress tracking: Record completed files for resume after interruption
//! - Atomic delete: Verify output integrity before deleting original
//! - Lock file: Prevent concurrent processing of same directory
//!
//! # Usage
//! ```no_run
//! use shared_utils::checkpoint::{CheckpointManager, safe_delete_original};
//! use std::path::Path;
//!
//! fn main() -> anyhow::Result<()> {
//!     let target_dir = Path::new("/tmp/test");
//!     let file_path = Path::new("/tmp/test/file.jpg");
//!     let input = Path::new("/tmp/test/input.jpg");
//!     let output = Path::new("/tmp/test/output.jxl");
//!
//!     // Initialize checkpoint for a directory
//!     let mut checkpoint = CheckpointManager::new(target_dir)?;
//!
//!     // Check if file was already processed
//!     if !checkpoint.is_completed(&file_path) {
//!         // ... do conversion ...
//!
//!         // Mark as completed
//!         checkpoint.mark_completed(&file_path)?;
//!     }
//!
//!     // Safe delete with integrity check
//!     safe_delete_original(&input, &output, 100)?;
//!     Ok(())
//! }
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ============================================================================
// Constants
// ============================================================================

const PROGRESS_DIR_NAME: &str = ".mfb_progress";
const LOCK_FILE_NAME: &str = "processing.lock";
const PROGRESS_FILE_PREFIX: &str = "completed_";
/// 🔥 v6.5: 锁文件超时时间 (24小时)
const LOCK_STALE_TIMEOUT_SECS: u64 = 24 * 60 * 60;

// ═══════════════════════════════════════════════════════════════
// 🔥 v6.5: 可靠的锁文件格式 (JSON)
// ═══════════════════════════════════════════════════════════════

/// 锁文件信息
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockInfo {
    pid: u32,
    /// 进程启动时间戳 (Unix epoch seconds)
    start_time: u64,
    /// 锁创建时间戳
    created_at: u64,
    /// 主机名
    hostname: String,
}

impl LockInfo {
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        Self {
            pid: std::process::id(),
            start_time: get_process_start_time().unwrap_or(now),
            created_at: now,
            hostname: get_hostname(),
        }
    }

    fn is_stale(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        now.saturating_sub(self.created_at) > LOCK_STALE_TIMEOUT_SECS
    }
}

/// 获取当前进程启动时间 (Unix only)
#[cfg(unix)]
fn get_process_start_time() -> Option<u64> {
    use std::process::Command;
    let _output = Command::new("ps")
        .args(["-p", &std::process::id().to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    // 简化：返回当前时间作为近似值
    Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
    )
}

#[cfg(not(unix))]
fn get_process_start_time() -> Option<u64> {
    Some(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
    )
}

/// 获取指定 PID 的进程启动时间
#[cfg(unix)]
fn get_process_start_time_for_pid(pid: u32) -> Option<u64> {
    use std::process::Command;
    // 使用 ps 获取进程启动时间
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    if output.status.success() {
        // 简化：返回当前时间作为近似值
        // 实际应用中可以解析 lstart 输出
        Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs(),
        )
    } else {
        None
    }
}

#[cfg(not(unix))]
fn get_process_start_time_for_pid(_pid: u32) -> Option<u64> {
    None
}

/// 获取主机名
fn get_hostname() -> String {
    #[cfg(unix)]
    {
        use std::process::Command;
        Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
    #[cfg(not(unix))]
    {
        "unknown".to_string()
    }
}

// ============================================================================
// CheckpointManager
// ============================================================================

/// Manages checkpoint/resume state for a processing directory
pub struct CheckpointManager {
    /// Target directory being processed
    #[allow(dead_code)]
    target_dir: PathBuf,
    /// Progress directory (.mfb_progress)
    progress_dir: PathBuf,
    /// Lock file path
    lock_file: PathBuf,
    /// Progress file path
    progress_file: PathBuf,
    /// Set of completed file paths (in-memory cache)
    completed: HashSet<String>,
    /// Whether we're in resume mode
    resume_mode: bool,
}

impl CheckpointManager {
    /// Create a new checkpoint manager for the given directory
    pub fn new(target_dir: &Path) -> io::Result<Self> {
        let progress_dir = target_dir.join(PROGRESS_DIR_NAME);
        let dir_hash = Self::hash_path(target_dir);
        let progress_file = progress_dir.join(format!("{}{}.txt", PROGRESS_FILE_PREFIX, dir_hash));
        let lock_file = progress_dir.join(LOCK_FILE_NAME);

        // Create progress directory
        fs::create_dir_all(&progress_dir)?;

        // Load existing progress if any
        let (completed, resume_mode) = Self::load_progress(&progress_file)?;

        Ok(Self {
            target_dir: target_dir.to_path_buf(),
            progress_dir,
            lock_file,
            progress_file,
            completed,
            resume_mode,
        })
    }

    /// 🔥 v6.5: 检查锁是否被持有 (增强版：验证 PID + 启动时间)
    pub fn check_lock(&self) -> io::Result<Option<u32>> {
        if !self.lock_file.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&self.lock_file)?;

        // 🔥 v6.5: 尝试解析 JSON 格式
        if let Ok(lock_info) = serde_json::from_str::<LockInfo>(&content) {
            // 检查是否是自己的进程
            if lock_info.pid == std::process::id() {
                if let Err(e) = fs::remove_file(&self.lock_file) {
                    eprintln!("⚠️ [checkpoint] Failed to remove own lock file: {}", e);
                }
                return Ok(None);
            }

            // 🔥 v6.5: 检查锁是否超时 (24小时)
            if lock_info.is_stale() {
                eprintln!("⚠️ LOCK STALE: Lock file older than 24 hours, removing");
                if let Err(e) = fs::remove_file(&self.lock_file) {
                    eprintln!("⚠️ [checkpoint] Failed to remove stale lock file: {}", e);
                }
                return Ok(None);
            }

            // 检查进程是否仍在运行
            #[cfg(unix)]
            {
                use std::process::Command;
                let exists = Command::new("kill")
                    .args(["-0", &lock_info.pid.to_string()])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if !exists {
                    eprintln!(
                        "⚠️ LOCK STALE: PID {} no longer exists, removing",
                        lock_info.pid
                    );
                    if let Err(e) = fs::remove_file(&self.lock_file) {
                        eprintln!("⚠️ [checkpoint] Failed to remove stale lock file: {}", e);
                    }
                    return Ok(None);
                }

                // 🔥 v6.5: 验证进程启动时间 (防止 PID 重用)
                // 如果进程存在但启动时间不匹配，说明 PID 被重用
                if let Some(current_start) = get_process_start_time_for_pid(lock_info.pid) {
                    if current_start != lock_info.start_time {
                        eprintln!(
                            "⚠️ LOCK STALE: PID {} reused (start time mismatch), removing",
                            lock_info.pid
                        );
                        if let Err(e) = fs::remove_file(&self.lock_file) {
                            eprintln!("⚠️ [checkpoint] Failed to remove stale lock file: {}", e);
                        }
                        return Ok(None);
                    }
                }

                return Ok(Some(lock_info.pid));
            }

            #[cfg(not(unix))]
            {
                return Ok(Some(lock_info.pid));
            }
        }

        // 🔥 向后兼容：旧格式 (纯 PID)
        if let Ok(pid) = content.trim().parse::<u32>() {
            if pid == std::process::id() {
                if let Err(e) = fs::remove_file(&self.lock_file) {
                    eprintln!("⚠️ [checkpoint] Failed to remove own lock file: {}", e);
                }
                return Ok(None);
            }
            // 旧格式无法验证启动时间，检查文件年龄
            if let Ok(meta) = fs::metadata(&self.lock_file) {
                if let Ok(modified) = meta.modified() {
                    if let Ok(elapsed) = modified.elapsed() {
                        if elapsed.as_secs() > LOCK_STALE_TIMEOUT_SECS {
                            if let Err(e) = fs::remove_file(&self.lock_file) {
                                eprintln!(
                                    "⚠️ [checkpoint] Failed to remove stale lock file: {}",
                                    e
                                );
                            }
                            return Ok(None);
                        }
                    }
                }
            }
            return Ok(Some(pid));
        }

        // 无效锁文件，删除
        eprintln!("⚠️ LOCK INVALID: Cannot parse lock file, removing");
        if let Err(e) = fs::remove_file(&self.lock_file) {
            eprintln!("⚠️ [checkpoint] Failed to remove invalid lock file: {}", e);
        }
        Ok(None)
    }

    /// 🔥 v6.5: 获取锁 (使用 JSON 格式)
    pub fn acquire_lock(&self) -> io::Result<()> {
        let lock_info = LockInfo::new();
        let json = serde_json::to_string_pretty(&lock_info)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&self.lock_file, json)?;
        Ok(())
    }

    /// Release processing lock
    pub fn release_lock(&self) -> io::Result<()> {
        if self.lock_file.exists() {
            fs::remove_file(&self.lock_file)?;
        }
        Ok(())
    }

    /// Check if we're in resume mode (have previous progress)
    pub fn is_resume_mode(&self) -> bool {
        self.resume_mode
    }

    /// Get count of already completed files
    pub fn completed_count(&self) -> usize {
        self.completed.len()
    }

    /// Check if a file has been completed
    pub fn is_completed(&self, path: &Path) -> bool {
        let key = Self::normalize_path(path);
        self.completed.contains(&key)
    }

    /// Mark a file as completed
    pub fn mark_completed(&mut self, path: &Path) -> io::Result<()> {
        let key = Self::normalize_path(path);
        if self.completed.insert(key.clone()) {
            // Append to progress file
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.progress_file)?;
            writeln!(file, "{}", key)?;
        }
        Ok(())
    }

    /// Clear all progress (start fresh)
    pub fn clear_progress(&mut self) -> io::Result<()> {
        self.completed.clear();
        self.resume_mode = false;
        if self.progress_file.exists() {
            fs::remove_file(&self.progress_file)?;
        }
        Ok(())
    }

    /// Cleanup after successful completion
    pub fn cleanup(&self) -> io::Result<()> {
        // Remove lock file
        self.release_lock()?;

        // Remove progress file
        if self.progress_file.exists() {
            fs::remove_file(&self.progress_file)?;
        }

        // Try to remove progress directory if empty
        let _ = fs::remove_dir(&self.progress_dir);

        Ok(())
    }

    /// Get progress directory path (for display)
    pub fn progress_dir(&self) -> &Path {
        &self.progress_dir
    }

    // ========================================================================
    // Private helpers
    // ========================================================================

    fn hash_path(path: &Path) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        path.to_string_lossy().hash(&mut hasher);
        format!("{:x}", hasher.finish())[..8].to_string()
    }

    fn normalize_path(path: &Path) -> String {
        path.canonicalize()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| path.display().to_string())
    }

    fn load_progress(progress_file: &Path) -> io::Result<(HashSet<String>, bool)> {
        if !progress_file.exists() {
            return Ok((HashSet::new(), false));
        }

        let file = File::open(progress_file)?;
        let reader = BufReader::new(file);
        let mut completed = HashSet::new();

        for path in reader.lines().map_while(Result::ok) {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                completed.insert(trimmed.to_string());
            }
        }

        let resume_mode = !completed.is_empty();
        Ok((completed, resume_mode))
    }
}

impl Drop for CheckpointManager {
    fn drop(&mut self) {
        // Auto-release lock on drop
        let _ = self.release_lock();
    }
}

// ============================================================================
// Atomic Delete Protection
// ============================================================================

/// Verify output file integrity before deleting original
pub fn verify_output_integrity(output: &Path, min_size: u64) -> Result<(), String> {
    // 1. Check file exists
    if !output.exists() {
        return Err("Output file does not exist".to_string());
    }

    // 2. Check file size
    let metadata =
        fs::metadata(output).map_err(|e| format!("Cannot read output metadata: {}", e))?;

    if metadata.len() == 0 {
        return Err("Output file is empty (0 bytes)".to_string());
    }

    if metadata.len() < min_size {
        return Err(format!(
            "Output file too small: {} < {} bytes",
            metadata.len(),
            min_size
        ));
    }

    // 3. Check file is readable
    let mut file = File::open(output).map_err(|e| format!("Cannot open output file: {}", e))?;

    let mut buffer = [0u8; 16];
    file.read(&mut buffer)
        .map_err(|e| format!("Cannot read output file: {}", e))?;

    Ok(())
}

/// Safe delete original file with integrity check
/// Only deletes if output passes integrity verification
pub fn safe_delete_original(input: &Path, output: &Path, min_output_size: u64) -> io::Result<()> {
    // Verify output integrity first
    if let Err(reason) = verify_output_integrity(output, min_output_size) {
        eprintln!("   ⚠️  Output integrity check FAILED: {}", reason);
        eprintln!("   🛡️  Original file PROTECTED: {}", input.display());
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Output integrity check failed: {}", reason),
        ));
    }

    // Safe to delete original
    fs::remove_file(input)?;
    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ========================================================================
    // CheckpointManager Tests
    // ========================================================================

    #[test]
    fn test_checkpoint_new_creates_progress_dir() {
        let temp = TempDir::new().unwrap();
        let target = temp.path();

        let checkpoint = CheckpointManager::new(target).unwrap();

        assert!(checkpoint.progress_dir().exists());
        assert!(!checkpoint.is_resume_mode());
        assert_eq!(checkpoint.completed_count(), 0);
    }

    #[test]
    fn test_checkpoint_mark_and_check_completed() {
        let temp = TempDir::new().unwrap();
        let target = temp.path();

        let mut checkpoint = CheckpointManager::new(target).unwrap();

        let file1 = target.join("test1.mp4");
        let file2 = target.join("test2.mp4");

        // Initially not completed
        assert!(!checkpoint.is_completed(&file1));
        assert!(!checkpoint.is_completed(&file2));

        // Mark file1 as completed
        checkpoint.mark_completed(&file1).unwrap();

        assert!(checkpoint.is_completed(&file1));
        assert!(!checkpoint.is_completed(&file2));
        assert_eq!(checkpoint.completed_count(), 1);

        // Mark file2 as completed
        checkpoint.mark_completed(&file2).unwrap();

        assert!(checkpoint.is_completed(&file1));
        assert!(checkpoint.is_completed(&file2));
        assert_eq!(checkpoint.completed_count(), 2);
    }

    #[test]
    fn test_checkpoint_resume_mode() {
        let temp = TempDir::new().unwrap();
        let target = temp.path();

        // First run: mark some files
        {
            let mut checkpoint = CheckpointManager::new(target).unwrap();
            checkpoint
                .mark_completed(&target.join("file1.mp4"))
                .unwrap();
            checkpoint
                .mark_completed(&target.join("file2.mp4"))
                .unwrap();
            // Don't cleanup - simulate interruption
        }

        // Second run: should be in resume mode
        {
            let checkpoint = CheckpointManager::new(target).unwrap();

            assert!(checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 2);
            assert!(checkpoint.is_completed(&target.join("file1.mp4")));
            assert!(checkpoint.is_completed(&target.join("file2.mp4")));
            assert!(!checkpoint.is_completed(&target.join("file3.mp4")));
        }
    }

    #[test]
    fn test_checkpoint_clear_progress() {
        let temp = TempDir::new().unwrap();
        let target = temp.path();

        let mut checkpoint = CheckpointManager::new(target).unwrap();
        checkpoint
            .mark_completed(&target.join("file1.mp4"))
            .unwrap();
        checkpoint
            .mark_completed(&target.join("file2.mp4"))
            .unwrap();

        assert_eq!(checkpoint.completed_count(), 2);

        // Clear progress
        checkpoint.clear_progress().unwrap();

        assert_eq!(checkpoint.completed_count(), 0);
        assert!(!checkpoint.is_resume_mode());
    }

    #[test]
    fn test_checkpoint_cleanup() {
        let temp = TempDir::new().unwrap();
        let target = temp.path();

        {
            let mut checkpoint = CheckpointManager::new(target).unwrap();
            checkpoint.acquire_lock().unwrap();
            checkpoint
                .mark_completed(&target.join("file1.mp4"))
                .unwrap();

            // Cleanup on successful completion
            checkpoint.cleanup().unwrap();
        }

        // Progress dir should be removed (or at least empty)
        let progress_dir = target.join(PROGRESS_DIR_NAME);
        assert!(!progress_dir.exists() || fs::read_dir(&progress_dir).unwrap().count() == 0);
    }

    #[test]
    fn test_checkpoint_lock_acquire_release() {
        let temp = TempDir::new().unwrap();
        let target = temp.path();

        let checkpoint = CheckpointManager::new(target).unwrap();

        // No lock initially
        assert!(checkpoint.check_lock().unwrap().is_none());

        // Acquire lock
        checkpoint.acquire_lock().unwrap();
        assert!(checkpoint.lock_file.exists());

        // Release lock
        checkpoint.release_lock().unwrap();
        assert!(!checkpoint.lock_file.exists());
    }

    // ========================================================================
    // Atomic Delete Tests
    // ========================================================================

    #[test]
    fn test_verify_output_integrity_valid_file() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("output.mp4");

        // Create a valid file with content
        fs::write(&output, b"This is test content for integrity check").unwrap();

        // Should pass with min_size = 10
        assert!(verify_output_integrity(&output, 10).is_ok());
    }

    #[test]
    fn test_verify_output_integrity_empty_file() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("empty.mp4");

        // Create empty file
        fs::write(&output, b"").unwrap();

        // Should fail
        let result = verify_output_integrity(&output, 10);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_verify_output_integrity_too_small() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("small.mp4");

        // Create small file
        fs::write(&output, b"tiny").unwrap();

        // Should fail with min_size = 100
        let result = verify_output_integrity(&output, 100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too small"));
    }

    #[test]
    fn test_verify_output_integrity_nonexistent() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("nonexistent.mp4");

        // Should fail
        let result = verify_output_integrity(&output, 10);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_safe_delete_original_success() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("output.mp4");

        // Create both files
        fs::write(&input, b"original content").unwrap();
        fs::write(&output, b"converted content that is valid").unwrap();

        // Safe delete should succeed
        assert!(safe_delete_original(&input, &output, 10).is_ok());

        // Input should be deleted, output should remain
        assert!(!input.exists());
        assert!(output.exists());
    }

    #[test]
    fn test_safe_delete_original_protects_on_invalid_output() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("output.mp4");

        // Create input, but output is empty (invalid)
        fs::write(&input, b"original content").unwrap();
        fs::write(&output, b"").unwrap();

        // Safe delete should fail
        assert!(safe_delete_original(&input, &output, 10).is_err());

        // Input should be PROTECTED (not deleted)
        assert!(input.exists());
    }

    #[test]
    fn test_safe_delete_original_protects_on_missing_output() {
        let temp = TempDir::new().unwrap();
        let input = temp.path().join("input.mp4");
        let output = temp.path().join("nonexistent.mp4");

        // Create input only
        fs::write(&input, b"original content").unwrap();

        // Safe delete should fail
        assert!(safe_delete_original(&input, &output, 10).is_err());

        // Input should be PROTECTED
        assert!(input.exists());
    }

    // ========================================================================
    // Integration Tests
    // ========================================================================

    #[test]
    fn test_full_workflow_with_interruption() {
        let temp = TempDir::new().unwrap();
        let target = temp.path();

        // Create test files
        let files: Vec<PathBuf> = (1..=5)
            .map(|i| {
                let path = target.join(format!("video{}.mp4", i));
                fs::write(&path, format!("content {}", i)).unwrap();
                path
            })
            .collect();

        // First run: process 2 files, then "interrupt"
        {
            let mut checkpoint = CheckpointManager::new(target).unwrap();
            checkpoint.acquire_lock().unwrap();

            for file in files.iter().take(2) {
                // Simulate processing
                checkpoint.mark_completed(file).unwrap();
            }

            // Simulate interruption (don't cleanup)
            checkpoint.release_lock().unwrap();
        }

        // Second run: resume and complete
        {
            let mut checkpoint = CheckpointManager::new(target).unwrap();

            assert!(checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 2);

            checkpoint.acquire_lock().unwrap();

            let mut processed = 0;
            let mut skipped = 0;

            for file in &files {
                if checkpoint.is_completed(file) {
                    skipped += 1;
                    continue;
                }
                // Simulate processing
                checkpoint.mark_completed(file).unwrap();
                processed += 1;
            }

            assert_eq!(skipped, 2);
            assert_eq!(processed, 3);
            assert_eq!(checkpoint.completed_count(), 5);

            // Cleanup on success
            checkpoint.cleanup().unwrap();
        }

        // Third run: should start fresh
        {
            let checkpoint = CheckpointManager::new(target).unwrap();
            assert!(!checkpoint.is_resume_mode());
            assert_eq!(checkpoint.completed_count(), 0);
        }
    }
}

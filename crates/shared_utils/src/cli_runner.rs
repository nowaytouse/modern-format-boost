use crate::batch::{disk_full_pause_reason, BatchPauseController, BatchResult};
use crate::common_utils::has_extension;
use crate::file_copier::{
    copy_unsupported_files, verify_output_completeness, SUPPORTED_VIDEO_EXTENSIONS,
};
use crate::report::print_summary_report;
use crate::smart_file_copier::fix_extension_if_mismatch;
use anyhow::Result;
use log::{error, info, warn};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub trait CliProcessingResult {
    fn is_skipped(&self) -> bool;
    fn is_success(&self) -> bool;
    fn skip_reason(&self) -> Option<&str>;
    fn input_path(&self) -> &str;
    fn output_path(&self) -> Option<&str>;
    fn input_size(&self) -> u64;
    fn output_size(&self) -> Option<u64>;
    fn message(&self) -> &str;
    fn blake3(&self) -> Option<&str>;
}

impl CliProcessingResult for crate::conversion::ConversionResult {
    fn is_skipped(&self) -> bool {
        self.skipped
    }
    fn is_success(&self) -> bool {
        self.success && !self.skipped
    }
    fn skip_reason(&self) -> Option<&str> {
        self.skip_reason.as_deref()
    }
    fn input_path(&self) -> &str {
        &self.input_path
    }
    fn output_path(&self) -> Option<&str> {
        self.output_path.as_deref()
    }
    fn input_size(&self) -> u64 {
        self.input_size
    }
    fn output_size(&self) -> Option<u64> {
        self.output_size
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn blake3(&self) -> Option<&str> {
        self.blake3.as_deref()
    }
}

pub struct CliRunnerConfig {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub recursive: bool,
    pub label: String,
    pub base_dir: Option<PathBuf>,
    pub resume: bool,
}

/// Resolve `base_dir` for video `run` command. Shared by `vid_hevc` and `vid_av1` to reduce duplication.
/// Returns: explicit override, or when recursive and input is a dir then input, else parent of input.
pub fn resolve_video_run_base_dir(
    input: &Path,
    recursive: bool,
    base_dir_override: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(explicit) = base_dir_override {
        return Some(explicit);
    }
    if recursive && input.is_dir() {
        Some(input.to_path_buf())
    } else {
        input.parent().map(std::path::Path::to_path_buf)
    }
}

/// Run a command-line tool with automatic processing features.
///
/// # Errors
/// Returns an error if command execution or file processing fails.
pub fn run_auto_command<F, R>(config: CliRunnerConfig, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R>,
    R: CliProcessingResult,
{
    if config.input.is_dir() {
        process_directory(&config, converter)
    } else {
        process_single_file(&config, converter)
    }
}

fn process_directory<F, R>(config: &CliRunnerConfig, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R>,
    R: CliProcessingResult,
{
    let input = &config.input;
    let recursive = config.recursive;

    // Check for Apple Photos library before processing
    if let Err(e) = crate::safety::check_apple_photos_library(input) {
        anyhow::bail!("{e}");
    }

    let files = crate::collect_video_files_for_perceived_speed(
        input,
        SUPPORTED_VIDEO_EXTENSIONS,
        recursive,
    );

    if files.is_empty() {
        anyhow::bail!(
            "No video files found in directory: {}\n\
             Supported video formats: {}\n\
             Use imgquality tool for images",
            input.display(),
            SUPPORTED_VIDEO_EXTENSIONS.join(", ")
        );
    }

    info!("Found {} video files to process", files.len());
    info!("Strategy: deeper paths -> lighter workload -> shorter duration -> smaller files -> lower resolution");

    // Reset global session stats to zero at the start of each directory processing run.
    // This ensures that progressive UI stats (X: 12v, etc.) reflect the current task.
    crate::progress_mode::reset_session_stats();

    // Pre-flight disk space check: require at least the total input size free on the output volume.
    // This catches "No space left on device" before encoding starts rather than mid-encode.
    // Skip if MFB_SKIP_DISK_PRECHECK=1 (script has already done the check).
    // Initialize checkpoint manager if resume is enabled
    let mut checkpoint = if config.resume {
        match crate::checkpoint::CheckpointManager::new_with_context(
            input,
            config.output.as_deref(),
        ) {
            Ok(cp) => {
                // Detect when user deleted the output directory to start fresh:
                // clear old checkpoint state so all files get reprocessed.
                if let Err(e) = cp.reset_if_output_root_missing(config.output.as_deref()) {
                    warn!("⚠️  Failed to check output root for checkpoint reset: {e}");
                }

                if cp.is_resume_mode() {
                    info!(
                        "📂 Resume: skipping {} already completed files",
                        cp.completed_count()
                    );
                } else {
                    crate::clear_processed_list();
                }
                Some(cp)
            }
            Err(e) => {
                warn!("⚠️  Could not initialize checkpoint manager: {e}");
                None
            }
        }
    } else {
        None
    };

    if std::env::var("MFB_SKIP_DISK_PRECHECK").as_deref() != Ok("1") {
        let total_input_size: u64 = files
            .iter()
            .map(|f| match crate::io_utils::metadata_with_retry(f) {
                Ok(metadata) => metadata.len(),
                Err(err) => {
                    warn!(
                        "Failed to read file metadata during disk-space precheck ({}): {}",
                        f.display(),
                        err
                    );
                    0
                }
            })
            .sum();
        let check_path = config.output.as_deref().unwrap_or(input);
        if let Some(avail) = crate::system_memory::get_available_disk_bytes(check_path) {
            // Reserve 1 GB headroom on top of total input size (temp files, partial encodes, etc.)
            let required = total_input_size.saturating_add(1024 * 1024 * 1024);
            if avail < required {
                let avail_gb = avail as f64 / (1024.0 * 1024.0 * 1024.0);
                let required_gb = required as f64 / (1024.0 * 1024.0 * 1024.0);
                anyhow::bail!(
                    "❌ Insufficient disk space on output volume.\n\
                     💾 Available: {avail_gb:.2} GB\n\
                     💾 Required:  {required_gb:.2} GB (input size + 1 GB headroom)\n\
                     💡 Free up space or choose a different output location."
                );
            }
            info!(
                "💾 Disk space OK: {:.2} GB available, {:.2} GB required",
                avail as f64 / (1024.0 * 1024.0 * 1024.0),
                required as f64 / (1024.0 * 1024.0 * 1024.0)
            );
        }
    }

    let start_time = Instant::now();
    let mut batch_result = BatchResult::new();
    let mut total_input_bytes: u64 = 0;
    let mut total_output_bytes: u64 = 0;
    let pause_controller = BatchPauseController::new();
    let total_files = files.len();
    let progress_bar = crate::CoarseProgressBar::new(total_files as u64, "Running");
    let mut pending_files = files;
    let mut recent_success_ext: Option<String> = None;
    let mut recent_success_parent: Option<PathBuf> = None;

    // 📡 Optional: Initialize audit log directory
    let debug_dir = Path::new("debug");
    if debug_dir.exists() && debug_dir.is_dir() {
        let _ = std::fs::create_dir_all(debug_dir);
    }

    while !pending_files.is_empty() {
        if pause_controller.is_paused() {
            break;
        }

        let next_index = select_hot_start_file_index(
            &pending_files,
            recent_success_ext.as_deref(),
            recent_success_parent.as_deref(),
        );
        let file = pending_files.remove(next_index);
        progress_bar.set_message(&file.file_name().unwrap_or_default().to_string_lossy());

        // Fix extension by content first; after fix, only treat as video if extension still in list (avoids disguised-extension panic).
        let fixed = match fix_extension_if_mismatch(&file) {
            Ok(p) => p,
            Err(e) => {
                error!("❌ Extension fix failed for {}: {}", file.display(), e);
                if let Some(reason) = disk_full_pause_reason(&e.to_string()) {
                    if pause_controller.request_pause(&file, reason.clone()) {
                        warn!("⏸️ Batch paused at {}: {}", file.display(), reason);
                    }
                    batch_result.pause(file.clone(), reason, pending_files.len().saturating_add(1));
                    break;
                }
                batch_result.fail(file.clone(), e.to_string());
                progress_bar.set(batch_result.total as u64);
                continue;
            }
        };
        if !has_extension(&fixed, SUPPORTED_VIDEO_EXTENSIONS) {
            if let Some(ref out) = config.output {
                if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                    &fixed,
                    Some(out),
                    config.base_dir.as_deref(),
                    true,
                ) {
                    error!("❌ Failed to copy {}: {}", fixed.display(), copy_err);
                } else {
                    info!(
                        "📋 Copied (content not video after fix): {}",
                        fixed.display()
                    );
                }
            }
            batch_result.skip();
            progress_bar.set(batch_result.total as u64);
            continue;
        }

        // Skip if already processed
        if let Some(ref cp) = checkpoint {
            if cp.is_completed(&fixed) {
                if crate::progress_mode::is_verbose_mode() {
                    info!(
                        "   SKIP: {} (Already recorded as completed in checkpoint)",
                        fixed.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
                batch_result.skip();
                progress_bar.set(batch_result.total as u64);
                continue;
            }
        }

        match converter(fixed.as_path()) {
            Ok(result) => {
                if result.is_skipped() {
                    info!(
                        "⏭️ {} → SKIP ({})",
                        fixed.file_name().unwrap_or_default().to_string_lossy(),
                        result.skip_reason().unwrap_or("unknown")
                    );
                    batch_result.skip();
                } else if result.is_success() {
                    info!(
                        "{} → {} ({}) ✅",
                        fixed.file_name().unwrap_or_default().to_string_lossy(),
                        result.output_path().unwrap_or("?"),
                        result.message()
                    );
                    batch_result.success();
                    crate::progress_mode::video_processed_success();
                    total_input_bytes += result.input_size();
                    total_output_bytes += result.output_size().unwrap_or(result.input_size());
                    recent_success_ext = extension_lower(&fixed);
                    recent_success_parent = fixed.parent().map(Path::to_path_buf);

                    // 📡 Audit: Log live decision to JSONL if debug/ exists
                    log_live_audit_to_jsonl(
                        result.blake3(),
                        &config.label,
                        result.output_path().unwrap_or("original"),
                        result.message(),
                    );

                    // Mark as completed
                    if let Some(ref mut cp) = checkpoint {
                        if let Err(err) = cp.mark_completed(&fixed) {
                            warn!(
                                "⚠️ Failed to mark checkpoint complete for {}: {}",
                                fixed.display(),
                                err
                            );
                        }
                    }
                } else {
                    if let Some(reason) = disk_full_pause_reason(result.message()) {
                        if pause_controller.request_pause(&fixed, reason.clone()) {
                            warn!("⏸️ Batch paused at {}: {}", fixed.display(), reason);
                        }
                        batch_result.pause(
                            fixed.clone(),
                            reason,
                            pending_files.len().saturating_add(1),
                        );
                        break;
                    }
                    info!(
                        "{} → FAILED ({}) ❌",
                        fixed.file_name().unwrap_or_default().to_string_lossy(),
                        result.message()
                    );
                    batch_result.fail(fixed.clone(), result.message().to_string());
                    crate::progress_mode::video_processed_failure();
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                let maybe_ue = e.downcast_ref::<crate::unified_error::UnifiedError>();
                let is_skip = maybe_ue.is_some_and(super::unified_error::UnifiedError::is_skip);
                let category = maybe_ue
                    .map_or(crate::unified_error::ErrorCategory::Recoverable, |ue| {
                        ue.category()
                    });

                if is_skip {
                    info!(
                        "⏭️ {} → SKIP ({})",
                        fixed.file_name().unwrap_or_default().to_string_lossy(),
                        error_msg
                    );
                    batch_result.skip();
                } else if let Some(reason) = disk_full_pause_reason(&error_msg) {
                    if pause_controller.request_pause(&fixed, reason.clone()) {
                        warn!("⏸️ Batch paused at {}: {}", fixed.display(), reason);
                    }
                    batch_result.pause(
                        fixed.clone(),
                        reason,
                        pending_files.len().saturating_add(1),
                    );
                    break;
                } else {
                    error!(
                        "❌ {} failed: {}",
                        fixed.file_name().unwrap_or_default().to_string_lossy(),
                        e
                    );
                    batch_result.fail(fixed.clone(), error_msg);

                    if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                        &fixed,
                        config.output.as_deref(),
                        config.base_dir.as_deref(),
                        true,
                    ) {
                        error!("❌ Failed to copy original: {copy_err}");
                    } else {
                        info!(
                            "📋 Copied original (conversion failed): {}",
                            fixed.display()
                        );
                    }

                    if category == crate::unified_error::ErrorCategory::Fatal {
                        error!("🛑 Fatal error encountered, stopping batch processing.");
                        break;
                    }

                    crate::progress_mode::video_processed_failure();
                }
            }
        }

        progress_bar.set(batch_result.total as u64);
    }

    if batch_result.paused {
        progress_bar.finish_and_clear();
    } else {
        progress_bar.finish();
    }

    // Cleanup checkpoint only on 100% success
    if let Some(cp) = checkpoint {
        if batch_result.paused {
            if let Err(err) = cp.release_lock() {
                warn!("⚠️ Failed to release checkpoint lock after pause: {err}");
            }
        } else if batch_result.failed == 0 {
            if let Err(err) = cp.cleanup() {
                warn!("⚠️ Failed to clean up checkpoint state: {err}");
            }
        } else if let Err(err) = cp.release_lock() {
            warn!("⚠️ Failed to release checkpoint lock after failure: {err}");
        }
    }

    print_summary_report(
        &batch_result,
        start_time.elapsed(),
        total_input_bytes,
        total_output_bytes,
        &config.label,
    );

    if batch_result.paused {
        return Ok(());
    }

    if let Some(ref output_dir) = config.output {
        info!("\n📦 Copying unsupported files...");
        let copy_result = copy_unsupported_files(input, output_dir, recursive);
        if copy_result.copied > 0 {
            info!("📦 Copied {} unsupported files", copy_result.copied);
        }
        if copy_result.failed > 0 {
            error!("❌ Failed to copy {} files", copy_result.failed);
        }

        info!("\n🔍 Verifying output completeness...");
        let verify = verify_output_completeness(input, output_dir, recursive);
        info!("{}", verify.message);
        if !verify.passed {
            warn!("⚠️  Some files may be missing from output!");
        }

        if let Some(ref base_dir) = config.base_dir {
            info!("\n📁 Preserving directory metadata...");
            if let Err(e) = crate::metadata::preserve_directory_metadata(base_dir, output_dir) {
                error!("⚠️ Failed to preserve directory metadata: {e}");
            } else {
                info!("✅ Directory metadata preserved");
            }
        }
    }

    Ok(())
}

fn extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

fn select_hot_start_file_index(
    pending_files: &[PathBuf],
    recent_success_ext: Option<&str>,
    recent_success_parent: Option<&Path>,
) -> usize {
    if pending_files.is_empty() {
        return 0;
    }

    let window = pending_files.len().min(48);
    let mut best_index = 0usize;
    let mut best_score = (i32::MIN, i32::MIN);

    for (index, path) in pending_files.iter().take(window).enumerate() {
        let ext_match = recent_success_ext
            .and_then(|ext| extension_lower(path).map(|current| current == ext))
            .unwrap_or(false);
        let parent_match =
            recent_success_parent.is_some_and(|parent| path.parent() == Some(parent));

        let hot_start_score = i32::from(ext_match) * 4 + i32::from(parent_match) * 2;
        let proximity_score = crate::numeric_cast::usize_to_i32_sat(window - index);
        let score = (hot_start_score, proximity_score);

        if score > best_score {
            best_score = score;
            best_index = index;
        }
    }

    best_index
}

fn process_single_file<F, R>(config: &CliRunnerConfig, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R>,
    R: CliProcessingResult,
{
    // Check for Apple Photos library before processing
    if let Err(e) = crate::safety::check_apple_photos_library(&config.input) {
        anyhow::bail!("{e}");
    }

    // Fix extension by content first so all downstream checks see the real format (avoids disguised-extension panic).
    let fixed_input = fix_extension_if_mismatch(&config.input)?;
    let input = fixed_input.as_path();

    if !has_extension(input, SUPPORTED_VIDEO_EXTENSIONS) {
        let ext_str = input
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("(none)");
        if let Some(ref out) = config.output {
            if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                input,
                Some(out),
                config.base_dir.as_deref(),
                true,
            ) {
                error!("❌ Failed to copy to output: {copy_err}");
            } else {
                info!(
                    "📋 Copied to output (not a video after content check): {}",
                    input.display()
                );
            }
        }
        anyhow::bail!(
            "❌ Not a video file: {}\n\
             💡 Extension (after content fix): .{}\n\
             💡 Supported video formats: {}\n\
             💡 Use imgquality tool for images",
            input.display(),
            ext_str,
            SUPPORTED_VIDEO_EXTENSIONS.join(", ")
        );
    }

    let result = match converter(input) {
        Ok(r) => r,
        Err(e) => {
            if let Some(ref output_dir) = config.output {
                if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                    input,
                    Some(output_dir),
                    config.base_dir.as_deref(),
                    true,
                ) {
                    error!("❌ Failed to copy original to output dir: {copy_err}");
                } else {
                    info!(
                        "📋 Copied original to output (conversion failed): {}",
                        input.display()
                    );
                }
            }
            return Err(e);
        }
    };

    info!("");
    info!("📊 Conversion Summary:");
    info!(
        "   Input:  {} ({} bytes)",
        result.input_path(),
        result.input_size()
    );
    if let Some(out_path) = result.output_path() {
        info!(
            "   Output: {} ({} bytes)",
            out_path,
            result.output_size().unwrap_or(0)
        );
    }
    info!("   Result: {}", result.message());

    // 📡 Audit: Log live decision to JSONL if debug/ exists
    log_live_audit_to_jsonl(
        result.blake3(),
        &config.label,
        result.output_path().unwrap_or("original"),
        result.message(),
    );

    Ok(())
}


#[derive(serde::Serialize)]
struct AuditRecord<'a> {
    blake3: &'a str,
    session_id: &'a str,
    actual_format: &'a str,
    actual_params_json: &'a str,
    audit_at: i64,
}

fn log_live_audit_to_jsonl(
    blake3: Option<&str>,
    session_id: &str,
    actual_format: &str,
    actual_params_json: &str,
) {
    let Some(hash) = blake3 else { return };
    let debug_dir = Path::new("debug");
    if !debug_dir.exists() {
        return;
    }

    let audit_file = debug_dir.join("live_audit.jsonl");
    let record = AuditRecord {
        blake3: hash,
        session_id,
        actual_format,
        actual_params_json,
        audit_at: chrono::Utc::now().timestamp(),
    };

    if let Ok(json) = serde_json::to_string(&record) {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(audit_file)
        {
            let _ = writeln!(file, "{}", json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_start_prefers_same_extension_within_front_window() {
        let pending = vec![
            PathBuf::from("a/clip-01.mov"),
            PathBuf::from("b/clip-02.mp4"),
            PathBuf::from("c/clip-03.mov"),
            PathBuf::from("d/clip-04.mp4"),
        ];

        let next = select_hot_start_file_index(&pending, Some("mp4"), None);
        assert_eq!(next, 1);
    }

    #[test]
    fn hot_start_prefers_same_parent_when_extension_ties() {
        let pending = vec![
            PathBuf::from("alpha/one.mov"),
            PathBuf::from("beta/two.mov"),
            PathBuf::from("alpha/three.mp4"),
        ];

        let next = select_hot_start_file_index(&pending, Some("mov"), Some(Path::new("beta")));
        assert_eq!(next, 1);
    }
}

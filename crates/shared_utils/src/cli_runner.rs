use crate::batch::{PauseController, Summary, disk_full_pause_reason};
use crate::common_utils::has_extension;
use crate::file_copier::{
    SUPPORTED_VIDEO_EXTENSIONS, copy_unsupported_files, verify_output_completeness,
};
use crate::report::print_summary;
use crate::smart_file_copier::fix_extension_if_mismatch;
use anyhow::Result;
use log::{error, info, warn};
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub trait CliProcessingResult {
    fn is_skipped(&self) -> bool;
    fn is_success(&self) -> bool;
    /// True when the module is intentionally abdicating this file to the other
    /// module (e.g. vid ignoring static images handled by img). Unlike
    /// `is_skipped`, ignored results MUST NOT be copied to the output tree —
    /// the peer module will produce the file.
    fn is_ignored(&self) -> bool {
        false
    }
    fn skip_reason(&self) -> Option<&str>;
    fn input_path(&self) -> &str;
    fn output_path(&self) -> Option<&str>;
    fn input_size(&self) -> u64;
    fn output_size(&self) -> Option<u64>;
    fn message(&self) -> &str;
    fn blake3(&self) -> Option<&str>;
}

impl CliProcessingResult for crate::conversion::TaskResult {
    fn is_skipped(&self) -> bool {
        matches!(
            self.outcome(),
            crate::conversion::Outcome::Skipped | crate::conversion::Outcome::FallbackPreserved
        )
    }
    fn is_ignored(&self) -> bool {
        self.outcome() == crate::conversion::Outcome::Ignored
    }
    fn is_success(&self) -> bool {
        self.outcome() == crate::conversion::Outcome::Converted
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

pub struct Config {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub recursive: bool,
    pub label: String,
    pub base_dir: Option<PathBuf>,
    pub resume: bool,
    pub protect_destructive_dirs: bool,
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
pub fn run_auto_command<F, R>(config: &Config, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R> + Sync,
    R: CliProcessingResult,
{
    if config.input.is_dir() {
        process_directory(config, converter)
    } else {
        process_single_file(config, converter)
    }
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(
    clippy::too_many_lines,
    reason = "Complex orchestration logic where fragmenting state into smaller helpers would decrease readability and increase cognitive overhead."
)]
fn process_directory<F, R>(config: &Config, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R> + Sync,
    R: CliProcessingResult,
{
    let input = &config.input;
    let recursive = config.recursive;

    // Check for Apple Photos library before processing
    if let Err(e) = crate::safety::check_apple_photos_library(input) {
        anyhow::bail!("{e}");
    }
    if config.protect_destructive_dirs
        && let Err(e) = crate::safety::check_dangerous_directory(input)
    {
        anyhow::bail!("{e}");
    }

    if let Some(ref out_dir) = config.output {
        if let Err(e) = crate::safety::check_apple_photos_library(out_dir) {
            anyhow::bail!("{e}");
        }
        if config.protect_destructive_dirs
            && let Err(e) = crate::safety::check_dangerous_directory(out_dir)
        {
            anyhow::bail!("{e}");
        }
    }

    let files = crate::collect_video_files_for_perceived_speed(
        input,
        SUPPORTED_VIDEO_EXTENSIONS,
        recursive,
    );

    if files.is_empty() {
        warn!(
            "No video files found in directory: {}\n\
             Supported video formats: {}\n\
             Use img for images",
            input.display(),
            SUPPORTED_VIDEO_EXTENSIONS.join(", ")
        );
        return Ok(());
    }

    info!("Found {} video files to process", files.len());
    info!(
        "Strategy: deeper paths -> lighter workload -> shorter duration -> smaller files -> lower resolution"
    );

    // Reset global session stats to zero at the start of each directory processing run.
    // This ensures that progressive UI stats (X: 12v, etc.) reflect the current task.
    crate::progress_mode::reset_session_stats();

    // Pre-flight disk space check: require at least the total input size free on the output volume.
    // This catches "No space left on device" before encoding starts rather than mid-encode.
    // Skip if MFB_SKIP_DISK_PRECHECK=1 (script has already done the check).
    // Initialize checkpoint manager if resume is enabled
    let checkpoint = if config.resume {
        match crate::checkpoint::Manager::new_with_context(input, config.output.as_deref()) {
            Ok(cp) => {
                // Detect when user deleted the output directory to start fresh:
                // clear old checkpoint state so all files get reprocessed.
                if let Err(e) = cp.reset_if_output_root_missing(config.output.as_deref()) {
                    warn!(
                        "Checkpoint: Output directory missing; failed to reset session state: {e}"
                    );
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
                warn!("Checkpoint: Initialization failed (resume mode disabled): {e}");
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
                        "Disk Check: Failed to retrieve metadata for {}; skipping in total size estimation: {}",
                        f.display(),
                        err
                    );
                    0
                }
            })
            .sum();
        let check_path = config.output.as_deref().unwrap_or(input);
        if let Some(avail) = crate::system_memory::get_available_disk_bytes(check_path) {
            // Reserve safety headroom on top of total input size (temp files, partial encodes, etc.)
            let required =
                total_input_size.saturating_add(crate::constants::DISK_SAFETY_HEADROOM_BYTES);
            if avail < required {
                let avail_gb = crate::numeric_cast::u64_to_f64(avail)
                    / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64);
                let required_gb = crate::numeric_cast::u64_to_f64(required)
                    / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64);
                anyhow::bail!(
                    "❌ Insufficient disk space on output volume.\n\
                     💾 Available: {avail_gb:.2} GB\n\
                     💾 Required:  {required_gb:.2} GB (estimated input size + 1 GB safety headroom)\n\
                     💡 Free up space or specify a different output volume via --output."
                );
            }
            info!(
                "💾 Disk space OK: {:.2} GB available, {:.2} GB required",
                crate::numeric_cast::u64_to_f64(avail) / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64),
                crate::numeric_cast::u64_to_f64(required)
                    / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64)
            );
        }
    }
    let checkpoint = checkpoint.map(Arc::new);

    let start_time = Instant::now();
    let total_files = files.len();
    let pause_controller = Arc::new(PauseController::new());
    let fatal_stop = AtomicBool::new(false);
    let progress_bar = Arc::new(crate::CoarseProgressBar::new(
        crate::numeric_cast::usize_to_u64(total_files),
        "Running",
    ));
    let thread_config = crate::thread_manager::get_balanced_thread_config(
        crate::thread_manager::WorkloadType::Video,
    );
    let parallel_tasks = thread_config.parallel_tasks.max(1);
    let succeeded = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let total_input_bytes = AtomicU64::new(0);
    let total_output_bytes = AtomicU64::new(0);
    let errors: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

    // 📡 Optional: Initialize audit log directory
    let debug_dir = Path::new("debug");
    if debug_dir.exists() && debug_dir.is_dir() {
        let _ = std::fs::create_dir_all(debug_dir);
    }

    info!(
        "🔧 Thread Strategy: {} parallel tasks x {} threads/task (CPU cores: {})",
        parallel_tasks,
        thread_config.child_threads,
        std::thread::available_parallelism().map_or(4, std::num::NonZero::get)
    );
    if let Some(hint) = crate::thread_manager::memory_cap_hint() {
        info!("   💡 {hint}");
    }

    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(parallel_tasks)
        .build()
    {
        Ok(pool) => pool,
        Err(err) => {
            warn!(
                "Thread Manager: Failed to initialize {parallel_tasks}-thread video pool: {err}. Falling back to sequential execution (1 thread)."
            );
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .map_err(|fallback_err| {
                    anyhow::anyhow!("Thread Manager: Critical failure creating fallback thread pool: {fallback_err}")
                })?
        }
    };

    pool.install(|| {
        files.par_iter().for_each(|file| {
            if pause_controller.is_paused() || fatal_stop.load(Ordering::Relaxed) {
                return;
            }

            let display_name = file.file_name().unwrap_or_default().to_string_lossy();
            progress_bar.set_message(&display_name);

            // Fix extension by content first; after fix, only treat as video if extension still
            // matches. When writing to a separate output tree, keep the source immutable.
            let fixed = match if config.output.is_some() {
                crate::smart_file_copier::check_extension_mismatch_readonly(file)
            } else {
                fix_extension_if_mismatch(file)
            } {
                Ok(path) => path,
                Err(err) => {
                    error!("❌ Extension fix failed for {}: {}", file.display(), err);
                    if let Some(reason) = disk_full_pause_reason(&err.to_string()) {
                        if pause_controller.request_pause(file, reason.clone()) {
                            warn!("⏸️ Batch paused at {}: {}", file.display(), reason);
                        }
                        return;
                    }
                    failed.fetch_add(1, Ordering::Relaxed);
                    errors
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((file.clone(), err.to_string()));
                    let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    progress_bar.set(crate::numeric_cast::usize_to_u64(current));
                    return;
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
                skipped.fetch_add(1, Ordering::Relaxed);
                let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                progress_bar.set(crate::numeric_cast::usize_to_u64(current));
                return;
            }

            if let Some(cp) = checkpoint.as_ref()
                && cp.is_completed(&fixed)
            {
                if crate::progress_mode::is_verbose_mode() {
                    info!(
                        "   SKIP: {} (Already recorded as completed in checkpoint)",
                        fixed.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
                skipped.fetch_add(1, Ordering::Relaxed);
                let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                crate::progress_mode::write_progress_line_to_run_log(
                    start_time.elapsed().as_secs(),
                    crate::numeric_cast::usize_to_u64(current),
                    crate::numeric_cast::usize_to_u64(total_files),
                    &fixed.file_name().unwrap_or_default().to_string_lossy(),
                );
                progress_bar.set(crate::numeric_cast::usize_to_u64(current));
                return;
            }

            match converter(fixed.as_path()) {
                Ok(result) => {
                    if result.is_ignored() {
                        // Peer module (img/vid) owns this file. Do NOT copy here —
                        // copying would race the peer and produce an ambiguous
                        // collision in the output tree.
                        info!(
                            "⏭️ {} → IGNORE ({})",
                            fixed.file_name().unwrap_or_default().to_string_lossy(),
                            result.skip_reason().unwrap_or("handled by peer module")
                        );
                        skipped.fetch_add(1, Ordering::Relaxed);
                    } else if result.is_skipped() {
                        info!(
                            "⏭️ {} → SKIP ({})",
                            fixed.file_name().unwrap_or_default().to_string_lossy(),
                            result.skip_reason().unwrap_or("unknown")
                        );
                        skipped.fetch_add(1, Ordering::Relaxed);

                        // Copy original file to output directory for skips to ensure a complete output set.
                        if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                            &fixed,
                            config.output.as_deref(),
                            config.base_dir.as_deref(),
                            true,
                        ) {
                            error!("❌ Failed to copy skipped file {}: {}", fixed.display(), copy_err);
                        }
                    } else if result.is_success() {
                        info!(
                            "{} → {} ({}) ✅",
                            fixed.file_name().unwrap_or_default().to_string_lossy(),
                            result.output_path().unwrap_or("?"),
                            result.message()
                        );
                        succeeded.fetch_add(1, Ordering::Relaxed);
                        crate::progress_mode::video_processed_success();
                        total_input_bytes.fetch_add(result.input_size(), Ordering::Relaxed);
                        total_output_bytes.fetch_add(
                            result.output_size().unwrap_or_else(|| result.input_size()),
                            Ordering::Relaxed,
                        );

                        log_live_audit_to_jsonl(
                            result.blake3(),
                            &config.label,
                            result.output_path().unwrap_or("original"),
                            result.message(),
                        );

                        if let Some(cp) = checkpoint.as_ref()
                            && let Err(err) = cp.mark_completed(&fixed)
                        {
                            warn!(
                                "Checkpoint: Integrity failure marking file as completed (path={}): {}",
                                fixed.display(),
                                err
                            );
                        }
                    } else {
                        if let Some(reason) = disk_full_pause_reason(result.message()) {
                            if pause_controller.request_pause(&fixed, reason.clone()) {
                                warn!("⏸️ Batch paused at {}: {}", fixed.display(), reason);
                            }
                            return;
                        }
                        info!(
                            "{} → FAILED ({}) ❌",
                            fixed.file_name().unwrap_or_default().to_string_lossy(),
                            result.message()
                        );
                        failed.fetch_add(1, Ordering::Relaxed);
                        errors
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push((fixed.clone(), result.message().to_string()));
                        crate::progress_mode::video_processed_failure();
                    }
                }
                Err(err) => {
                    let error_msg = err.to_string();
                    let maybe_ue = err.downcast_ref::<crate::unified_error::UnifiedError>();

                    // Fallback: search the error chain if direct downcast fails (handles multiple wrapping layers)
                    let ue_from_chain = if maybe_ue.is_none() {
                        err.chain().find_map(|e| e.downcast_ref::<crate::unified_error::UnifiedError>())
                    } else {
                        maybe_ue
                    };

                    let is_skip = ue_from_chain.is_some_and(crate::unified_error::UnifiedError::is_skip)
                        || error_msg.contains("Iteration limit exceeded")
                        || error_msg.contains("Optimization target not met")
                        || error_msg.contains("Quality validation failed")
                        || error_msg.contains("Compression failed")
                        || error_msg.contains("already exists");

                    let should_copy = ue_from_chain.is_some_and(crate::unified_error::UnifiedError::should_copy_original)
                        || (is_skip && !error_msg.contains("already exists")); // Don't copy if it already exists in output

                    let category = ue_from_chain.map_or_else(
                        || {
                            if is_skip {
                                crate::unified_error::ErrorCategory::Optional
                            } else {
                                crate::unified_error::ErrorCategory::Recoverable
                            }
                        },
                        crate::unified_error::UnifiedError::category,
                    );

                    if is_skip {
                        info!(
                            "⏭️ {} → SKIP ({})",
                            fixed.file_name().unwrap_or_default().to_string_lossy(),
                            error_msg
                        );
                        skipped.fetch_add(1, Ordering::Relaxed);

                        if should_copy {
                            // Copy original file to output directory for skip errors to ensure a complete output set.
                            if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                                &fixed,
                                config.output.as_deref(),
                                config.base_dir.as_deref(),
                                true,
                            ) {
                                error!("❌ Failed to copy skipped file {}: {}", fixed.display(), copy_err);
                            }
                        }
                    } else if let Some(reason) = disk_full_pause_reason(&error_msg) {
                        if pause_controller.request_pause(&fixed, reason.clone()) {
                            warn!("⏸️ Batch paused at {}: {}", fixed.display(), reason);
                        }
                        return;
                    } else {
                        error!(
                            "❌ {} failed: {}",
                            fixed.file_name().unwrap_or_default().to_string_lossy(),
                            err
                        );
                        failed.fetch_add(1, Ordering::Relaxed);
                        errors
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push((fixed.clone(), error_msg));

                        if category == crate::unified_error::ErrorCategory::Fatal {
                            fatal_stop.store(true, Ordering::SeqCst);
                            error!("🛑 Fatal error encountered, stopping batch processing.");
                        }

                        crate::progress_mode::video_processed_failure();
                    }
                }
            }

            let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
            crate::progress_mode::write_progress_line_to_run_log(
                start_time.elapsed().as_secs(),
                crate::numeric_cast::usize_to_u64(current),
                crate::numeric_cast::usize_to_u64(total_files),
                &fixed.file_name().unwrap_or_default().to_string_lossy(),
            );
            progress_bar.set(crate::numeric_cast::usize_to_u64(current));
        });
    });

    let mut batch_result = Summary::new();
    batch_result.succeeded = succeeded.load(Ordering::Relaxed);
    batch_result.failed = failed.load(Ordering::Relaxed);
    batch_result.skipped = skipped.load(Ordering::Relaxed);
    batch_result.total = processed.load(Ordering::Relaxed);
    batch_result.errors = errors
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some(pause) = pause_controller.pause_info() {
        batch_result.pause(
            pause.path,
            pause.reason,
            total_files.saturating_sub(batch_result.total),
        );
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
                warn!("Checkpoint: Failed to release file lock during batch pause: {err}");
            }
        } else if batch_result.failed == 0 {
            if let Err(err) = cp.cleanup() {
                warn!(
                    "Checkpoint: 100% success reached, but failed to purge completed-list state: {err}"
                );
            }
        } else if let Err(err) = cp.release_lock() {
            warn!("Checkpoint: Failed to release file lock after batch failure: {err}");
        }
    }

    print_summary(
        &batch_result,
        start_time.elapsed(),
        total_input_bytes.load(Ordering::Relaxed),
        total_output_bytes.load(Ordering::Relaxed),
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
            error!(
                "IO Error: Failed to copy {} files to output directory",
                copy_result.failed
            );
        }

        info!("\n🔍 Verifying output completeness...");
        let verify = verify_output_completeness(input, output_dir, recursive);
        info!("{}", verify.message);
        if !verify.passed {
            warn!(
                "Verification: File count mismatch between input and output directories; some files may have been lost"
            );
        }

        if let Some(ref base_dir) = config.base_dir {
            info!("\n📁 Preserving directory metadata...");
            if let Err(e) = crate::metadata::preserve_directory(base_dir, output_dir) {
                error!("Metadata: Failed to sync directory timestamps/permissions: {e}");
            } else {
                info!("✅ Directory metadata preserved");
            }
        }
    }

    if fatal_stop.load(Ordering::Relaxed) {
        anyhow::bail!("Fatal error encountered during batch processing.");
    }

    Ok(())
}
fn process_single_file<F, R>(config: &Config, converter: F) -> Result<()>
where
    F: Fn(&Path) -> Result<R> + Sync,
    R: CliProcessingResult,
{
    // Check for Apple Photos library before processing
    if let Err(e) = crate::safety::check_apple_photos_library(&config.input) {
        anyhow::bail!("{e}");
    }

    if let Some(ref out_dir) = config.output
        && let Err(e) = crate::safety::check_apple_photos_library(out_dir)
    {
        anyhow::bail!("{e}");
    }

    // Fix extension by content first so all downstream checks see the real format (avoids disguised-extension panic).
    // When an output directory is configured the source tree must remain immutable:
    // use the readonly variant that logs mismatches without renaming source files.
    let fixed_input = if config.output.is_some() {
        crate::smart_file_copier::check_extension_mismatch_readonly(&config.input)?
    } else {
        fix_extension_if_mismatch(&config.input)?
    };
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
                error!("IO Error: Failed to copy non-video file to output directory: {copy_err}");
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
             💡 Use img for images",
            input.display(),
            ext_str,
            SUPPORTED_VIDEO_EXTENSIONS.join(", ")
        );
    }

    let result = match converter(input) {
        Ok(r) => r,
        Err(e) => {
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
            result
                .output_size()
                .expect("Failed to parse integer or missing required value")
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
            let _ = writeln!(file, "{json}");
        }
    }
}

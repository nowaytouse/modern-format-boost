use crate::batch::{PauseController, Summary, disk_full_pause_reason};
use crate::common_utils::has_extension;
use crate::file_copier::{
    SUPPORTED_VIDEO_EXTENSIONS, VerifyDomain, copy_unsupported_files,
    verify_output_completeness_for_domain,
};
use crate::report::print_summary;
use crate::smart_file_copier::fix_extension_if_mismatch;
use anyhow::{Context, Result};

use rayon::prelude::*;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub trait CliProcessingResult {
    fn is_skipped(&self) -> bool;
    fn is_success(&self) -> bool;
    /// True when the module is intentionally ignoring a file outside its
    /// processing domain. Unlike `is_skipped`, ignored results MUST NOT be
    /// copied to the output tree.
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
        self.outcome() == crate::conversion::Outcome::Skipped
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
    pub error_mode: crate::BatchErrorMode,
}

/// Resolve `base_dir` for video `run` command. Shared by `vid_hevc` and
/// `vid_av1` to reduce duplication. Returns: explicit override, or when
/// recursive and input is a dir then input, else parent of input.
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

// Rationale: This function handles complex, sequential initialization or
// business logic where further fragmentation would hinder readability and
// maintainability.
#[allow(clippy::too_many_lines)]
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
        crate::media_conversion_gate::delivery_pipeline_path_audit(
            "delivery_pipeline_batch",
            input,
            format!(
                "No video files found in directory: {}\nSupported video formats: {}\nUse img for \
                 images",
                input.display(),
                SUPPORTED_VIDEO_EXTENSIONS.join(", ")
            ),
        );
        return Ok(());
    }

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_BATCH,
        &format!("Found {} video files to process", files.len())
    );
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_STRATEGY,
        crate::infra::static_logs::messages::MSG_STRATEGY_DESCRIPTION
    );

    // Reset global session stats to zero at the start of each directory processing
    // run. This ensures that progressive UI stats (X: 12v, etc.) reflect the
    // current task.
    crate::progress_mode::reset_session_stats();

    // Pre-flight disk space check: require at least the total input size free on
    // the output volume. This catches "No space left on device" before encoding
    // starts rather than mid-encode. Skip if MFB_SKIP_DISK_PRECHECK=1 (script
    // has already done the check). Initialize checkpoint manager if resume is
    // enabled
    let checkpoint = if config.resume {
        let cp =
            crate::checkpoint::Manager::new_resuming_with_context(input, config.output.as_deref())
                .with_context(|| {
                    format!(
                        "failed to initialize resume checkpoint for {}",
                        input.display()
                    )
                })?;
        // Detect when user deleted the output directory to start fresh:
        // clear old checkpoint state so all files get reprocessed.
        if cp.is_resume_mode() {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_CHECKPOINT,
                &format!(
                    "{} Resume: skipping {} already completed files",
                    crate::media_conversion_gate::ui_icon_pick("📂", "[DIR]"),
                    cp.completed_count()
                )
            );
        } else {
            crate::clear_processed_list();
        }
        Some(cp)
    } else {
        None
    };

    if std::env::var(crate::constants::ENV_MFB_SKIP_DISK_PRECHECK).as_deref() != Ok("1") {
        let total_input_size: u64 = files
            .iter()
            .map(|f| match crate::io_utils::metadata_with_retry(f) {
                Ok(metadata) => metadata.len(),
                Err(err) => {
                    crate::media_conversion_gate::delivery_pipeline_path_audit(
                        "delivery_pipeline_cli",
                        f,
                        format!(
                            "Failed to retrieve metadata for {}; skipping in total size \
                             estimation: {}",
                            f.display(),
                            err
                        ),
                    );
                    0
                }
            })
            .sum();
        let check_path = crate::media_conversion_gate::delivery_disk_check_path_or_input(
            config.output.as_deref(),
            input,
            "cli_runner disk preflight",
        );
        if let Some(avail) = crate::system_memory::get_available_disk_bytes(check_path) {
            // Reserve safety headroom on top of total input size (temp files, partial
            // encodes, etc.)
            let required =
                total_input_size.saturating_add(crate::constants::DISK_SAFETY_HEADROOM_BYTES);
            if avail < required {
                let avail_gb = crate::numeric_cast::u64_to_f64(avail)
                    / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64);
                let required_gb = crate::numeric_cast::u64_to_f64(required)
                    / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64);
                anyhow::bail!(format!(
                    "{}\n   💾 Available: {avail_gb:.2} GB\n   💾 Required:  {required_gb:.2} GB \
                     (estimated input size + 1 GB safety headroom)\n   {} Free up space or \
                     specify a different output volume via --output.",
                    crate::media_conversion_gate::ui_user_facing_error(
                        "Insufficient disk space on output volume."
                    ),
                    crate::media_conversion_gate::ui_icon_pick("💡", "[HINT]")
                ));
            }
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_DISK,
                &format!(
                    "Disk space OK: {:.2} GB available, {:.2} GB required",
                    crate::numeric_cast::u64_to_f64(avail)
                        / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64),
                    crate::numeric_cast::u64_to_f64(required)
                        / (1_024.0_f64 * 1_024.0_f64 * 1_024.0_f64)
                )
            );
        }
    }
    let checkpoint = checkpoint.map(Arc::new);

    let start_time = Instant::now();
    let total_files = files.len();
    let pause_controller = Arc::new(PauseController::new());
    let progress_bar = Arc::new(crate::CoarseProgressBar::new(
        crate::numeric_cast::usize_to_u64(total_files),
        &config.label,
    ));
    let thread_config = crate::thread_manager::get_balanced_thread_config(
        crate::thread_manager::WorkloadType::Video,
    );
    let parallel_tasks = thread_config.parallel_tasks.max(1);
    let succeeded = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);
    let ignored = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let processed = AtomicUsize::new(0);
    let total_input_bytes = AtomicU64::new(0);
    let total_output_bytes = AtomicU64::new(0);
    let errors: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());
    let abort_requested = AtomicBool::new(false);
    let abort_reason: Mutex<Option<String>> = Mutex::new(None);

    // 📡 Optional: Initialize audit log directory
    let debug_dir = Path::new("debug");
    if debug_dir.exists() && debug_dir.is_dir() {
        crate::media_conversion_gate::delivery_create_dir_all_or_audit(
            "cli_runner_debug_dir",
            debug_dir,
        );
    }

    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_THREAD,
        &crate::infra::static_logs::messages::MSG_THREAD_STRATEGY
            .replacen("{}", &parallel_tasks.to_string(), 1)
            .replacen("{}", &thread_config.child_threads.to_string(), 1)
            .replacen(
                "{}",
                &crate::media_conversion_gate::runtime_available_parallelism_or_default(
                    "cli_runner::thread_strategy_log",
                )
                .to_string(),
                1
            )
    );
    {
        let tier = crate::performance_schedule::current_perf_tier();
        let mut governor_line = format!(
            "Performance governor: {} (gpu_slots={})",
            tier.as_str(),
            crate::media_conversion_gate::gpu_concurrency_max_or_default()
        );
        if let Some(requested) = crate::performance_schedule::perf_tier_from_env() {
            if requested == tier {
                let _ = write!(
                    governor_line,
                    " [override: {}]",
                    crate::constants::ENV_MFB_PERF_TIER
                );
            } else {
                let _ = write!(
                    governor_line,
                    " [requested {} via {}, stability applied]",
                    requested.as_str(),
                    crate::constants::ENV_MFB_PERF_TIER
                );
            }
        }
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_THREAD,
            &governor_line
        );
    }
    if let Some(hint) = crate::thread_manager::memory_cap_hint() {
        crate::log_info!(crate::infra::static_logs::messages::LABEL_THREAD, hint);
    }

    let pool = match rayon::ThreadPoolBuilder::new()
        .num_threads(parallel_tasks)
        .build()
    {
        Ok(pool) => pool,
        Err(err) => {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_batch",
                format!(
                    "Failed to initialize {parallel_tasks}-thread video pool: {err}. Falling back \
                     to sequential execution (1 thread)."
                ),
            );
            rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .map_err(|fallback_err| {
                    anyhow::anyhow!(
                        "Thread Manager: Critical failure creating fallback thread pool: \
                         {fallback_err}"
                    )
                })?
        }
    };

    let audit_pipeline = crate::infra::static_logs::audit_pipeline_from_label(&config.label);
    crate::infra::static_logs::log_batch_start_audit(audit_pipeline, &config.label, files.len());
    let request_abort = |file: &Path, reason: &str| {
        if !abort_requested.swap(true, Ordering::SeqCst) {
            *crate::media_conversion_gate::mutex_guard_or_recover(
                "cli_batch_abort_reason",
                abort_reason.lock(),
            ) = Some(format!("{}: {reason}", file.display()));
        }
    };

    pool.install(|| {
        files.par_iter().for_each(|file| {
            if pause_controller.is_paused() || abort_requested.load(Ordering::SeqCst) {
                return;
            }

            let display_name =
                std::borrow::Cow::Owned(crate::media_conversion_gate::path_file_name_for_log(file));

            let span = tracing::info_span!("video_processing", file = %file.display());
            let _enter = span.enter();

            progress_bar.set_message(&display_name);

            // Fix extension by content first; after fix, only treat as video if extension
            // still matches. When writing to a separate output tree, keep the
            // source immutable.
            let fixed = match if config.output.is_some() {
                crate::smart_file_copier::check_extension_mismatch_readonly(file)
            } else {
                fix_extension_if_mismatch(file)
            } {
                Ok(path) => path,
                Err(err) => {
                    crate::media_conversion_gate::delivery_pipeline_batch_audit(
                        "delivery_pipeline_cli",
                        format!("Extension fix failed for {}: {}", file.display(), err),
                    );
                    if let Some(reason) = disk_full_pause_reason(&err.to_string()) {
                        if pause_controller.request_pause(file, reason.clone()) {
                            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                                "delivery_pipeline_batch",
                                format!("Batch paused at {}: {}", file.display(), reason),
                            );
                        }
                        return;
                    }
                    let err_msg = err.to_string();
                    crate::infra::static_logs::log_file_outcome_audit(
                        audit_pipeline,
                        "failed",
                        file,
                        &err_msg,
                    );
                    failed.fetch_add(1, Ordering::Relaxed);
                    crate::media_conversion_gate::mutex_guard_or_recover(
                        "cli_batch_errors",
                        errors.lock(),
                    )
                    .push((file.clone(), err_msg));
                    crate::progress_mode::video_processed_failure();
                    if config.error_mode.should_abort_error(&err) {
                        request_abort(file, &err.to_string());
                    }
                    let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    progress_bar.set(crate::numeric_cast::usize_to_u64(current));
                    return;
                }
            };

            if !has_extension(&fixed, SUPPORTED_VIDEO_EXTENSIONS) {
                const IGNORE_REASON: &str = "outside video domain after content check";
                crate::progress_mode::video_ignored(
                    &fixed,
                    IGNORE_REASON,
                    Some(crate::infra::static_logs::audit_ignore_class::VID_OUT_OF_DOMAIN),
                );
                crate::log_info!(
                    crate::infra::static_logs::messages::LABEL_BATCH,
                    &format!(
                        "⏭️ {} → IGNORE ({})",
                        crate::media_conversion_gate::path_file_name_for_log(&fixed),
                        IGNORE_REASON
                    )
                );
                ignored.fetch_add(1, Ordering::Relaxed);
                let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                progress_bar.set(crate::numeric_cast::usize_to_u64(current));
                return;
            }

            if let Some(cp) = checkpoint.as_ref()
                && cp.is_completed(&fixed)
            {
                if crate::progress_mode::is_verbose_mode() {
                    crate::log_info!(
                        crate::infra::static_logs::messages::LABEL_CHECKPOINT,
                        &format!(
                            "SKIP: {} (Already recorded as completed in checkpoint)",
                            crate::media_conversion_gate::path_file_name_for_log(&fixed)
                        )
                    );
                }
                crate::infra::static_logs::log_file_outcome_audit(
                    audit_pipeline,
                    "skipped",
                    &fixed,
                    "checkpoint already completed",
                );
                skipped.fetch_add(1, Ordering::Relaxed);
                let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
                crate::progress_mode::write_progress_line_to_run_log(
                    start_time.elapsed().as_secs(),
                    crate::numeric_cast::usize_to_u64(current),
                    crate::numeric_cast::usize_to_u64(total_files),
                    &crate::media_conversion_gate::path_file_name_for_log(&fixed),
                );
                progress_bar.set(crate::numeric_cast::usize_to_u64(current));
                return;
            }

            match converter(fixed.as_path()) {
                Ok(result) => {
                    if result.is_ignored() {
                        // Domain ignore: do not copy here. Copying would create
                        // ambiguous output for a file this tool did not own.
                        let ignore_reason = crate::media_conversion_gate::pipeline_outcome_reason(
                            result.skip_reason(),
                            "outside this tool domain",
                            "cli_runner ignore",
                        );
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_BATCH,
                            &format!(
                                "⏭️ {} → IGNORE ({})",
                                crate::media_conversion_gate::path_file_name_for_log(&fixed),
                                ignore_reason.as_ref()
                            )
                        );
                        ignored.fetch_add(1, Ordering::Relaxed);
                        crate::progress_mode::video_ignored(
                            &fixed,
                            ignore_reason.as_ref(),
                            Some(crate::infra::static_logs::audit_ignore_class::VID_OUT_OF_DOMAIN),
                        );
                    } else if result.is_skipped() {
                        let skip_reason = crate::media_conversion_gate::pipeline_outcome_reason(
                            result.skip_reason(),
                            "unknown",
                            "cli_runner skip",
                        );
                        // Copy original file to output directory for skips to ensure a complete
                        // output set.
                        if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                            &fixed,
                            config.output.as_deref(),
                            config.base_dir.as_deref(),
                            true,
                        ) {
                            crate::infra::static_logs::log_file_outcome_audit(
                                audit_pipeline,
                                "failed",
                                &fixed,
                                &format!("failed to preserve skipped source: {copy_err}"),
                            );
                            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                                "delivery_pipeline_cli",
                                format!(
                                    "Failed to copy skipped file {}: {}",
                                    fixed.display(),
                                    copy_err
                                ),
                            );
                            failed.fetch_add(1, Ordering::Relaxed);
                            crate::media_conversion_gate::mutex_guard_or_recover(
                                "cli_batch_errors",
                                errors.lock(),
                            )
                            .push((
                                fixed.clone(),
                                format!("failed to preserve skipped source: {copy_err}"),
                            ));
                            crate::progress_mode::video_processed_failure();
                            request_abort(
                                &fixed,
                                &format!("failed to preserve skipped source: {copy_err}"),
                            );
                        } else {
                            crate::infra::static_logs::log_file_outcome_audit(
                                audit_pipeline,
                                "skipped",
                                &fixed,
                                skip_reason.as_ref(),
                            );
                            crate::log_info!(
                                crate::infra::static_logs::messages::LABEL_BATCH,
                                &format!(
                                    "⏭️ {} → SKIP ({})",
                                    crate::media_conversion_gate::path_file_name_for_log(&fixed),
                                    skip_reason.as_ref()
                                )
                            );
                            skipped.fetch_add(1, Ordering::Relaxed);
                        }
                    } else if result.is_success() {
                        let checkpoint_error = checkpoint
                            .as_ref()
                            .and_then(|cp| cp.mark_completed(&fixed).err());
                        if let Some(err) = checkpoint_error {
                            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                                "delivery_pipeline_batch",
                                format!(
                                    "Integrity failure marking file as completed (path={}): {}",
                                    fixed.display(),
                                    err
                                ),
                            );
                            failed.fetch_add(1, Ordering::Relaxed);
                            crate::media_conversion_gate::mutex_guard_or_recover(
                                "cli_batch_errors",
                                errors.lock(),
                            )
                            .push((
                                fixed.clone(),
                                format!("failed to mark checkpoint complete: {err}"),
                            ));
                            crate::progress_mode::video_processed_failure();
                            request_abort(
                                &fixed,
                                &format!("failed to mark checkpoint complete: {err}"),
                            );
                        } else {
                            crate::log_info!(
                                crate::infra::static_logs::messages::LABEL_BATCH,
                                &format!(
                                    "{} → {} ({}) ✅",
                                    crate::media_conversion_gate::path_file_name_for_log(&fixed),
                                    crate::media_conversion_gate::trace_label_or_default(
                                        result.output_path(),
                                        "?",
                                    ),
                                    result.message()
                                )
                            );
                            succeeded.fetch_add(1, Ordering::Relaxed);
                            crate::progress_mode::video_processed_success();
                            total_input_bytes.fetch_add(result.input_size(), Ordering::Relaxed);
                            total_output_bytes.fetch_add(
                                crate::media_conversion_gate::delivery_batch_output_bytes_or_input(
                                    result.output_size(),
                                    result.input_size(),
                                    "cli_runner batch success",
                                ),
                                Ordering::Relaxed,
                            );

                            log_live_audit_to_jsonl(
                                result.blake3(),
                                &config.label,
                                crate::media_conversion_gate::trace_label_or_default(
                                    result.output_path(),
                                    "original",
                                ),
                                result.message(),
                            );
                        }
                    } else {
                        if let Some(reason) = disk_full_pause_reason(result.message()) {
                            if pause_controller.request_pause(&fixed, reason.clone()) {
                                crate::media_conversion_gate::delivery_pipeline_batch_audit(
                                    "delivery_pipeline_batch",
                                    format!("Batch paused at {}: {}", fixed.display(), reason),
                                );
                            }
                            return;
                        }
                        crate::infra::static_logs::log_file_outcome_audit(
                            audit_pipeline,
                            "failed",
                            &fixed,
                            result.message(),
                        );
                        crate::log_info!(
                            crate::infra::static_logs::messages::LABEL_BATCH,
                            &format!(
                                "{} → FAILED ({})",
                                crate::media_conversion_gate::ui_user_facing_error(
                                    crate::media_conversion_gate::path_file_name_for_log(&fixed),
                                ),
                                result.message()
                            )
                        );
                        failed.fetch_add(1, Ordering::Relaxed);
                        crate::media_conversion_gate::mutex_guard_or_recover(
                            "cli_batch_errors",
                            errors.lock(),
                        )
                        .push((fixed.clone(), result.message().to_string()));
                        crate::progress_mode::video_processed_failure();
                        if config.error_mode.is_fail_fast() {
                            request_abort(&fixed, result.message());
                        }
                    }
                }
                Err(err) => {
                    let should_abort = config.error_mode.should_abort_error(&err);
                    let error_msg = err.to_string();
                    let maybe_ue = err.downcast_ref::<crate::unified_error::UnifiedError>();

                    // Fallback: search the error chain if direct downcast fails (handles multiple
                    // wrapping layers)
                    let ue_from_chain = if maybe_ue.is_none() {
                        err.chain()
                            .find_map(|e| e.downcast_ref::<crate::unified_error::UnifiedError>())
                    } else {
                        maybe_ue
                    };

                    let is_skip =
                        ue_from_chain.is_some_and(crate::unified_error::UnifiedError::is_skip);

                    let should_copy = ue_from_chain
                        .is_some_and(crate::unified_error::UnifiedError::should_copy_original)
                        && !matches!(
                            ue_from_chain,
                            Some(crate::unified_error::UnifiedError::OutputExists { .. })
                        );

                    if is_skip {
                        if should_copy {
                            // Copy original file to output directory for skip errors to ensure a
                            // complete output set.
                            if let Err(copy_err) = crate::smart_file_copier::copy_on_skip_or_fail(
                                &fixed,
                                config.output.as_deref(),
                                config.base_dir.as_deref(),
                                true,
                            ) {
                                crate::infra::static_logs::log_file_outcome_audit(
                                    audit_pipeline,
                                    "failed",
                                    &fixed,
                                    &format!("failed to preserve skipped source: {copy_err}"),
                                );
                                crate::media_conversion_gate::delivery_pipeline_batch_audit(
                                    "delivery_pipeline_cli",
                                    format!(
                                        "Failed to copy skipped file {}: {}",
                                        fixed.display(),
                                        copy_err
                                    ),
                                );
                                failed.fetch_add(1, Ordering::Relaxed);
                                crate::media_conversion_gate::mutex_guard_or_recover(
                                    "cli_batch_errors",
                                    errors.lock(),
                                )
                                .push((
                                    fixed.clone(),
                                    format!("failed to preserve skipped source: {copy_err}"),
                                ));
                                crate::progress_mode::video_processed_failure();
                                request_abort(
                                    &fixed,
                                    &format!("failed to preserve skipped source: {copy_err}"),
                                );
                            } else {
                                crate::infra::static_logs::log_file_outcome_audit(
                                    audit_pipeline,
                                    "skipped",
                                    &fixed,
                                    &error_msg,
                                );
                                crate::log_info!(
                                    crate::infra::static_logs::messages::LABEL_BATCH,
                                    &format!(
                                        "⏭️ {} → SKIP ({})",
                                        crate::media_conversion_gate::path_file_name_for_log(
                                            &fixed
                                        ),
                                        error_msg
                                    )
                                );
                                skipped.fetch_add(1, Ordering::Relaxed);
                            }
                        } else {
                            crate::infra::static_logs::log_file_outcome_audit(
                                audit_pipeline,
                                "skipped",
                                &fixed,
                                &error_msg,
                            );
                            crate::log_info!(
                                crate::infra::static_logs::messages::LABEL_BATCH,
                                &format!(
                                    "⏭️ {} → SKIP ({})",
                                    crate::media_conversion_gate::path_file_name_for_log(&fixed),
                                    error_msg
                                )
                            );
                            skipped.fetch_add(1, Ordering::Relaxed);
                        }
                    } else if let Some(reason) = disk_full_pause_reason(&error_msg) {
                        if pause_controller.request_pause(&fixed, reason.clone()) {
                            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                                "delivery_pipeline_batch",
                                format!("Batch paused at {}: {}", fixed.display(), reason),
                            );
                        }
                        return;
                    } else {
                        crate::infra::static_logs::log_file_outcome_audit(
                            audit_pipeline,
                            "failed",
                            &fixed,
                            &error_msg,
                        );
                        crate::media_conversion_gate::delivery_pipeline_batch_audit(
                            "delivery_pipeline_cli",
                            format!(
                                "{} failed: {}",
                                crate::media_conversion_gate::path_file_name_for_log(&fixed),
                                err
                            ),
                        );
                        failed.fetch_add(1, Ordering::Relaxed);
                        crate::media_conversion_gate::mutex_guard_or_recover(
                            "cli_batch_errors",
                            errors.lock(),
                        )
                        .push((fixed.clone(), error_msg));

                        crate::progress_mode::video_processed_failure();
                        if should_abort {
                            request_abort(&fixed, &err.to_string());
                        }
                    }
                }
            }

            let current = processed.fetch_add(1, Ordering::Relaxed) + 1;
            crate::progress_mode::write_progress_line_to_run_log(
                start_time.elapsed().as_secs(),
                crate::numeric_cast::usize_to_u64(current),
                crate::numeric_cast::usize_to_u64(total_files),
                &crate::media_conversion_gate::path_file_name_for_log(&fixed),
            );
            progress_bar.set(crate::numeric_cast::usize_to_u64(current));
        });
    });

    let mut batch_result = Summary::new();
    batch_result.succeeded = succeeded.load(Ordering::Relaxed);
    batch_result.failed = failed.load(Ordering::Relaxed);
    batch_result.skipped = skipped.load(Ordering::Relaxed);
    batch_result.ignored = ignored.load(Ordering::Relaxed);
    batch_result.total = processed.load(Ordering::Relaxed);
    batch_result.errors =
        crate::media_conversion_gate::mutex_into_inner_or_recover("cli_batch_errors", errors);

    if let Some(pause) = pause_controller.pause_info() {
        batch_result.pause(
            pause.path,
            pause.reason,
            total_files.saturating_sub(batch_result.total),
        );
    }
    let abort_reason = crate::media_conversion_gate::mutex_guard_or_recover(
        "cli_batch_abort_reason",
        abort_reason.lock(),
    )
    .clone();

    if batch_result.paused {
        progress_bar.finish_and_clear();
    } else {
        progress_bar.finish();
    }

    // Cleanup checkpoint only on 100% success.
    let checkpoint_error = if let Some(cp) = checkpoint {
        if batch_result.paused {
            if let Err(err) = cp.release_lock() {
                crate::media_conversion_gate::delivery_pipeline_batch_audit(
                    "delivery_pipeline_batch",
                    format!("Failed to release file lock during batch pause: {err}"),
                );
                Some(format!(
                    "failed to release checkpoint lock during batch pause: {err}"
                ))
            } else {
                None
            }
        } else if batch_result.failed == 0 && abort_reason.is_none() {
            if let Err(err) = cp.cleanup() {
                crate::media_conversion_gate::delivery_pipeline_batch_audit(
                    "delivery_pipeline_batch",
                    format!(
                        "100% success reached, but failed to purge completed-list state: {err}"
                    ),
                );
                Some(format!("failed to purge completed checkpoint state: {err}"))
            } else {
                None
            }
        } else if let Err(err) = cp.release_lock() {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_batch",
                format!("Failed to release file lock after batch failure: {err}"),
            );
            Some(format!(
                "failed to release checkpoint lock after batch failure: {err}"
            ))
        } else {
            None
        }
    } else {
        None
    };

    crate::infra::static_logs::log_batch_complete_audit(
        audit_pipeline,
        batch_result.succeeded,
        batch_result.skipped,
        batch_result.ignored,
        batch_result.failed,
        batch_result.total,
    );

    print_summary(
        &batch_result,
        start_time.elapsed(),
        total_input_bytes.load(Ordering::Relaxed),
        total_output_bytes.load(Ordering::Relaxed),
        &config.label,
    );

    if let Some(error) = checkpoint_error {
        anyhow::bail!(error);
    }
    if batch_result.paused {
        return Ok(());
    }
    if let Some(reason) = abort_reason {
        anyhow::bail!("Batch aborted by error policy after {reason}");
    }

    if let Some(ref output_dir) = config.output {
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_BATCH,
            crate::infra::static_logs::messages::MSG_COPY_UNSUPPORTED
        );
        let copy_result = copy_unsupported_files(input, output_dir, recursive);
        if copy_result.copied > 0 {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_COPY,
                &format!("Copied {} unsupported files", copy_result.copied)
            );
        }
        if copy_result.failed > 0 {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_cli",
                format!(
                    "Failed to copy {} files to output directory",
                    copy_result.failed
                ),
            );
            let sample_errors = copy_result
                .errors
                .iter()
                .take(3)
                .map(|(path, message, category)| {
                    format!("{category}: {} ({message})", path.display())
                })
                .collect::<Vec<_>>()
                .join(" | ");
            anyhow::bail!(
                "Batch output is incomplete: {} unsupported file copies failed{}",
                copy_result.failed,
                if sample_errors.is_empty() {
                    String::new()
                } else {
                    format!("; sample failures: {sample_errors}")
                }
            );
        }

        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_VERIFY,
            crate::infra::static_logs::messages::MSG_VERIFY_COMPLETENESS
        );
        let verify = verify_output_completeness_for_domain(
            input,
            output_dir,
            recursive,
            VerifyDomain::VideosAndPassthrough,
        );
        let ignored_count = ignored.load(Ordering::Relaxed);
        let failed_count = failed.load(Ordering::Relaxed);
        let adjusted_expected = verify
            .expected
            .saturating_sub(ignored_count)
            .saturating_sub(failed_count);
        let adjusted_diff = crate::numeric_cast::usize_to_i64_sat(adjusted_expected)
            - crate::numeric_cast::usize_to_i64_sat(verify.actual);
        let (adjusted_passed, adjusted_message) = match adjusted_diff.cmp(&0) {
            std::cmp::Ordering::Equal => (
                true,
                format!(
                    "✅ Verification passed: {} files (ignored {} files, failed {} files excluded)",
                    verify.actual, ignored_count, failed_count
                ),
            ),
            std::cmp::Ordering::Greater => (
                false,
                crate::media_conversion_gate::ui_user_facing_error(format!(
                    "Verification FAILED: missing {adjusted_diff} files after excluding \
                     {ignored_count} ignored and {failed_count} failed inputs (expected \
                     {adjusted_expected}, got {})",
                    verify.actual
                )),
            ),
            std::cmp::Ordering::Less => (
                true,
                format!(
                    "{} Output has {} extra files after excluding {} ignored and {} failed inputs \
                     (expected {}, got {})",
                    crate::modern_ui::symbols::styled_warning_icon(),
                    -adjusted_diff,
                    ignored_count,
                    failed_count,
                    adjusted_expected,
                    verify.actual
                ),
            ),
        };
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_VERIFY,
            &adjusted_message
        );
        if !adjusted_passed {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_cli",
                crate::infra::static_logs::messages::MSG_VERIFY_MISMATCH,
            );
            anyhow::bail!("Batch output completeness verification failed: {adjusted_message}");
        }

        if let Some(ref base_dir) = config.base_dir {
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_METADATA,
                crate::infra::static_logs::messages::MSG_METADATA_DIR_SYNC
            );
            if let Err(e) = crate::metadata::preserve_directory(base_dir, output_dir) {
                crate::media_conversion_gate::delivery_pipeline_batch_audit(
                    "delivery_pipeline_cli",
                    format!("Failed to sync directory timestamps/permissions: {e}"),
                );
                anyhow::bail!(
                    "Batch directory metadata synchronization failed for {} -> {}: {e}",
                    base_dir.display(),
                    output_dir.display()
                );
            }
            crate::log_info!(
                crate::infra::static_logs::messages::LABEL_METADATA,
                crate::infra::static_logs::messages::MSG_METADATA_DIR_DONE
            );
        }
    }

    if batch_result.failed > 0 {
        anyhow::bail!(
            "Batch completed with {} failed file(s)",
            batch_result.failed
        );
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

    // Fix extension by content first so all downstream checks see the real format
    // (avoids disguised-extension panic). When an output directory is
    // configured the source tree must remain immutable: use the readonly
    // variant that logs mismatches without renaming source files.
    let fixed_input = if config.output.is_some() {
        crate::smart_file_copier::check_extension_mismatch_readonly(&config.input)?
    } else {
        fix_extension_if_mismatch(&config.input)?
    };
    let input = fixed_input.as_path();

    if !has_extension(input, SUPPORTED_VIDEO_EXTENSIONS) {
        let ext_str = crate::media_conversion_gate::delivery_cli_extension_display_or_none(input);
        let supported = SUPPORTED_VIDEO_EXTENSIONS.join(", ");
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_BATCH,
            &format!(
                "IGNORED: outside video domain: {input_display} (extension after content check: \
                 .{ext_str}; supported video formats: {supported})",
                input_display = input.display(),
            )
        );
        return Ok(());
    }

    let result = match converter(input) {
        Ok(r) => r,
        Err(e) => {
            return Err(e);
        }
    };

    crate::log_info!(crate::infra::static_logs::messages::LABEL_REPORT, "");
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_REPORT,
        crate::infra::static_logs::messages::CONVERSION_SUMMARY
    );
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_REPORT,
        &format!(
            "   Input:  {path} ({size} bytes)",
            path = result.input_path(),
            size = result.input_size()
        )
    );
    if let Some(out_path) = result.output_path() {
        let output_size = crate::media_conversion_gate::delivery_cli_output_size_label_or_unknown(
            result.output_size(),
            result.input_path(),
            out_path,
        );
        crate::log_info!(
            crate::infra::static_logs::messages::LABEL_REPORT,
            &format!("   Output: {out_path} ({output_size} bytes)")
        );
    }
    crate::log_info!(
        crate::infra::static_logs::messages::LABEL_REPORT,
        &format!("   Result: {message}", message = result.message())
    );

    // 📡 Audit: Log live decision to JSONL if debug/ exists
    log_live_audit_to_jsonl(
        result.blake3(),
        &config.label,
        crate::media_conversion_gate::trace_label_or_default(result.output_path(), "original"),
        result.message(),
    );

    if !result.is_success() && !result.is_skipped() && !result.is_ignored() {
        anyhow::bail!(result.message().to_string());
    }

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

    match serde_json::to_string(&record) {
        Ok(json) => {
            use std::io::Write;
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(audit_file)
            {
                Ok(mut file) => {
                    if let Err(e) = writeln!(file, "{json}") {
                        crate::media_conversion_gate::delivery_pipeline_batch_audit(
                            "delivery_pipeline_cli",
                            format!("Failed to append live audit record: {e}"),
                        );
                    }
                }
                Err(e) => {
                    crate::media_conversion_gate::delivery_pipeline_batch_audit(
                        "delivery_pipeline_cli",
                        format!("Failed to open live audit JSONL for append: {e}"),
                    );
                }
            }
        }
        Err(e) => {
            crate::media_conversion_gate::delivery_pipeline_batch_audit(
                "delivery_pipeline_cli",
                format!("Failed to serialize live audit record: {e}"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_MP4: &[u8] = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2";

    fn test_config(input: PathBuf) -> Config {
        Config {
            input,
            output: None,
            recursive: false,
            label: "test".to_string(),
            base_dir: None,
            resume: false,
            protect_destructive_dirs: false,
            error_mode: crate::BatchErrorMode::LogAndContinue,
        }
    }

    #[test]
    fn failed_task_result_makes_single_file_command_fail() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let input = temp.path().join("broken.mp4");
        std::fs::write(&input, MINIMAL_MP4)?;

        let error = process_single_file(&test_config(input), |path| {
            Ok(crate::conversion::TaskResult::failed(
                path,
                u64::try_from(MINIMAL_MP4.len()).expect("fixture length fits u64"),
                "verification failed",
                "verification_failed",
            ))
        })
        .expect_err("failed task must make the command fail");

        assert_eq!(error.to_string(), "verification failed");
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn one_failed_file_does_not_stop_the_batch() -> anyhow::Result<()> {
        let _skip_disk_precheck =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_SKIP_DISK_PRECHECK, "1");
        let temp = tempfile::tempdir()?;
        let total = 8;
        for index in 0..total {
            std::fs::write(temp.path().join(format!("{index:02}.mp4")), MINIMAL_MP4)?;
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_converter = Arc::clone(&calls);

        let result = process_directory(&test_config(temp.path().to_path_buf()), move |path| {
            let call = calls_in_converter.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err(crate::unified_error::UnifiedError::analysis_error("bad media").into())
            } else {
                Ok(crate::conversion::TaskResult::skipped_custom(
                    path,
                    u64::try_from(MINIMAL_MP4.len()).expect("fixture length fits u64"),
                    "size gate retained source",
                    "size_increase",
                ))
            }
        });

        assert!(
            result.is_err(),
            "batch must report its failed file; result={result:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            total,
            "a per-file failure must not stop later files; result={result:?}"
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn fail_fast_recoverable_error_stops_before_next_file() -> anyhow::Result<()> {
        let _low_memory =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_LOW_MEMORY, "1");
        let _skip_disk_precheck =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_SKIP_DISK_PRECHECK, "1");
        let temp = tempfile::tempdir()?;
        for index in 0..4 {
            std::fs::write(temp.path().join(format!("{index:02}.mp4")), MINIMAL_MP4)?;
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_converter = Arc::clone(&calls);
        let mut config = test_config(temp.path().to_path_buf());
        config.error_mode = crate::BatchErrorMode::FailFast;

        let result = process_directory(&config, move |path| {
            let call = calls_in_converter.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err(crate::UnifiedError::analysis_error("debug fixture failure").into())
            } else {
                Ok(crate::conversion::TaskResult::skipped_custom(
                    path,
                    u64::try_from(MINIMAL_MP4.len()).expect("fixture length fits u64"),
                    "must not run",
                    "unexpected_call",
                ))
            }
        });

        assert!(result.is_err(), "result={result:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fail-fast must stop before the next file; result={result:?}"
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn normal_mode_unknown_error_stops_before_next_file() -> anyhow::Result<()> {
        let _low_memory =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_LOW_MEMORY, "1");
        let _skip_disk_precheck =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_SKIP_DISK_PRECHECK, "1");
        let temp = tempfile::tempdir()?;
        for index in 0..4 {
            std::fs::write(temp.path().join(format!("{index:02}.mp4")), MINIMAL_MP4)?;
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_converter = Arc::clone(&calls);

        let result = process_directory(&test_config(temp.path().to_path_buf()), move |path| {
            let call = calls_in_converter.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                Err(anyhow::anyhow!("unclassified system failure"))
            } else {
                Ok(crate::conversion::TaskResult::skipped_custom(
                    path,
                    u64::try_from(MINIMAL_MP4.len()).expect("fixture length fits u64"),
                    "must not run",
                    "unexpected_call",
                ))
            }
        });

        assert!(result.is_err(), "result={result:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "normal mode must fail closed for unknown errors; result={result:?}"
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn skipped_source_copy_failure_is_fatal_in_normal_mode() -> anyhow::Result<()> {
        let _low_memory =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_LOW_MEMORY, "1");
        let _skip_disk_precheck =
            crate::common_utils::EnvGuard::set(crate::constants::ENV_MFB_SKIP_DISK_PRECHECK, "1");
        let temp = tempfile::tempdir()?;
        let input = temp.path().join("input");
        std::fs::create_dir(&input)?;
        for index in 0..4 {
            std::fs::write(input.join(format!("{index:02}.mp4")), MINIMAL_MP4)?;
        }
        let input = std::fs::canonicalize(input)?;
        let output_blocker = temp.path().join("output-is-a-file");
        std::fs::write(&output_blocker, b"not a directory")?;

        let calls = Arc::new(AtomicUsize::new(0));
        let calls_in_converter = Arc::clone(&calls);
        let mut config = test_config(input.clone());
        config.output = Some(output_blocker);
        config.base_dir = Some(input);

        let result = process_directory(&config, move |path| {
            calls_in_converter.fetch_add(1, Ordering::SeqCst);
            Ok(crate::conversion::TaskResult::skipped_custom(
                path,
                u64::try_from(MINIMAL_MP4.len()).expect("fixture length fits u64"),
                "size gate retained source",
                "size_increase",
            ))
        });

        let error = result.expect_err("failed source preservation must fail the batch");
        assert!(
            error
                .to_string()
                .contains("failed to preserve skipped source"),
            "error={error:#}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "integrity failure must stop before the next file"
        );
        Ok(())
    }
}

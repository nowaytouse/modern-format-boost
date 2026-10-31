//! Application session lifetime and attempt-scoped history, independent of a CLI shell.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::app::worker::{ToolSnapshotBindings, WorkerSession};
use crate::infra::drag_drop::OutputCleanupReceipt;
use crate::infra::history_store::{self, HistorySessionLease};
use crate::infra::log_paths::{
    append_jsonl_audit_record, archive_drag_drop_session_bundle, ensure_unified_log_dir,
    format_session_stamp,
};
use crate::infra::process_stream::{ProcessorByteTotals, ProcessorStats};
use crate::infra::rich_panel::PipelineSummary;
use crate::infra::ui_tokens::pick_symbol;

/// A session owns its history lease and artifacts; each attempt has an independent run identity.
#[derive(Clone)]
pub struct AppSession {
    _history_lease: Arc<HistorySessionLease>,
    started_at: DateTime<Local>,
    stamp: String,
    log_dir: PathBuf,
    session_log: PathBuf,
    verbose_log: PathBuf,
    session_audit: PathBuf,
    run_id: Option<String>,
    tool_snapshots: Arc<ToolSnapshotBindings>,
}

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl AppSession {
    /// Scope an execution attempt without releasing the shared history lease.
    ///
    /// # Errors
    /// Returns an error when the process-local attempt sequence is exhausted.
    pub fn for_run(&self) -> Result<Self> {
        let sequence = RUN_SEQUENCE
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("run identity sequence exhausted"))?;
        let mut scoped = self.clone();
        scoped.tool_snapshots = Arc::default();
        let stamp_hash = blake3::hash(self.stamp.as_bytes()).to_hex();
        scoped.run_id = Some(format!(
            "run-{}-{}-{sequence}",
            std::process::id(),
            &stamp_hash[..24]
        ));
        Ok(scoped)
    }

    /// Start the normal application session and bind its child logging environment.
    ///
    /// # Errors
    /// Returns log-directory, lease, artifact reservation or history registration failures.
    pub fn start() -> Result<Self> {
        let log_dir = ensure_unified_log_dir()?;
        Self::start_with_log_dir(&log_dir, true)
    }

    /// Start in an explicit log directory without changing the process environment.
    ///
    /// # Errors
    /// Returns log-directory, lease, artifact reservation or history registration failures.
    pub fn start_in(log_dir: &Path) -> Result<Self> {
        Self::start_with_log_dir(log_dir, false)
    }

    fn start_with_log_dir(log_dir: &Path, set_child_env: bool) -> Result<Self> {
        fs::create_dir_all(log_dir)
            .with_context(|| format!("create drag/drop log dir {}", log_dir.display()))?;
        let log_dir = log_dir
            .canonicalize()
            .with_context(|| format!("canonicalize drag/drop log dir {}", log_dir.display()))?;
        let started_at = Local::now();
        let stamp = format!(
            "{}_{}_{:09}",
            format_session_stamp(Some(started_at)),
            std::process::id(),
            started_at.timestamp_subsec_nanos()
        );
        let session = Self {
            _history_lease: history_store::acquire_history_session_lease(&log_dir, &stamp)?,
            started_at,
            session_log: log_dir.join(format!("MFB_Session_{stamp}.log")),
            verbose_log: log_dir.join(format!("verbose_{stamp}.log")),
            session_audit: log_dir.join(format!("session_audit_{stamp}.jsonl")),
            log_dir,
            stamp,
            run_id: None,
            tool_snapshots: Arc::default(),
        };
        // A collision must never append another session's events or counters.
        for path in [
            &session.session_log,
            &session.verbose_log,
            &session.session_audit,
        ] {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .context("reserve unique processing session log")?;
        }
        if set_child_env {
            unsafe {
                std::env::set_var("MFB_SESSION_ID", &session.stamp);
                std::env::set_var("MFB_LOG_DIR", session.log_dir.as_os_str());
            }
            if std::env::var("RUST_LOG").map_or(true, |value| value.trim().is_empty()) {
                unsafe {
                    std::env::set_var("RUST_LOG", "trace");
                }
            }
        }
        session.append_line(&session.session_log, "SESSION_STARTED")?;
        session.append_line(&session.verbose_log, "SESSION_STARTED")?;
        append_jsonl_audit_record(&session.session_audit, "SESSION_STARTED")?;
        history_store::register_history_artifacts(
            &session.log_dir,
            &session.stamp,
            &[
                session.session_log.clone(),
                session.verbose_log.clone(),
                session.session_audit.clone(),
            ],
        )?;
        Ok(session)
    }

    /// Scope an input's diagnostic audit while retaining the same session and attempt.
    #[must_use]
    pub fn for_input_audit(&self, index: usize, count: usize) -> Self {
        let mut scoped = self.clone();
        if count > 1 {
            scoped.session_audit = self
                .log_dir
                .join(format!("session_audit_{}_input_{index}.jsonl", self.stamp));
        }
        scoped
    }

    #[must_use]
    pub fn session_log(&self) -> &Path {
        &self.session_log
    }

    #[must_use]
    pub fn verbose_log(&self) -> &Path {
        &self.verbose_log
    }

    /// Append one timestamped diagnostic line.
    ///
    /// # Errors
    /// Returns failures to open or write the requested diagnostic artifact.
    pub fn append_line(&self, path: &Path, event: &str) -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open drag/drop log {}", path.display()))?;
        let stamp = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        writeln!(file, "[{stamp}] {event}")
            .with_context(|| format!("write drag/drop log {}", path.display()))
    }

    /// Bind a structured history event to this attempt before persistence.
    ///
    /// # Errors
    /// Returns an error for conflicting identity, invalid payload or failed storage.
    pub fn append_history_event(&self, name: &str, mut payload: serde_json::Value) -> Result<()> {
        if let Some(run_id) = &self.run_id {
            let fields = payload
                .as_object_mut()
                .context("history payload is not an object")?;
            if let Some(existing) = fields.get("run_id") {
                anyhow::ensure!(
                    existing.as_str() == Some(run_id.as_str()),
                    "history event run identity does not match the active run"
                );
            } else {
                fields.insert("run_id".to_owned(), serde_json::json!(run_id));
            }
        }
        history_store::append_history_event(
            &self.log_dir,
            &self.stamp,
            &format!("{name}={payload}"),
        )
    }

    /// Persist a cleanup receipt before publishing its interface marker.
    ///
    /// # Errors
    /// Returns serialization, size-limit, persistence or output failures.
    pub fn record_output_cleanup(&self, mut receipt: OutputCleanupReceipt) -> Result<()> {
        receipt.run_id.clone_from(&self.run_id);
        let payload = serde_json::to_value(receipt).context("serialize output cleanup receipt")?;
        let marker = serde_json::to_string(&payload).context("encode output cleanup marker")?;
        anyhow::ensure!(
            marker.len() < 64 * 1024 - "MFB_HISTORY_OUTPUT_CLEANUP=".len(),
            "output cleanup marker exceeds 64 KiB"
        );
        self.append_history_event("MFB_HISTORY_OUTPUT_CLEANUP", payload)?;
        writeln!(io::stdout().lock(), "MFB_OUTPUT_CLEANUP={marker}")
            .context("write output cleanup marker to stdout")
    }

    /// Persist a current Fast IMG result only when its counts match worker observations.
    ///
    /// # Errors
    /// Returns malformed, oversized, duplicate or conflicting result and persistence errors.
    pub fn record_fast_img_result(&self, text: &str, stats: &ProcessorStats) -> Result<()> {
        let mut reports = text
            .lines()
            .filter(|line| line.contains("MFB_FAST_IMG_RESULT="));
        let Some(line) = reports.next() else {
            return Ok(());
        };
        let payload = line
            .strip_prefix("MFB_FAST_IMG_RESULT=")
            .context("fast-img result has an invalid record prefix")?;
        anyhow::ensure!(
            payload.len() <= 32_768 && reports.next().is_none(),
            "fast-img result is oversized or duplicated"
        );
        let report: serde_json::Value =
            serde_json::from_str(payload).context("decode fast-img result for history")?;
        anyhow::ensure!(
            report["schema_version"] == 1 && stats.counts_complete(),
            "fast-img history requires complete current result counts"
        );
        for (key, expected) in [
            ("succeeded", stats.succeeded),
            ("skipped", stats.skipped),
            ("failed", stats.failed),
            ("ignored", stats.ignored),
            (
                "unprocessed",
                stats
                    .unprocessed
                    .context("missing fast-img unprocessed count")?,
            ),
        ] {
            anyhow::ensure!(
                report[key].as_u64() == Some(u64::try_from(expected)?),
                "fast-img result count {key} disagrees with worker output"
            );
        }
        self.append_history_event("MFB_HISTORY_FAST_IMG_RESULT", report)
    }

    /// Record machine verification facts separately from processor outcome counts.
    ///
    /// # Errors
    /// Returns missing evidence, path encoding or persistence errors.
    pub fn record_history_verification(
        &self,
        verify: &crate::infra::drag_drop::VerifyOutcome,
        source: &Path,
        output: &Path,
        failed_sources_verified: Option<usize>,
    ) -> Result<()> {
        let counts = verify
            .integrity_summary
            .as_ref()
            .context("history verification requires the machine integrity summary")?;
        let source =
            std::path::absolute(source).context("resolve history verification source path")?;
        let output =
            std::path::absolute(output).context("resolve history verification output path")?;
        self.append_history_event("MFB_HISTORY_VERIFICATION", serde_json::json!({
            "schema_version": 1,
            "source_path": serde_json::to_value(source).context("encode history verification source path")?,
            "output_path": serde_json::to_value(output).context("encode history verification output path")?,
            "has_warnings": counts.has_warnings, "issue_count": counts.issue_count,
            "source_count": counts.source_count, "optimized_count": counts.optimized_count,
            "skipped_count": counts.skipped_count, "failed_count": counts.failed_count,
            "source_remaining_count": counts.source_remaining_count,
            "verified_deleted_count": counts.verified_deleted_count, "count_status": counts.count_status,
            "failed_sources_verified": failed_sources_verified,
            "primary_cleanup_complete": failed_sources_verified.map(|_| true),
        }))
    }

    /// Rename this session's diagnostic log using the selected input name.
    ///
    /// # Errors
    /// Returns a filesystem rename failure without changing the recorded log path.
    pub fn rename_log_to_project(&mut self, target: &Path) -> Result<()> {
        let project_name = target.file_name().map_or_else(
            || "project".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let new_name = self
            .log_dir
            .join(format!("MFB_{project_name}_{}.log", self.stamp));
        if self.session_log == new_name {
            return Ok(());
        }
        if self.session_log.is_file() {
            fs::rename(&self.session_log, &new_name).with_context(|| {
                format!(
                    "rename session log {} -> {}",
                    self.session_log.display(),
                    new_name.display()
                )
            })?;
            self.session_log = new_name;
        }
        Ok(())
    }

    /// Persist the result summary and append its diagnostic representation.
    ///
    /// # Errors
    /// Returns failures to serialize, persist or write the final summary.
    pub fn finish_log(&self, summary: &PipelineSummary, size_summary: Option<&str>) -> Result<()> {
        self.append_history_event("MFB_HISTORY_SUMMARY", history_summary_payload(summary))?;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.session_log)
            .with_context(|| format!("open session log {}", self.session_log.display()))?;
        writeln!(file, "\n========================================")?;
        writeln!(file, "Final Statistics")?;
        writeln!(file, "========================================")?;
        writeln!(
            file,
            "End Time: {}",
            format_session_stamp(Some(Local::now()))
        )?;
        writeln!(file, "Verbose Log: {}", self.verbose_log.display())?;
        let label =
            |count: Option<usize>| count.map_or_else(|| "unknown".to_owned(), |n| n.to_string());
        for (media, stats, active) in [
            ("Images", &summary.img, summary.has_image_stats()),
            ("Videos", &summary.vid, summary.has_video_stats()),
        ] {
            if active {
                writeln!(
                    file,
                    "{media}: {} succeeded, {} skipped, {} ignored, {} failed, {} unprocessed",
                    label(stats.reported[0].then_some(stats.succeeded)),
                    label(stats.reported[1].then_some(stats.skipped)),
                    label(stats.reported[2].then_some(stats.ignored)),
                    label(stats.reported[3].then_some(stats.failed)),
                    label(stats.unprocessed)
                )?;
            }
        }
        writeln!(
            file,
            "Total: {} succeeded, {} skipped, {} ignored, {} failed, {} unprocessed",
            label(summary.total_count(0)),
            label(summary.total_count(1)),
            label(summary.total_count(2)),
            label(summary.total_count(3)),
            label(summary.total_unprocessed())
        )?;
        writeln!(
            file,
            "Count scope: processor outcomes, not unique files or Photos assets"
        )?;
        writeln!(
            file,
            "Conversion Success Rate: {}",
            summary
                .success_rate()
                .map_or_else(|| "N/A".to_owned(), |rate| rate.to_string())
        )?;
        if let Some(state) = summary.integrity_state {
            writeln!(
                file,
                "Integrity: {state}; issues={}",
                summary.integrity_issue_count
            )?;
        }
        if !summary.failed_file_names.is_empty() || !summary.skipped_file_names.is_empty() {
            writeln!(file, "Retained files (source preserved):")?;
            for name in &summary.failed_file_names {
                writeln!(file, "  [FAIL] {name}")?;
            }
            for name in &summary.skipped_file_names {
                writeln!(file, "  [SKIP] {name}")?;
            }
        } else if let Some(failed) = summary.total_count(3).filter(|count| *count > 0) {
            writeln!(
                file,
                "Failed: {} file(s) - see verbose log for details: {}",
                failed,
                self.verbose_log.display()
            )?;
        }
        if let Some(block) = size_summary {
            writeln!(file, "{block}")?;
        }
        writeln!(
            file,
            "\n========================================\nSession completed.\n========================================"
        )?;
        append_jsonl_audit_record(
            &self.session_audit,
            &format!(
                "SESSION_COMPLETED images_ok={} images_skip={} images_ignore={} images_fail={} images_unprocessed={} videos_ok={} \
             videos_skip={} videos_ignore={} videos_fail={} videos_unprocessed={} integrity={:?} integrity_issues={}",
                label(summary.img.reported[0].then_some(summary.img.succeeded)),
                label(summary.img.reported[1].then_some(summary.img.skipped)),
                label(summary.img.reported[2].then_some(summary.img.ignored)),
                label(summary.img.reported[3].then_some(summary.img.failed)),
                label(summary.img.unprocessed),
                label(summary.vid.reported[0].then_some(summary.vid.succeeded)),
                label(summary.vid.reported[1].then_some(summary.vid.skipped)),
                label(summary.vid.reported[2].then_some(summary.vid.ignored)),
                label(summary.vid.reported[3].then_some(summary.vid.failed)),
                label(summary.vid.unprocessed),
                summary.integrity_state,
                summary.integrity_issue_count
            ),
        )?;
        let has_integrity_issues =
            summary.integrity_state == Some("WARNINGS") && summary.integrity_issue_count > 0;
        if summary.total_failed() > 0 || has_integrity_issues {
            eprintln!(
                "   {} Session log:  {}",
                pick_symbol("📋", "[LOG]"),
                self.session_log.display()
            );
        }
        Ok(())
    }

    /// Archive session artifacts and register their resulting locations.
    ///
    /// # Errors
    /// Returns artifact, archive or history registration failures.
    pub fn archive(&self) -> Result<Option<PathBuf>> {
        self.append_line(&self.session_log, "SESSION_ARCHIVE_BEGIN")?;
        self.append_line(&self.verbose_log, "SESSION_ARCHIVE_BEGIN")?;
        append_jsonl_audit_record(&self.session_audit, "SESSION_ARCHIVE_BEGIN")?;
        let archived = archive_drag_drop_session_bundle(
            &self.log_dir,
            &self.stamp,
            Some(&self.session_log),
            Some(&self.verbose_log),
            Some(&self.session_audit),
            Some(self.started_at),
        )?;
        if let Some(bundle) = &archived {
            let archived_audit = bundle.join(format!("session_audit_{}.jsonl", self.stamp));
            append_jsonl_audit_record(&archived_audit, "SESSION_ARCHIVE_DONE")?;
            let paths = [&self.session_log, &self.verbose_log, &self.session_audit]
                .into_iter()
                .map(|path| {
                    path.file_name()
                        .map(|name| bundle.join(name))
                        .context("session artifact has no filename")
                })
                .collect::<Result<Vec<_>>>()?;
            history_store::register_history_artifacts(&self.log_dir, &self.stamp, &paths)?;
        }
        Ok(archived)
    }
}

impl WorkerSession for AppSession {
    fn tool_snapshots(&self) -> &ToolSnapshotBindings {
        &self.tool_snapshots
    }
    fn run_id(&self) -> Option<&str> {
        self.run_id.as_deref()
    }
    fn stamp(&self) -> &str {
        &self.stamp
    }
    fn log_dir(&self) -> &Path {
        &self.log_dir
    }
    fn session_audit(&self) -> &Path {
        &self.session_audit
    }
    fn append_history_event(&self, name: &str, payload: serde_json::Value) -> Result<()> {
        Self::append_history_event(self, name, payload)
    }
    fn append_session_line(&self, event: &str) -> Result<()> {
        self.append_line(&self.session_log, event)
    }
    fn append_verbose_line(&self, event: &str) -> Result<()> {
        self.append_line(&self.verbose_log, event)
    }
}

/// Context supplied by an application entrypoint, not by worker-output parsing.
pub struct RunContext {
    pub inputs: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub backup: Option<PathBuf>,
    pub mode: String,
    pub dry_run: bool,
    pub in_place: bool,
    pub shortest_path: bool,
    pub retry: bool,
    pub resume: bool,
    pub no_resume: bool,
}

/// Persist context before execution and the terminal outcome afterwards.
/// A missing session deliberately does not resolve or persist history context.
///
/// # Errors
/// Returns context, processing or history-write failures without replacing a primary processing error.
pub fn run_with_history(
    session: Option<&AppSession>,
    context: impl FnOnce() -> Result<RunContext>,
    run: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let absolute = |path: &Path| -> Result<PathBuf> {
        std::path::absolute(path).context("resolve history absolute path")
    };
    let context_result = match session {
        Some(session) => (|| {
            let context = context()?;
            let inputs = context
                .inputs
                .iter()
                .map(|path| absolute(path))
                .collect::<Result<Vec<_>>>()?;
            let output = context.output.as_deref().map(absolute).transpose()?;
            let backup = context.backup.as_deref().map(absolute).transpose()?;
            session.append_history_event("MFB_HISTORY_CONTEXT", serde_json::json!({
                "schema_version": 1,
                "inputs": serde_json::to_value(inputs).context("encode history input paths")?,
                "output": serde_json::to_value(output).context("encode history output path")?,
                "backup": serde_json::to_value(backup).context("encode history backup path")?,
                "mode": context.mode, "dry_run": context.dry_run, "in_place": context.in_place,
                "shortest_path": context.shortest_path, "version": env!("CARGO_PKG_VERSION"),
                "retry": context.retry, "resume": context.resume, "no_resume": context.no_resume,
                "output_sequence": history_store::session_output_cursor(&session.log_dir, &session.stamp)?,
            }))
        })(),
        None => Ok(()),
    };
    let context_failed = context_result.is_err();
    let result = context_result.and_then(|()| run());
    if let Some(session) = session {
        let cancelled = if result.is_err() && !context_failed {
            match foundation::batch_control::is_cancelled() {
                Ok(cancelled) => cancelled,
                Err(error) => {
                    eprintln!("history cancellation state unavailable: {error:#}");
                    false
                }
            }
        } else {
            false
        };
        let finished = (|| {
            session.append_history_event("MFB_HISTORY_FINISHED", serde_json::json!({
            "schema_version": 1,
            "outcome": if cancelled { "cancelled" } else if result.is_err() { "failed" } else { "completed" },
            "error": result.as_ref().err().map(|error| format!("{error:#}")),
            "output_sequence": history_store::session_output_cursor(&session.log_dir, &session.stamp)?,
        }))
        })();
        if let Err(error) = finished {
            if result.is_ok() {
                return Err(error).context("write history terminal outcome");
            }
            eprintln!("history terminal outcome write failed: {error:#}");
        }
    }
    result
}

/// Preserve missing/inconsistent processor counts as unknown in application receipts.
pub fn processor_history_counts(stats: &ProcessorStats, active: bool) -> serde_json::Value {
    let inventory_valid = !stats.unprocessed_invalid
        && stats.checked_total().is_some_and(|total| {
            stats
                .unprocessed
                .is_none_or(|pending| total.checked_add(pending).is_some())
        });
    let count =
        |index: usize, value| (active && stats.reported[index] && inventory_valid).then_some(value);
    serde_json::json!({
        "active": active,
        "succeeded": count(0, stats.succeeded),
        "skipped": count(1, stats.skipped),
        "ignored": count(2, stats.ignored),
        "failed": count(3, stats.failed),
        "unprocessed": stats.unprocessed.filter(|_| active && inventory_valid),
        "exit_code": active.then_some(stats.exit_code),
    })
}

/// Build the structured session summary without conflating counts with integrity proof.
pub fn history_summary_payload(summary: &PipelineSummary) -> serde_json::Value {
    let active = [
        (&summary.img, summary.has_image_stats()),
        (&summary.vid, summary.has_video_stats()),
    ];
    let size_invalid = active
        .iter()
        .any(|(stats, enabled)| *enabled && stats.converted_bytes_invalid);
    let size = (summary
        .fast_img_session_converted
        .is_some_and(|count| count > 0)
        && (summary.fast_img_session_source_bytes.is_some()
            || summary.fast_img_session_output_bytes.is_some()))
    .then(|| {
        serde_json::json!({
            "scope": "fast_img_converted_this_run",
            "input_bytes": summary.fast_img_session_source_bytes,
            "output_bytes": summary.fast_img_session_output_bytes,
        })
    })
    .or_else(|| {
        if size_invalid
            || !active
                .iter()
                .any(|(stats, enabled)| *enabled && stats.converted_bytes.is_some())
        {
            return None;
        }
        let bytes = active
            .iter()
            .filter(|(_, enabled)| *enabled)
            .map(|(stats, _)| {
                stats
                    .converted_bytes
                    .unwrap_or_else(ProcessorByteTotals::unknown)
            })
            .reduce(ProcessorByteTotals::merge)?;
        let supported = |value: Option<u64>| value.filter(|bytes| i64::try_from(*bytes).is_ok());
        Some(serde_json::json!({
            "scope": "converted_this_run",
            "input_bytes": supported(bytes.input_bytes),
            "output_bytes": supported(bytes.output_bytes),
        }))
    });
    serde_json::json!({
        "schema_version": 1,
        "count_scope": "processor_outcomes",
        "img": processor_history_counts(&summary.img, summary.has_image_stats()),
        "vid": processor_history_counts(&summary.vid, summary.has_video_stats()),
        "integrity": {
            "state": summary.integrity_state,
            "issue_count": summary.integrity_state.map(|_| summary.integrity_issue_count),
        },
        "failed_files": summary.failed_file_names,
        "skipped_files": summary.skipped_file_names,
        "size": size,
        "size_invalid": size_invalid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_attempts_and_input_scopes_keep_the_session_lease() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let session = AppSession::start_in(directory.path())?;
        let first = session.for_run()?;
        let second = session.for_run()?;
        assert_ne!(first.run_id(), second.run_id());
        assert!(session.run_id().is_none());
        assert!(Arc::ptr_eq(&session._history_lease, &first._history_lease));
        assert!(!Arc::ptr_eq(&first.tool_snapshots, &second.tool_snapshots));
        let scoped = first.for_input_audit(1, 2);
        assert_eq!(scoped.run_id(), first.run_id());
        assert_eq!(scoped.stamp(), session.stamp());
        assert!(Arc::ptr_eq(&scoped._history_lease, &session._history_lease));
        assert!(Arc::ptr_eq(&scoped.tool_snapshots, &first.tool_snapshots));
        assert_ne!(scoped.session_audit(), first.session_audit());
        assert_eq!(
            first.for_input_audit(0, 1).session_audit(),
            first.session_audit()
        );
        Ok(())
    }

    #[test]
    fn long_session_stamp_keeps_worker_invocation_identity_bounded() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let session = AppSession::start_in(directory.path())?;
        let command = crate::app::worker::LaunchCommand::from_argv(vec![
            directory
                .path()
                .join("unavailable-worker")
                .to_string_lossy()
                .into_owned(),
        ])?;
        let original = command.invocation(&session)?.invocation_id;
        let mut long_session = session.clone();
        long_session.stamp = "Unicode 界面".repeat(300);
        let long_invocation = command.invocation(&long_session)?;
        assert!(long_invocation.invocation_id.len() <= 256);
        assert!(long_invocation.invocation_id.is_ascii());
        assert!(
            long_invocation
                .invocation_id
                .starts_with(&format!("{}-", std::process::id()))
        );
        assert_ne!(long_invocation.invocation_id, original);
        assert_ne!(
            long_invocation.invocation_id,
            command.invocation(&long_session)?.invocation_id
        );
        Ok(())
    }

    #[test]
    fn context_failure_prevents_work_and_retains_terminal_error() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let session = AppSession::start_in(directory.path())?.for_run()?;
        let called = std::cell::Cell::new(false);
        let error = run_with_history(
            Some(&session),
            || anyhow::bail!("synthetic context failure"),
            || {
                called.set(true);
                Ok(())
            },
        )
        .unwrap_err();
        assert!(!called.get());
        assert_eq!(error.to_string(), "synthetic context failure");
        let database = rusqlite::Connection::open_with_flags(
            session.log_dir().join(history_store::HISTORY_DATABASE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let event: String = database.query_row(
            "SELECT event FROM history_events WHERE session_id=?1 ORDER BY sequence DESC LIMIT 1",
            [session.stamp()],
            |row| row.get(0),
        )?;
        let payload: serde_json::Value = serde_json::from_str(
            event
                .strip_prefix("MFB_HISTORY_FINISHED=")
                .context("terminal event missing")?,
        )?;
        assert_eq!(payload["outcome"], "failed");
        assert_eq!(payload["error"], "synthetic context failure");
        assert_eq!(payload["run_id"].as_str(), session.run_id());
        Ok(())
    }

    #[test]
    fn history_without_session_does_not_resolve_or_invent_context() -> Result<()> {
        let called = std::cell::Cell::new(false);
        run_with_history(
            None,
            || anyhow::bail!("context must not be requested"),
            || {
                called.set(true);
                Ok(())
            },
        )?;
        assert!(called.get());
        Ok(())
    }
}

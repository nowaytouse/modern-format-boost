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
    append_jsonl_audit_record, archive_drag_drop_session_bundle_with_label, ensure_unified_log_dir,
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
    project_label: Option<String>,
    run_id: Option<String>,
    tool_snapshots: Arc<ToolSnapshotBindings>,
}

static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FastImgRetryReused {
    schema_version: u8,
    parent_task_key: String,
    request_key: String,
    child_working_copy: PathBuf,
    newly_processed: u64,
    newly_delivered: u64,
    newly_saved_bytes: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FastImgRetryFinalized {
    schema_version: u8,
    parent_task_key: String,
    request_key: String,
    child_working_copy: PathBuf,
    newly_processed: u64,
    newly_delivered: u64,
    newly_saved_bytes: u64,
    evidence: foundation::pipeline::verification::FailedRetryFinishEvidence,
}

/// The exact request and Core state observed before starting its worker.
pub struct FastImgRetryRequest {
    run_id: String,
    parent: foundation::fast_img_store::FailedRetryParent,
    delivery: foundation::fast_img_store::FailedRetryDeliveryIntent,
    input: PathBuf,
    before: Option<foundation::fast_img_store::FailedRetryFinishSnapshot>,
}

impl FastImgRetryRequest {
    fn reference(&self) -> serde_json::Value {
        let mut value = failed_retry_reference(&self.parent, &self.delivery);
        value["input_path"] = serde_json::json!(self.input);
        value["child_snapshot_before"] =
            self.before
                .as_ref()
                .map_or(serde_json::Value::Null, |before| {
                    serde_json::json!({"snapshot_blake3":before.snapshot_blake3(),
                "revision":before.revision(),"manifest_blake3":before.manifest_blake3(),
                "task_key":before.task_key()})
                });
        value
    }
}

fn failed_retry_reference(
    parent: &foundation::fast_img_store::FailedRetryParent,
    delivery: &foundation::fast_img_store::FailedRetryDeliveryIntent,
) -> serde_json::Value {
    serde_json::json!({"schema_version":1,"parent_task_key":parent.task_key(),
        "request_key":parent.request_key(),"parent_working_copy":parent.marker().working_copy,
        "child_working_copy":parent.child_working_copy(),"source_directory":parent.marker().src_dir,
        "strategy":parent.marker().strategy,"delivery_intent":delivery})
}

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
            project_label: None,
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

    /// Record the selected immutable parent before the worker can create a child.
    ///
    /// # Errors
    /// Propagates missing run identity and history persistence errors.
    pub fn record_fast_img_failed_retry_request(
        &self,
        parent: &foundation::fast_img_store::FailedRetryParent,
        delivery: &foundation::fast_img_store::FailedRetryDeliveryIntent,
        input: &Path,
    ) -> Result<FastImgRetryRequest> {
        let _ = foundation::fast_img_store::find_failed_retry_child(parent, delivery)?;
        let input = std::path::absolute(input)?;
        anyhow::ensure!(
            !input.components().any(|component| matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )) && (input == parent.marker().src_dir
                || (parent.marker().failed_sources.len() == 1
                    && parent
                        .marker()
                        .failed_sources
                        .keys()
                        .next()
                        .is_some_and(|relative| parent.marker().src_dir.join(relative) == input))),
            "retry request input does not match the selected parent's exact saved scope"
        );
        let request = FastImgRetryRequest {
            run_id: self
                .run_id
                .as_ref()
                .context("retry request has no run identity")?
                .clone(),
            parent: parent.clone(),
            delivery: delivery.clone(),
            input,
            before: foundation::fast_img_store::inspect_failed_retry_finish(
                parent.child_working_copy(),
            )?,
        };
        let mut report = request.reference();
        report["run_id"] = serde_json::json!(
            self.run_id
                .as_ref()
                .context("failed-only request has no run identity")?
        );
        self.append_history_event("MFB_HISTORY_FAST_IMG_FAILED_RETRY_REQUEST", report.clone())?;
        writeln!(
            io::stdout().lock(),
            "MFB_FAST_IMG_FAILED_RETRY_REQUEST={report}"
        )?;
        Ok(request)
    }

    /// Confirm either a historical reference or fresh finish-only evidence against Core.
    /// Both have zero new media work; only finalization carries current custody observations.
    ///
    /// # Errors
    /// Rejects conflicting worker records, counts, request identity or Core state.
    pub fn record_fast_img_retry_outcome(
        &self,
        text: &str,
        stats: &ProcessorStats,
        request: Option<&FastImgRetryRequest>,
    ) -> Result<bool> {
        const REUSED: &str = "MFB_FAST_IMG_RETRY_REUSED=";
        const FINALIZED: &str = "MFB_FAST_IMG_RETRY_FINALIZED=";
        let mut lines = text
            .lines()
            .filter(|line| line.contains(REUSED) || line.contains(FINALIZED));
        let Some(line) = lines.next() else {
            return Ok(false);
        };
        let (encoded, finalized) = if let Some(encoded) = line.strip_prefix(FINALIZED) {
            (encoded, true)
        } else {
            (
                line.strip_prefix(REUSED)
                    .context("retry outcome has an invalid prefix")?,
                false,
            )
        };
        anyhow::ensure!(
            encoded.len() <= 16_384
                && lines.next().is_none()
                && !text.contains("MFB_FAST_IMG_RESULT=")
                && !text.contains("MFB_CONVERTED_BYTES="),
            "retry outcome is oversized, duplicated or mixed with fresh delivery evidence"
        );
        let (receipt, evidence) = if finalized {
            let value: FastImgRetryFinalized = serde_json::from_str(encoded)?;
            (
                FastImgRetryReused {
                    schema_version: value.schema_version,
                    parent_task_key: value.parent_task_key,
                    request_key: value.request_key,
                    child_working_copy: value.child_working_copy,
                    newly_processed: value.newly_processed,
                    newly_delivered: value.newly_delivered,
                    newly_saved_bytes: value.newly_saved_bytes,
                },
                Some(value.evidence),
            )
        } else {
            (serde_json::from_str::<FastImgRetryReused>(encoded)?, None)
        };
        let request =
            request.context("retry outcome was not requested by this Fast IMG invocation")?;
        anyhow::ensure!(
            self.run_id.as_ref() == Some(&request.run_id),
            "retry outcome belongs to a different execution attempt"
        );
        let parent = &request.parent;
        let delivery = &request.delivery;
        anyhow::ensure!(
            receipt.schema_version == 1
                && receipt.newly_processed == 0
                && receipt.newly_delivered == 0
                && receipt.newly_saved_bytes == 0
                && receipt.parent_task_key == parent.task_key()
                && receipt.request_key == parent.request_key()
                && receipt.child_working_copy == parent.child_working_copy()
                && stats.exit_code == 0
                && stats.counts_complete()
                && !stats.converted_bytes_invalid
                && stats.converted_bytes.is_none()
                && stats.total() == 0
                && stats.unprocessed == Some(0),
            "retry outcome disagrees with the selected request or current worker outcome"
        );
        let child = foundation::fast_img_store::find_failed_retry_child(parent, delivery)?
            .context("reused retry record has no durable child")?;
        anyhow::ensure!(
            child.working_copy == receipt.child_working_copy && child.is_terminal(),
            "reused retry record does not name its completed durable child"
        );
        let context = foundation::fast_img_store::failed_retry_context(&child.working_copy)?
            .context("reused child lost its immutable parent relation")?;
        anyhow::ensure!(
            context.parent_task_key() == parent.task_key()
                && context.requested_delivery() == delivery,
            "reused child delivery intent or parent relation differs from the current request"
        );
        if let Some(evidence) = &evidence {
            let before = request
                .before
                .as_ref()
                .context("finalized retry has no pre-worker Core snapshot")?;
            anyhow::ensure!(
                child.failed_retry_finish_evidence() == Some(evidence)
                    && evidence.task_key == before.task_key()
                    && evidence.request_key == before.request_key()
                    && evidence.manifest_blake3 == before.manifest_blake3()
                    && evidence.verified_snapshot_blake3 == before.snapshot_blake3()
                    && evidence.verified_revision == before.revision()
                    && evidence.verified_stage == before.marker().stage
                    && evidence.verified_tier2_in_progress == before.marker().tier2_in_progress
                    && evidence.verified_error == before.marker().error,
                "finalized retry does not match its pre-worker snapshot and durable Core evidence"
            );
        }
        let mut report = request.reference();
        report["run_id"] = serde_json::json!(
            self.run_id
                .as_ref()
                .context("reused retry has no run identity")?
        );
        for key in ["newly_processed", "newly_delivered", "newly_saved_bytes"] {
            report[key] = serde_json::json!(0);
        }
        if let Some(evidence) = evidence {
            report["evidence"] = serde_json::to_value(evidence)?;
        }
        let kind = if finalized { "FINALIZED" } else { "REUSED" };
        self.append_history_event(
            &format!("MFB_HISTORY_FAST_IMG_RETRY_{kind}"),
            report.clone(),
        )?;
        writeln!(
            io::stdout().lock(),
            "MFB_FAST_IMG_RETRY_{kind}_CONFIRMED={report}"
        )?;
        Ok(true)
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

    /// Label the archive from the selected input without renaming authoritative logs.
    pub fn set_project_label(&mut self, target: &Path) {
        self.project_label = Some(target.file_name().map_or_else(
            || "project".to_string(),
            |n| n.to_string_lossy().into_owned(),
        ));
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
        let archived = archive_drag_drop_session_bundle_with_label(
            &self.log_dir,
            &self.stamp,
            Some(&self.session_log),
            Some(&self.verbose_log),
            Some(&self.session_audit),
            Some(self.started_at),
            self.project_label.as_deref(),
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
    fn project_label_archives_canonical_session_artifacts_without_worker_output() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let logs = temp.path().join("logs");
        let mut session = AppSession::start_in(&logs)?;
        let canonical = session.session_log.clone();
        let project = temp.path().join("合成媒体");
        fs::create_dir(&project)?;
        session.set_project_label(&project);
        let bundle = session.archive()?.context("session bundle missing")?;
        assert_eq!(
            bundle.file_name().and_then(|name| name.to_str()),
            Some(format!("合成媒体_Bundle_{}", session.stamp).as_str()),
        );
        assert_eq!(session.session_log, canonical);
        let names = [
            format!("MFB_Session_{}.log", session.stamp),
            format!("verbose_{}.log", session.stamp),
            format!("session_audit_{}.jsonl", session.stamp),
        ];
        for name in &names {
            assert!(bundle.join(name).is_file(), "{name}");
            assert!(!logs.join(name).exists(), "{name}");
        }
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(bundle.join("manifest.json"))?)?;
        let files = manifest["files"]
            .as_array()
            .context("manifest files missing")?;
        assert_eq!(files.len(), names.len());
        for name in names {
            assert!(files.contains(&serde_json::Value::String(name)));
        }
        assert!(
            fs::read_to_string(
                bundle.join(
                    canonical
                        .file_name()
                        .context("canonical log name missing")?
                )
            )?
            .contains("SESSION_STARTED")
        );
        Ok(())
    }

    #[test]
    fn finalized_retry_requires_the_pre_worker_snapshot_and_exact_durable_receipt() -> Result<()> {
        let thread = std::thread::current();
        let name = thread.name().context("test name unavailable")?;
        if std::env::var("MFB_SESSION_STATE_TEST").as_deref() != Ok(name) {
            let output = std::process::Command::new(std::env::current_exe()?)
                .args([name, "--exact", "--nocapture", "--test-threads=1"])
                .env("MFB_SESSION_STATE_TEST", name)
                .output()?;
            anyhow::ensure!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
                "isolated session-state regression failed: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        use foundation::fast_img_store::{
            FailedRetryDeliveryIntent, capture_failed_retry_admissions,
            create_or_resume_failed_retry, finalize_failed_retry_finish,
            inspect_failed_retry_finish, inspect_failed_retry_parent,
        };
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName,
            Gate1Checks, SkippedSourceEntry, WorkingCopyMarker, write_marker_atomic,
        };
        let temp = tempfile::tempdir()?;
        let _state = foundation::common_utils::EnvGuard::set(
            foundation::constants::ENV_MFB_HOME_ROOT,
            temp.path().join("state").to_str().unwrap(),
        );
        let root = temp.path().canonicalize()?;
        let source = root.join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("failed.jpg"), b"synthetic retained source")?;
        let source_hash =
            foundation::common_utils::calculate_blake3_hash(&source.join("failed.jpg"))?;
        let mut parent = WorkingCopyMarker::new(source.clone(), root.join("source_optimized"), 1);
        parent.stage = FastImgStageName::CleanupComplete;
        parent.processing_contract = Some(FastImgProcessingContract {
            schema: 1,
            tool_policy: foundation::runtime_config::ToolPolicy::Single,
            fallback_policy: foundation::runtime_config::FallbackPolicy::Strict,
            encoding: FastImgEncodingParameters::Jxl {
                jpeg_effort: 11,
                jpeg_effort_search: foundation::runtime_config::JpegEffortSearch::Full,
                archive: true,
                allow_expert_options: false,
            },
        });
        parent.failed_sources.insert(
            "failed.jpg".into(),
            SkippedSourceEntry {
                src: source_hash.clone(),
                reason: "synthetic failure".into(),
            },
        );
        write_marker_atomic(&parent)?;
        let reference = inspect_failed_retry_parent(&parent.working_copy)?;
        let delivery = FailedRetryDeliveryIntent::Local;
        let mut child = create_or_resume_failed_retry(
            &reference,
            &capture_failed_retry_admissions(&reference)?,
            &delivery,
            None,
        )?;
        fs::create_dir(&child.working_copy)?;
        // This models a prior verified Gate, testing the request/receipt binding.
        // It is not a media decode test or proof of a newly executed Gate.
        let output = [0xff, 0x0a, 0x31, 0x45, 0x59, 0x26];
        fs::write(child.working_copy.join("failed.JXL"), output)?;
        child.blake3_log.insert(
            "failed.jpg".into(),
            Blake3Entry {
                src: source_hash,
                out: blake3::hash(&output).to_hex().to_string(),
                out_rel: Some("failed.JXL".into()),
                library_asset: None,
            },
        );
        child.encoded_count = 1;
        child.stage = FastImgStageName::Gate1Passed;
        child.gate1_checks = Gate1Checks {
            count: true.into(),
            blake3: true.into(),
            metadata: true.into(),
            size: true.into(),
            orient: true.into(),
            decode: true.into(),
        };
        write_marker_atomic(&child)?;
        let session = AppSession::start_in(&root.join("logs"))?.for_run()?;
        let request =
            session.record_fast_img_failed_retry_request(&reference, &delivery, &source)?;
        let snapshot = inspect_failed_retry_finish(&child.working_copy)?
            .context("fixture snapshot missing")?;
        fs::remove_dir_all(&source)?;
        let stats = ProcessorStats {
            reported: [true; 4],
            unprocessed: Some(0),
            ..ProcessorStats::default()
        };
        let finished = finalize_failed_retry_finish(
            &snapshot,
            foundation::fast_img::verify_failed_retry_finish(&snapshot)?,
        )?;
        let payload = serde_json::json!({"schema_version":1,"parent_task_key":reference.task_key(),
            "request_key":reference.request_key(),"child_working_copy":child.working_copy,
            "newly_processed":0,"newly_delivered":0,"newly_saved_bytes":0,"evidence":finished.failed_retry_finish_evidence()});
        let line = format!("MFB_FAST_IMG_RETRY_FINALIZED={payload}");
        assert!(
            session
                .record_fast_img_retry_outcome(&line, &stats, None)
                .is_err()
        );
        assert!(
            session
                .for_run()?
                .record_fast_img_retry_outcome(&line, &stats, Some(&request))
                .is_err()
        );
        let late = session.record_fast_img_failed_retry_request(&reference, &delivery, &source)?;
        assert!(
            session
                .record_fast_img_retry_outcome(&line, &stats, Some(&late))
                .is_err()
        );
        let mut altered = payload.clone();
        altered["evidence"]["verified_revision"] = serde_json::json!(snapshot.revision() + 1);
        assert!(
            session
                .record_fast_img_retry_outcome(
                    &format!("MFB_FAST_IMG_RETRY_FINALIZED={altered}"),
                    &stats,
                    Some(&request)
                )
                .is_err()
        );
        let mut mixed = payload.clone();
        mixed.as_object_mut().unwrap().remove("evidence");
        assert!(
            session
                .record_fast_img_retry_outcome(
                    &format!("{line}\nMFB_FAST_IMG_RETRY_REUSED={mixed}"),
                    &stats,
                    Some(&request)
                )
                .is_err()
        );
        let mut nonzero = stats.clone();
        nonzero.succeeded = 1;
        assert!(
            session
                .record_fast_img_retry_outcome(&line, &nonzero, Some(&request))
                .is_err()
        );
        assert!(session.record_fast_img_retry_outcome(&line, &stats, Some(&request))?);
        let conn = rusqlite::Connection::open(session.log_dir().join("history.sqlite3"))?;
        let event:String=conn.query_row("SELECT event FROM history_events WHERE session_id=?1 AND event LIKE 'MFB_HISTORY_FAST_IMG_RETRY_FINALIZED=%'",[session.stamp()],|row|row.get(0))?;
        let report: serde_json::Value = serde_json::from_str(event.split_once('=').unwrap().1)?;
        assert_eq!(report["input_path"], serde_json::json!(source));
        assert_eq!(report["evidence"]["verified_output_count"], 1);
        assert_eq!(report["newly_processed"], 0);
        assert_eq!(
            foundation::pipeline::verification::read_marker(&parent.working_copy)?,
            parent
        );
        assert_eq!(fs::read(child.working_copy.join("failed.JXL"))?, output);
        assert!(!source.exists());
        Ok(())
    }

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

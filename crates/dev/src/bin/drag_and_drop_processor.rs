//! Modern Format Boost - Drag/Drop Processor (Rust primary;
//! `drag_and_drop_processor.py` retained as compat reference). PTY/process
//! streaming, watch mode, interactive TUI menu, handoff preserve — parity with
//! Python launcher.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local};
use clap::{Parser, ValueEnum};
use dev::infra::drag_drop::{
    ContentScan, FastImgAction, ProcessingFilter, acquire_global_lock, adjacent_output_for_target,
    build_size_comparison_summary, choose_fast_img_action, choose_fast_vid_shortest_path,
    confirm_in_place, count_fast_img_jxl_outputs, create_directory_structure,
    delete_fast_img_shortest_path_output_dir, effective_success_failure_counts,
    fast_img_integrity_counts, fast_img_marker_requires_retry, fast_img_restore_integrity_counts,
    fast_img_retained_file_names, fast_img_session_size_metrics, get_unique_output_path,
    run_unified_verification, safety_check, scan_content, sync_non_media_files,
};
use dev::infra::elapsed_spinner::{print_elapsed, update_terminal_title};
use dev::infra::fastmode_paths::{
    build_fast_img_command, build_fast_img_restore_command, build_fast_vid_command,
    fast_img_restore_output_dir_for_target, fast_vid_output_dir_for_target,
};
use dev::infra::hardening::delegated_exit_code;
use dev::infra::log_paths::{
    append_jsonl_audit_record, archive_drag_drop_session_bundle, ensure_unified_log_dir,
    format_session_stamp,
};
use dev::infra::process_stream::{ProcessorStats, stream_process_with_pty};
use dev::infra::rich_panel::{
    PipelineSummary, RuntimeDashboard, clear_screen, draw_banner, draw_separator,
    pause_before_gui_exit, print_critical_error_panel, print_menu_hint, print_menu_row,
    print_runtime_panel, print_summary_report,
};
use dev::infra::signal_handlers::{install_signal_handlers, set_child_active};
use dev::infra::terminal_input::{
    NavKey, drain_stdin, read_nav_key, resize_terminal_for_gui, validate_drag_drop_path,
};
use dev::infra::ui_tokens::{colors_enabled, pick_symbol};
use dev::infra::watch_mode::{is_watch_trigger_ext, watch_directory_with_debounce};

use dev::infra::system_checks::{check_system_resources, probe_system_snapshot};
use dev::media::scope::format_bytes;

use dev::media::scope::{
    HANDOFF_PRESERVE_PHASE_POST_IMG_VID, PIPELINE_IMAGE, PIPELINE_VIDEO, classify_media_owner,
    extract_routed_video_paths_from_audit, preserve_handoff_gaps,
    report_handoff_preserve_gaps_from_paths,
};
use foundation::process_lock::DirLock;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use walkdir::WalkDir;

// ── Error mode constants (mirrors Python MFB_DRAG_DROP_ERROR_MODE)
// ────────────
const DRAG_DROP_ERROR_MODE_ENV: &str = "MFB_DRAG_DROP_ERROR_MODE";
const DRAG_DROP_FAIL_FAST_ENV: &str = "MFB_DRAG_DROP_FAIL_FAST";
const DRAG_DROP_ERROR_MODE_FAIL_FAST: &str = "fail-fast";
const DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE: &str = "log-and-continue";
const DRAG_DROP_CHILD_ULTIMATE: bool = true;
const DRAG_DROP_CHILD_VERBOSE: bool = true;

#[derive(Clone, Debug, PartialEq, Eq, ValueEnum)]
enum LaunchMode {
    Auto,
    Images,
    Videos,
    FastImg,
    RestoreJpeg,
    FastVid,
    Collect,
    MergeXmp,
    IcloudImport,
    Diagnostic,
    CacheClean,
    DatabaseManager,
}

#[derive(Parser, Debug, Clone)]
#[command(
    name = "drag_and_drop_processor",
    about = "Modern Format Boost CLI-first drag/drop launcher"
)]
struct Args {
    #[arg(value_name = "INPUT")]
    inputs: Vec<PathBuf>,

    #[arg(long, value_enum, default_value_t = LaunchMode::Auto)]
    mode: LaunchMode,

    #[arg(short, long)]
    output: Option<PathBuf>,

    #[arg(long)]
    archive: bool,

    #[arg(long = "shortest-path")]
    shortest_path: bool,

    #[arg(long)]
    retry: bool,

    #[arg(short, long)]
    force: bool,

    #[arg(long)]
    dry_run: bool,

    #[arg(long)]
    plain: bool,

    #[arg(long)]
    vue: bool,

    #[arg(long)]
    resume: bool,

    #[arg(long)]
    ultimate: bool,

    #[arg(long)]
    in_place: bool,

    #[arg(short, long)]
    verbose: bool,

    #[arg(long)]
    base_dir: Option<PathBuf>,

    /// Watch input directory for new media (debounced re-run)
    #[arg(long)]
    watch: bool,

    /// Only process static images (maps to `--mode images`)
    #[arg(long, conflicts_with = "videos_only")]
    images_only: bool,

    /// Only process videos/animated media (maps to `--mode videos`)
    #[arg(long, conflicts_with = "images_only")]
    videos_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LaunchCommand {
    program: PathBuf,
    args: Vec<String>,
}

struct DragDropSession {
    started_at: DateTime<Local>,
    stamp: String,
    log_dir: PathBuf,
    session_log: PathBuf,
    verbose_log: PathBuf,
    session_audit: PathBuf,
}

impl DragDropSession {
    fn start() -> Result<Self> {
        let log_dir = ensure_unified_log_dir()?;
        Self::start_with_log_dir(&log_dir, true)
    }

    #[cfg(test)]
    fn start_for_test(log_dir: &Path) -> Result<Self> {
        Self::start_with_log_dir(log_dir, false)
    }

    fn start_with_log_dir(log_dir: &Path, set_child_env: bool) -> Result<Self> {
        fs::create_dir_all(log_dir)
            .with_context(|| format!("create drag/drop log dir {}", log_dir.display()))?;
        let log_dir = log_dir
            .canonicalize()
            .with_context(|| format!("canonicalize drag/drop log dir {}", log_dir.display()))?;
        let started_at = Local::now();
        let stamp = format_session_stamp(Some(started_at));
        let session = Self {
            started_at,
            session_log: log_dir.join(format!("MFB_Session_{stamp}.log")),
            verbose_log: log_dir.join(format!("verbose_{stamp}.log")),
            session_audit: log_dir.join(format!("session_audit_{stamp}.jsonl")),
            log_dir,
            stamp,
        };
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
        Ok(session)
    }

    fn append_line(&self, path: &Path, event: &str) -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open drag/drop log {}", path.display()))?;
        let stamp = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        writeln!(file, "[{stamp}] {event}")
            .with_context(|| format!("write drag/drop log {}", path.display()))
    }

    /// Rename session log to include project folder name (mirrors Python
    /// `rename_log_to_project`).
    fn rename_log_to_project(&mut self, target: &Path) -> Result<()> {
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

    /// Write final statistics block to session log (mirrors Python
    /// `finish_log`).
    fn finish_log(&self, summary: &PipelineSummary, size_summary: Option<&str>) -> Result<()> {
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
        writeln!(
            file,
            "Images:  {} succeeded, {} skipped, {} failed",
            summary.img.succeeded, summary.img.skipped, summary.img.failed
        )?;
        writeln!(
            file,
            "Videos:  {} succeeded, {} skipped, {} failed",
            summary.vid.succeeded, summary.vid.skipped, summary.vid.failed
        )?;
        let tot_s = summary.total_succeeded();
        let tot_sk = summary.img.skipped + summary.vid.skipped;
        let tot_f = summary.total_failed();
        let tot_proc = tot_s + tot_sk + tot_f;
        let (effective_s, effective_f, integrity_penalty) = effective_success_failure_counts(
            tot_s,
            tot_f,
            summary.integrity_state.map(|s| s == "WARNINGS"),
            summary.integrity_issue_count,
        );
        writeln!(
            file,
            "Total:   {effective_s} succeeded, {tot_sk} skipped, {effective_f} failed"
        )?;
        if integrity_penalty > 0 {
            writeln!(
                file,
                "Adjusted: raw failures={tot_f}, integrity penalty={integrity_penalty}"
            )?;
        }
        if let Some(rate) = (effective_s * 100).checked_div(tot_proc) {
            writeln!(file, "Success Rate: {rate}%")?;
        }
        if let Some(state) = summary.integrity_state {
            writeln!(file, "Integrity: {state}")?;
        }
        if !summary.failed_file_names.is_empty() || !summary.skipped_file_names.is_empty() {
            writeln!(file, "Retained files (source preserved):")?;
            for name in &summary.failed_file_names {
                writeln!(file, "  [FAIL] {name}")?;
            }
            for name in &summary.skipped_file_names {
                writeln!(file, "  [SKIP] {name}")?;
            }
        }
        if let Some(block) = size_summary {
            writeln!(file, "{block}")?;
        }
        writeln!(
            file,
            "\n========================================\nSession \
             completed.\n========================================"
        )?;
        append_jsonl_audit_record(
            &self.session_audit,
            &format!(
                "SESSION_COMPLETED images_ok={} images_skip={} images_fail={} videos_ok={} \
                 videos_skip={} videos_fail={}",
                summary.img.succeeded,
                summary.img.skipped,
                summary.img.failed,
                summary.vid.succeeded,
                summary.vid.skipped,
                summary.vid.failed
            ),
        )?;
        let has_integrity_issues =
            summary.integrity_state == Some("WARNINGS") && summary.integrity_issue_count > 0;
        if tot_f > 0 || has_integrity_issues {
            eprintln!(
                "   {} Session log:  {}",
                pick_symbol("📋", "[LOG]"),
                self.session_log.display()
            );
        }
        Ok(())
    }

    fn archive(&self) -> Result<Option<PathBuf>> {
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
        }
        Ok(archived)
    }
}

impl LaunchCommand {
    fn from_argv(argv: Vec<String>) -> Result<Self> {
        let mut iter = argv.into_iter();
        let Some(program) = iter.next() else {
            bail!("internal launcher error: empty command argv");
        };
        Ok(Self {
            program: PathBuf::from(program),
            args: iter.collect(),
        })
    }

    fn display(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(self.program.display().to_string());
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }

    #[allow(dead_code)]
    fn run(&self, dry_run: bool) -> Result<()> {
        self.run_with_session(dry_run, None).map(|_| ())
    }

    fn run_with_session(
        &self,
        dry_run: bool,
        session: Option<&DragDropSession>,
    ) -> Result<ProcessorStats> {
        self.run_collecting(dry_run, session, true)
    }

    fn run_collecting(
        &self,
        dry_run: bool,
        session: Option<&DragDropSession>,
        bail_on_failure: bool,
    ) -> Result<ProcessorStats> {
        println!("+ {}", self.display());
        if dry_run {
            return Ok(ProcessorStats::default());
        }
        if self.program.components().count() > 1 && !self.program.is_file() {
            bail!(
                "Rust CLI binary missing: {}. Build it with `cargo build -p {}` first.",
                self.program.display(),
                self.program
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("dev"))
                    .to_string_lossy()
            );
        }
        if let Some(sess) = session {
            let pipeline = if self
                .program
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n == "img")
            {
                "IMG"
            } else if self
                .program
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n == "vid")
            {
                "VID"
            } else {
                "CMD"
            };
            append_jsonl_audit_record(
                &sess.session_audit,
                &format!("{pipeline}_PIPELINE_SPAWN cmd={}", self.display()),
            )?;
        }

        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd.env("COLUMNS", "223");
        cmd.env("LINES", "45");
        let stats = if let Some(sess) = session {
            let verbose = sess.verbose_log.clone();
            let session_log = sess.session_log.clone();
            let heartbeat = sess.session_audit.clone();
            let mut last_heartbeat = Instant::now();
            let argv = self.argv();
            set_child_active(true);
            let result = stream_process_with_pty(
                &argv,
                None,
                |line| {
                    println!("{line}");
                    let _ = io::stdout().flush();
                    let _ = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&verbose)
                        .and_then(|mut file| writeln!(file, "{line}"));
                    let _ = fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&session_log)
                        .and_then(|mut file| writeln!(file, "{line}"));
                },
                || {
                    if last_heartbeat.elapsed() >= std::time::Duration::from_mins(1) {
                        let _ = append_jsonl_audit_record(&heartbeat, "SESSION_HEARTBEAT");
                        last_heartbeat = Instant::now();
                    }
                },
            );
            set_child_active(false);
            result?
        } else {
            let mut child = cmd
                .spawn()
                .with_context(|| format!("launch {}", self.display()))?;
            let status = child
                .wait()
                .with_context(|| format!("wait {}", self.display()))?;
            ProcessorStats {
                exit_code: delegated_exit_code(
                    status,
                    &self.program.to_string_lossy(),
                    "LaunchCommand",
                ),
                ..ProcessorStats::default()
            }
        };
        if let Some(sess) = session {
            append_jsonl_audit_record(
                &sess.session_audit,
                &format!(
                    "{}_PIPELINE_EXIT code={} succeeded={} skipped={} ignored={} failed={}",
                    if self
                        .program
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n == "img")
                    {
                        "IMG"
                    } else if self
                        .program
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n == "vid")
                    {
                        "VID"
                    } else {
                        "CMD"
                    },
                    stats.exit_code,
                    stats.succeeded,
                    stats.skipped,
                    stats.ignored,
                    stats.failed
                ),
            )?;
        }
        if stats.exit_code != 0 && bail_on_failure {
            bail!(
                "command failed with exit {}: {}",
                stats.exit_code,
                self.display()
            );
        }
        Ok(stats)
    }

    fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.to_string_lossy().into_owned());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

fn resolve_runtime_root() -> Result<PathBuf> {
    match project_root() {
        Ok(root) => Ok(root),
        Err(root_err) => {
            if let Some(bundle) = app_bundle_root() {
                Ok(bundle)
            } else {
                Err(root_err)
            }
        }
    }
}

fn app_bundle_root() -> Option<PathBuf> {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            eprintln!(
                "{} current_exe lookup failed while resolving app bundle root: {err}",
                pick_symbol("⚠️", "[WARN]")
            );
            return None;
        }
    };
    #[allow(unused_variables)]
    let exe_dir = exe.parent()?;
    #[cfg(target_os = "macos")]
    {
        let exe_dir_name = exe_dir.file_name().and_then(|n| n.to_str());
        if exe_dir_name == Some("Resources") || exe_dir_name == Some("MacOS") {
            return Some(exe_dir.to_path_buf());
        }
    }
    None
}

fn is_app_bundle_resource_root(root: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        let name = root.file_name().and_then(|n| n.to_str());
        matches!(name, Some("Resources" | "MacOS"))
            && root.parent().is_some_and(|parent| {
                parent.file_name().and_then(|n| n.to_str()) == Some("Contents")
            })
            && root
                .parent()
                .and_then(|contents| contents.parent())
                .is_some_and(|bundle| bundle.extension().and_then(|e| e.to_str()) == Some("app"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = root;
        false
    }
}

fn project_root() -> Result<PathBuf> {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(err) => {
            bail!("cannot locate Modern Format Boost workspace: current_exe failed: {err}")
        }
    };

    if let Some(mut dir) = exe.parent() {
        for _ in 0..3 {
            if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
                return Ok(dir.to_path_buf());
            }
            dir = match dir.parent() {
                Some(parent) => parent,
                None => break,
            };
        }
    }

    // Fallback to cwd search
    let cwd = std::env::current_dir().context("read current directory")?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Ok(dir.to_path_buf());
        }
        let Some(parent) = dir.parent() else {
            break;
        };
        dir = parent;
    }
    bail!(
        "cannot locate Modern Format Boost workspace root from {}",
        cwd.display()
    )
}

fn cli_binary(project_root: &Path, name: &str) -> PathBuf {
    if is_app_bundle_resource_root(project_root) {
        // When running from an app bundle, binaries are packaged alongside the
        // executable.
        project_root.join(name)
    } else {
        // smart_build always produces release binaries, regardless of how
        // drag_and_drop_processor was compiled
        project_root.join("target").join("release").join(name)
    }
}

fn du_size_recursive(path: &Path) -> Result<u64> {
    let mut total: u64 = 0;
    if path.is_dir() {
        for entry in WalkDir::new(path) {
            match entry {
                Ok(e) => {
                    if e.path().is_file() {
                        match e.metadata() {
                            Ok(meta) => total += meta.len(),
                            Err(err) => eprintln!(
                                "[DRAG] du metadata failed ({}): {err}",
                                e.path().display()
                            ),
                        }
                    }
                }
                Err(err) => eprintln!("[DRAG] du walk failed under {}: {err}", path.display()),
            }
        }
    } else if path.is_file() {
        total = path.metadata()?.len();
    }
    Ok(total)
}

fn unique_adjacent_output(input: &Path, suffix: &str) -> PathBuf {
    let name = input
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("output"))
        .to_string_lossy();
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let base = parent.join(format!("{name}_{suffix}"));
    let mut candidate = base;
    let mut count = 2;
    while candidate.exists() {
        candidate = parent.join(format!("{name}_{suffix}_{count}"));
        count += 1;
    }
    candidate
}

const fn mode_needs_img_vid_binaries(mode: &LaunchMode) -> bool {
    matches!(
        mode,
        LaunchMode::Auto
            | LaunchMode::Images
            | LaunchMode::Videos
            | LaunchMode::FastImg
            | LaunchMode::RestoreJpeg
            | LaunchMode::FastVid
    )
}

const fn mode_needs_db_health(mode: &LaunchMode) -> bool {
    matches!(
        mode,
        LaunchMode::Auto | LaunchMode::Images | LaunchMode::Videos
    )
}

// ── safety checks (ported from drag_and_drop_processor.py) ──────────────────

/// Resolve the current drag/drop error mode from the environment.
///
/// Mirrors `drag_drop_error_mode()` in the Python implementation.
/// Checks `MFB_DRAG_DROP_FAIL_FAST` (legacy) first, then
/// `MFB_DRAG_DROP_ERROR_MODE`. Any unrecognised value falls through to
/// log-and-continue.
fn drag_drop_error_mode() -> &'static str {
    fn truthy(v: &str) -> bool {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    }

    // Legacy fast-fail env (backwards compat with Python)
    let val = match std::env::var(DRAG_DROP_FAIL_FAST_ENV) {
        Ok(value) if !value.trim().is_empty() => value,
        _ => String::new(),
    };
    if truthy(&val) {
        return DRAG_DROP_ERROR_MODE_FAIL_FAST;
    }

    let raw = match std::env::var(DRAG_DROP_ERROR_MODE_ENV) {
        Ok(value) => value.trim().to_ascii_lowercase(),
        Err(_) => String::new(),
    };
    let normalized = raw.replace(['_', ' '], "-");
    match normalized.as_str() {
        "fail-fast" | "failfast" | "abort" | "strict" => DRAG_DROP_ERROR_MODE_FAIL_FAST,
        "" | "continue" | "log-and-continue" | "batch-report" | "report" | "normal" => {
            DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE
        }
        _ => {
            eprintln!(
                "{} Unknown {DRAG_DROP_ERROR_MODE_ENV}={raw:?}; falling back to \
                 {DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE}",
                pick_symbol("⚠️", "[WARN]")
            );
            DRAG_DROP_ERROR_MODE_LOG_AND_CONTINUE
        }
    }
}

/// Returns true when errors should abort immediately (fail-fast mode).
fn drag_drop_fail_fast_enabled() -> bool {
    drag_drop_error_mode() == DRAG_DROP_ERROR_MODE_FAIL_FAST
}

fn read_optional_text_file(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            eprintln!(
                "[DRAG] optional env read failed ({}): {err}",
                path.display()
            );
            None
        }
    }
}

/// Load `.modern_format_boost/local_env.json` (and `.sh` fallback) into
/// `std::env`.  Mirrors `load_local_env()` in the Python implementation.
///
/// Uses `serde_json` for robust JSON parsing — the previous hand-rolled
/// `split(',')` approach silently mishandled values containing commas (e.g.
/// `PostgreSQL` multi-host connection strings).
fn load_local_env(project_root: &Path) {
    // JSON source (preferred)
    let json_path = project_root
        .join("crates/.modern_format_boost")
        .join("local_env.json");
    if json_path.is_file() {
        if let Some(content) = read_optional_text_file(&json_path) {
            match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content) {
                Ok(map) => {
                    for (key, val) in &map {
                        if key.is_empty() {
                            continue;
                        }
                        // Coerce all scalar types to strings, matching Python behaviour.
                        let str_val = match val {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        // SAFETY: single-threaded startup; no concurrent env reads.
                        unsafe { std::env::set_var(key, &str_val) };
                    }
                }
                Err(err) => {
                    eprintln!(
                        "{} local_env.json parse error (skipping): {err}",
                        pick_symbol("⚠️", "[WARN]")
                    );
                }
            }
        }
        return; // JSON wins; skip .sh fallback
    }

    // Shell fallback: `export KEY="VALUE"`
    let sh_path = project_root
        .join("crates/.modern_format_boost")
        .join("local_env.sh");
    if sh_path.is_file()
        && let Some(content) = read_optional_text_file(&sh_path)
    {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || !line.contains("export ") {
                continue;
            }
            let stripped = line.replacen("export ", "", 1);
            let parts: Vec<&str> = stripped.splitn(2, '=').collect();
            if parts.len() == 2 {
                let key = parts[0].trim();
                let val = parts[1].trim().trim_matches('"').trim_matches('\'');
                if !key.is_empty() {
                    // SAFETY: single-threaded startup; no concurrent env reads.
                    unsafe { std::env::set_var(key, val) };
                }
            }
        }
    }
}

/// Call `img db-health` and fail-fast with a helpful error if `PostgreSQL` is
/// unreachable.  Mirrors `verify_database_mandatory()` in the Python impl.
fn verify_database_mandatory(project_root: &Path) -> Result<()> {
    let img_bin = cli_binary(project_root, "img");
    if !img_bin.is_file() {
        // Binary absent — skip; ensure_tools_ready will handle the build.
        return Ok(());
    }
    let out = Command::new(&img_bin)
        .arg("db-health")
        .output()
        .context("run img db-health")?;
    if !out.status.success() {
        let diag = String::from_utf8_lossy(&out.stderr);
        let diag = if diag.trim().is_empty() {
            String::from_utf8_lossy(&out.stdout).into_owned()
        } else {
            diag.into_owned()
        };
        eprintln!();
        eprintln!(
            "{} MANDATORY DATABASE CONNECTION FAILED",
            pick_symbol("❌", "[ERROR]")
        );
        eprintln!(
            "  Modern Format Boost requires a PostgreSQL backend for full forensic accuracy."
        );
        eprintln!();
        eprintln!("  HOW TO FIX:");
        eprintln!("  1. Ensure PostgreSQL is running locally.");
        eprintln!("  2. Run the private setup helper:");
        eprintln!("       rust-script crates/dev/src/bin/setup_private_db.rs");
        eprintln!("  3. Or create: crates/.modern_format_boost/local_env.json");
        eprintln!("     with: {{\"MFB_PG_CONNSTR\": \"postgresql://user:pass@localhost/db\"}}");
        if !diag.trim().is_empty() {
            eprintln!("  Diagnostic: {}", diag.trim());
        }
        bail!("database health check failed");
    }

    Ok(())
}

/// Trigger `smart_build` if the required `img`/`vid` release binaries are
/// absent.  Mirrors `ensure_tools_ready()` in the Python implementation.
fn ensure_tools_ready(project_root: &Path, mode: &LaunchMode) -> Result<()> {
    let img_bin = cli_binary(project_root, "img");
    let vid_bin = cli_binary(project_root, "vid");
    let needs_img = !matches!(mode, LaunchMode::Videos | LaunchMode::FastVid);
    let needs_vid = !matches!(
        mode,
        LaunchMode::Images | LaunchMode::FastImg | LaunchMode::RestoreJpeg
    );

    if is_app_bundle_resource_root(project_root) {
        // App bundle mode: binaries are pre-packaged. Just verify existence.
        if needs_img && !img_bin.is_file() {
            bail!("App bundle is missing img binary: {}", img_bin.display());
        }
        if needs_vid && !vid_bin.is_file() {
            bail!("App bundle is missing vid binary: {}", vid_bin.display());
        }
        return Ok(());
    }

    eprintln!(
        "{} Ensuring release binaries are up-to-date via smart_build…",
        pick_symbol("⚙️", "[BUILD]")
    );

    let use_rtk = std::process::Command::new("which")
        .arg("rtk")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let mut build_cmd = if use_rtk {
        let mut c = Command::new("rtk");
        c.arg("cargo");
        c
    } else {
        Command::new("cargo")
    };
    build_cmd.args([
        "run",
        "--release",
        "--locked",
        "-p",
        "dev",
        "--bin",
        "smart_build",
        "--",
        "--update",
    ]);
    if needs_img && !needs_vid {
        build_cmd.arg("--img");
    } else if needs_vid && !needs_img {
        build_cmd.arg("--vid");
    } else {
        build_cmd.arg("--all");
    }

    let status = build_cmd
        .current_dir(project_root)
        .status()
        .context("run smart_build via cargo")?;
    if !status.success() {
        bail!("smart_build failed — check the logs and retry");
    }

    if needs_img && !img_bin.is_file() {
        bail!(
            "smart_build completed but img binary is still missing: {}",
            img_bin.display()
        );
    }
    if needs_vid && !vid_bin.is_file() {
        bail!(
            "smart_build completed but vid binary is still missing: {}",
            vid_bin.display()
        );
    }

    Ok(())
}
const fn processing_filter(args: &Args) -> ProcessingFilter {
    if args.images_only {
        ProcessingFilter::ImagesOnly
    } else if args.videos_only {
        ProcessingFilter::VideosOnly
    } else {
        ProcessingFilter::Both
    }
}

const fn mode_uses_standard_pipeline(mode: &LaunchMode) -> bool {
    matches!(
        mode,
        LaunchMode::Auto | LaunchMode::Images | LaunchMode::Videos
    )
}

fn adjacent_output_dir(args: &Args) -> PathBuf {
    args.output.clone().unwrap_or_else(|| {
        unique_adjacent_output(args.inputs.first().expect("input required"), "optimized")
    })
}

fn plan_routed_pipeline_commands(
    args: &Args,
    project_root: &Path,
    scan: &ContentScan,
    fail_fast: bool,
) -> Result<Vec<LaunchCommand>> {
    if fail_fast {
        return plan_cli_invocations(args, project_root, None);
    }
    let target = args.inputs.first().context("input required")?;
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| adjacent_output_dir(args));
    let mut commands = Vec::new();
    let run_images = matches!(args.mode, LaunchMode::Auto | LaunchMode::Images)
        && scan.img_count > 0
        && processing_filter(args).accepts_image();
    let run_videos = matches!(args.mode, LaunchMode::Auto | LaunchMode::Videos)
        && scan.vid_count > 0
        && processing_filter(args).accepts_video();
    if run_images {
        for rel in &scan.routed_images {
            let mut file_args = args.clone();
            file_args.inputs = vec![target.join(rel)];
            file_args.output = Some(output.clone());
            file_args.base_dir = Some(target.clone());
            commands.push(rust_run_command(
                project_root,
                "img",
                &file_args,
                &file_args.inputs[0],
            ));
        }
    }
    if run_videos {
        for rel in &scan.routed_videos {
            let mut file_args = args.clone();
            file_args.inputs = vec![target.join(rel)];
            file_args.output = Some(output.clone());
            file_args.base_dir = Some(target.clone());
            commands.push(rust_run_command(
                project_root,
                "vid",
                &file_args,
                &file_args.inputs[0],
            ));
        }
    }
    if commands.is_empty() {
        return plan_cli_invocations(args, project_root, None);
    }
    Ok(commands)
}

fn run_post_adjacent_steps(
    args: &Args,
    project_root: &Path,
    session: &DragDropSession,
    scan: &ContentScan,
    summary: &mut PipelineSummary,
) -> Result<()> {
    let target = args.inputs.first().context("input required")?;
    let output = adjacent_output_dir(args);
    if !output.is_dir() {
        return Ok(());
    }
    finalize_handoff_preservation(target, &output, session)?;
    sync_non_media_files(
        target,
        &output,
        scan,
        &cli_binary(project_root, "img"),
        Some(&session.session_audit),
    )?;
    draw_separator("Auto Verification");
    println!(
        "   {} Running unified integrity verification via Rust verify...",
        pick_symbol("🔍", "[CHECK]")
    );
    let verify = run_unified_verification(
        &cli_binary(project_root, "verify"),
        target,
        Some(&output),
        processing_filter(args),
        Some(&session.verbose_log),
        &session.log_dir,
        &session.stamp,
        dev::infra::drag_drop::VerificationFlags {
            include_logs: true,
            auto_mode: true,
            fast_img: dev::infra::drag_drop::VerifyFastImgMode::None,
        },
    )?;
    if verify.exit_code != 0 {
        summary.integrity_state = Some("WARNINGS");
        summary.integrity_issue_count = summary.integrity_issue_count.max(1);
    } else {
        summary.integrity_state = verify
            .warnings
            .map(|w| if w { "WARNINGS" } else { "CLEAN" });
        summary.integrity_issue_count = verify.issue_count;
    }
    Ok(())
}

fn run_fast_img_post_success(
    args: &Args,
    project_root: &Path,
    session: &DragDropSession,
    summary: &mut PipelineSummary,
    output_dir: &Path,
) -> Result<()> {
    let verify_bin = cli_binary(project_root, "verify");
    let (delivered_count, delivered_size) = count_fast_img_jxl_outputs(output_dir)?;
    summary.img.succeeded = summary.img.succeeded.max(delivered_count);
    summary.img.skipped = 0;
    summary.img.ignored = 0;
    summary.img.failed = 0;
    summary.vid = ProcessorStats::default();
    summary.fast_img_size_after_override = summary
        .fast_img_session_output_bytes
        .or(Some(delivered_size));

    draw_separator("Auto Verification");
    println!(
        "   {} Running fast-img delivery verification via Rust verify...",
        pick_symbol("🔍", "[CHECK]")
    );
    let target = args.inputs.first().context("input required")?;
    let verify = run_unified_verification(
        &verify_bin,
        target,
        Some(output_dir),
        processing_filter(args),
        Some(&session.session_audit),
        &session.log_dir,
        &session.stamp,
        dev::infra::drag_drop::VerificationFlags {
            include_logs: false,
            auto_mode: true,
            fast_img: dev::infra::drag_drop::VerifyFastImgMode::Delivery,
        },
    )?;
    summary.integrity_state = verify
        .warnings
        .map(|w| if w { "WARNINGS" } else { "CLEAN" });
    summary.integrity_issue_count = verify.issue_count;
    if let Some(counts) = fast_img_integrity_counts(&verify.stdout) {
        summary.img.succeeded = counts.optimized_count;
        summary.img.skipped = counts.skipped_count;
        summary.img.failed = counts.failed_count;
    }
    if args.shortest_path && verify.warnings == Some(false) {
        match delete_fast_img_shortest_path_output_dir(output_dir, &verify_bin) {
            Ok(true) => {
                summary.fast_img_size_after_override = summary
                    .fast_img_session_output_bytes
                    .or(Some(delivered_size));
            }
            Ok(false) => {
                summary.integrity_state = Some("WARNINGS");
                summary.integrity_issue_count = summary.integrity_issue_count.max(1);
                eprintln!(
                    "   {} Shortest Path cleanup left residual files in {}",
                    pick_symbol("⚠️", "[WARN]"),
                    output_dir.display()
                );
            }
            Err(err) => {
                summary.integrity_state = Some("WARNINGS");
                summary.integrity_issue_count = summary.integrity_issue_count.max(1);
                eprintln!(
                    "   {} Shortest Path cleanup failed for {}: {err:#}",
                    pick_symbol("❌", "[ERROR]"),
                    output_dir.display()
                );
            }
        }
    }
    Ok(())
}

fn run_fast_img_restore_post_success(
    args: &Args,
    project_root: &Path,
    session: &DragDropSession,
    summary: &mut PipelineSummary,
    output_dir: &Path,
) -> Result<()> {
    draw_separator("Auto Verification");
    println!(
        "   {} Running fast-img restore verification via Rust verify...",
        pick_symbol("🔍", "[CHECK]")
    );
    let target = args.inputs.first().context("input required")?;
    let verify = run_unified_verification(
        &cli_binary(project_root, "verify"),
        target,
        Some(output_dir),
        processing_filter(args),
        Some(&session.session_audit),
        &session.log_dir,
        &session.stamp,
        dev::infra::drag_drop::VerificationFlags {
            include_logs: false,
            auto_mode: true,
            fast_img: dev::infra::drag_drop::VerifyFastImgMode::Restore,
        },
    )?;
    summary.integrity_state = verify
        .warnings
        .map(|w| if w { "WARNINGS" } else { "CLEAN" });
    summary.integrity_issue_count = verify.issue_count;
    if let Some(counts) = fast_img_restore_integrity_counts(&verify.stdout) {
        if summary.img.succeeded == 0
            && summary.img.skipped == 0
            && summary.img.ignored == 0
            && summary.img.failed == 0
        {
            summary.img.succeeded = counts.restored_jpeg_count;
        }
        let _ = counts.source_jxl_count;
    }
    summary.vid = ProcessorStats::default();
    Ok(())
}

fn run_fast_img_with_retry(
    args: &Args,
    project_root: &Path,
    session: &DragDropSession,
) -> Result<(ProcessorStats, PathBuf)> {
    let target = args.inputs.first().context("input required")?;
    let verify_bin = cli_binary(project_root, "verify");
    let output = args.output.clone().unwrap_or_else(|| {
        dev::infra::fastmode_paths::fast_img_output_dir_for_target(
            target,
            Some(&|dir: &Path| dev::infra::drag_drop::marker_exists(&verify_bin, dir)),
        )
    });
    let img_bin = cli_binary(project_root, "img");
    let mut retry = args.retry;
    if !retry && fast_img_marker_requires_retry(&verify_bin, &output)? {
        retry = true;
    }
    let command = LaunchCommand::from_argv(build_fast_img_command(
        &img_bin,
        target,
        args.shortest_path,
        true,
        retry,
    ))?;
    let stats = command.run_collecting(args.dry_run, Some(session), false)?;
    if stats.exit_code != 0 && !args.retry && fast_img_marker_requires_retry(&verify_bin, &output)?
    {
        eprintln!(
            "{} Recoverable failure detected, retrying automatically...",
            pick_symbol("🔄", "[RETRY]")
        );
        let retry_cmd = LaunchCommand::from_argv(build_fast_img_command(
            &img_bin,
            target,
            args.shortest_path,
            true,
            true,
        ))?;
        let retry_stats = retry_cmd.run_collecting(args.dry_run, Some(session), false)?;
        return Ok((retry_stats, output));
    }
    Ok((stats, output))
}

fn push_common_run_args(command: &mut Vec<String>, args: &Args, input: &Path) {
    command.push("run".to_string());
    command.push(input.to_string_lossy().into_owned());
    if let Some(output) = &args.output {
        command.push("--output".to_string());
        command.push(output.to_string_lossy().into_owned());
    }
    if let Some(base) = &args.base_dir {
        command.push("--base-dir".to_string());
        command.push(base.to_string_lossy().into_owned());
    } else if input.is_dir() {
        command.push("--base-dir".to_string());
        command.push(input.to_string_lossy().into_owned());
        command.push("--recursive".to_string());
    }
    if args.force {
        command.push("--force".to_string());
    }
    if args.archive {
        command.push("--archive".to_string());
    }
    if args.plain {
        command.push("--plain".to_string());
    }
    if args.resume {
        command.push("--resume".to_string());
    }
    if args.ultimate || DRAG_DROP_CHILD_ULTIMATE {
        command.push("--ultimate".to_string());
    }
    if args.in_place {
        command.push("--in-place".to_string());
    }
    if args.verbose || DRAG_DROP_CHILD_VERBOSE {
        command.push("--verbose".to_string());
    }
    command.push("--apple-compat".to_string());
}

fn rust_run_command(project_root: &Path, bin: &str, args: &Args, input: &Path) -> LaunchCommand {
    let mut command = vec![cli_binary(project_root, bin).to_string_lossy().into_owned()];
    push_common_run_args(&mut command, args, input);
    LaunchCommand::from_argv(command).expect("internal command must include binary")
}

fn plan_auto_file(project_root: &Path, args: &Args, input: &Path) -> Result<LaunchCommand> {
    match classify_media_owner(input)? {
        Some(owner) if owner == PIPELINE_IMAGE => {
            Ok(rust_run_command(project_root, "img", args, input))
        }
        Some(owner) if owner == PIPELINE_VIDEO => {
            Ok(rust_run_command(project_root, "vid", args, input))
        }
        Some(owner) => bail!("unsupported media owner `{owner}` for {}", input.display()),
        None => bail!("unsupported drag/drop input: {}", input.display()),
    }
}

fn dev_bin_command(project_root: &Path, bin: &str, args: Vec<String>) -> Result<LaunchCommand> {
    let mut launch_argv = Vec::with_capacity(args.len() + 1);
    launch_argv.push(cli_binary(project_root, bin).to_string_lossy().into_owned());
    launch_argv.extend(args);
    LaunchCommand::from_argv(launch_argv)
}

fn plan_cli_invocations(
    args: &Args,
    project_root: &Path,
    session: Option<&DragDropSession>,
) -> Result<Vec<LaunchCommand>> {
    if args.vue {
        bail!(
            "Vue prototype is scaffolding only; invoke it separately from crates/dev/src/vue \
             without processing files"
        );
    }
    if args.inputs.is_empty() {
        // Handled by main menu
        return Ok(Vec::new());
    }

    let img_bin = cli_binary(project_root, "img");
    let vid_bin = cli_binary(project_root, "vid");
    let mut commands = Vec::new();

    for input in &args.inputs {
        match args.mode {
            LaunchMode::Auto if input.is_dir() => {
                commands.push(rust_run_command(project_root, "img", args, input));
                commands.push(rust_run_command(project_root, "vid", args, input));
            }
            LaunchMode::Auto => commands.push(plan_auto_file(project_root, args, input)?),
            LaunchMode::Images => commands.push(rust_run_command(project_root, "img", args, input)),
            LaunchMode::Videos => commands.push(rust_run_command(project_root, "vid", args, input)),
            LaunchMode::FastImg => {
                commands.push(LaunchCommand::from_argv(build_fast_img_command(
                    &img_bin,
                    input,
                    args.shortest_path,
                    args.archive,
                    args.retry,
                ))?);
            }
            LaunchMode::RestoreJpeg => {
                let output = args
                    .output
                    .clone()
                    .unwrap_or_else(|| fast_img_restore_output_dir_for_target(input));
                commands.push(LaunchCommand::from_argv(build_fast_img_restore_command(
                    &img_bin, input, &output,
                ))?);
            }
            LaunchMode::FastVid => {
                let output = args
                    .output
                    .clone()
                    .unwrap_or_else(|| fast_vid_output_dir_for_target(input));
                commands.push(LaunchCommand::from_argv(build_fast_vid_command(
                    &vid_bin,
                    input,
                    &output,
                    args.shortest_path,
                ))?);
            }
            LaunchMode::Collect => {
                let output = args
                    .output
                    .clone()
                    .unwrap_or_else(|| unique_adjacent_output(input, "collected"));
                commands.push(dev_bin_command(
                    project_root,
                    "collect_optimized",
                    vec![
                        input.to_string_lossy().into_owned(),
                        output.to_string_lossy().into_owned(),
                    ],
                )?);
            }
            LaunchMode::MergeXmp => {
                commands.push(dev_bin_command(
                    project_root,
                    "merge_xmp",
                    vec![input.to_string_lossy().into_owned()],
                )?);
            }
            LaunchMode::IcloudImport => {
                commands.push(dev_bin_command(
                    project_root,
                    "icloud_import",
                    vec![input.to_string_lossy().into_owned()],
                )?);
            }
            LaunchMode::Diagnostic => {
                let mut verify_args =
                    vec!["--verify".to_string(), input.to_string_lossy().into_owned()];
                if let Some(output) = &args.output {
                    verify_args.push(output.to_string_lossy().into_owned());
                }
                verify_args.extend(["--mode".to_string(), "both".to_string()]);
                if let Some(s) = session {
                    verify_args.push("--session-audit".to_string());
                    verify_args.push(s.session_audit.to_string_lossy().into_owned());
                }
                commands.push(dev_bin_command(project_root, "verify", verify_args)?);
            }
            LaunchMode::CacheClean => {
                commands.push(dev_bin_command(project_root, "cache_cleaner", Vec::new())?);
            }
            LaunchMode::DatabaseManager => {
                commands.push(dev_bin_command(
                    project_root,
                    "database_manager",
                    Vec::new(),
                )?);
            }
        }
    }
    Ok(commands)
}

const fn mode_label(mode: &LaunchMode) -> &'static str {
    match mode {
        LaunchMode::Auto => "Standard Pipeline",
        LaunchMode::Images => "Images Only",
        LaunchMode::Videos => "Videos/Animated",
        LaunchMode::FastImg | LaunchMode::RestoreJpeg => "Fast Image",
        LaunchMode::FastVid => "Fast Video",
        LaunchMode::Collect => "Collect Optimized",
        LaunchMode::MergeXmp => "Merge XMP",
        LaunchMode::IcloudImport => "iCloud Import",
        LaunchMode::Diagnostic => "Diagnostic Verify",
        LaunchMode::CacheClean => "Cache Cleanup",
        LaunchMode::DatabaseManager => "Database Manager",
    }
}

const fn target_type_label(args: &Args) -> &'static str {
    if args.images_only {
        "Images Only"
    } else if args.videos_only {
        "Videos/Animated Media Only"
    } else {
        "Everything"
    }
}

fn build_runtime_dashboard(args: &Args) -> RuntimeDashboard {
    let target = args
        .inputs
        .first()
        .map_or_else(|| "(interactive)".to_string(), |p| p.display().to_string());
    let snapshot = args.inputs.first().map(|p| probe_system_snapshot(p));
    RuntimeDashboard {
        target_path: target,
        mode_label: mode_label(&args.mode).to_string(),
        target_type: target_type_label(args).to_string(),
        output_path: args.output.as_ref().map(|p| p.display().to_string()),
        ultimate: args.ultimate,
        watch: args.watch,
        cpu_percent: snapshot.as_ref().map(|s| s.cpu_percent),
        memory_percent: snapshot.as_ref().map(|s| s.memory_percent),
        disk_free_gb: snapshot.map(|s| {
            foundation::numeric_cast::u64_to_f64(s.disk_free_bytes) / (1024.0 * 1024.0 * 1024.0)
        }),
    }
}

fn command_phase_label(program: &Path, index: usize, total: usize) -> String {
    let name = program
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("worker");
    if total == 2 && index == 0 {
        return format!("Processing Images ({name})");
    }
    if total == 2 && index == 1 {
        return format!("Processing Videos ({name})");
    }
    format!("Processing ({name})")
}

struct CursorGuard;

impl Drop for CursorGuard {
    fn drop(&mut self) {
        dev::infra::elapsed_spinner::show_cursor();
    }
}

fn run_drag_drop(
    args: &Args,
    session: Option<&DragDropSession>,
    dir_lock: Option<&DirLock>,
) -> Result<()> {
    let root = resolve_runtime_root()?;
    let started = Instant::now();
    draw_banner(env!("CARGO_PKG_VERSION"));
    if !args.inputs.is_empty() {
        print_runtime_panel(&build_runtime_dashboard(args));
    }

    load_local_env(&root);

    if !args.inputs.is_empty() {
        safety_check(&args.inputs[0])
            .with_context(|| format!("safety check failed for {}", args.inputs[0].display()))?;
    }

    let scan =
        if !args.dry_run && !args.inputs.is_empty() && mode_uses_standard_pipeline(&args.mode) {
            Some(scan_content(
                &args.inputs[0],
                processing_filter(args),
                session.map(|s| s.session_audit.as_path()),
            )?)
        } else {
            None
        };

    // Defer binary check until after scan (matches Python: ensure_tools_ready after
    // count_files)
    if mode_needs_img_vid_binaries(&args.mode) {
        ensure_tools_ready(&root, &args.mode)?;
    }
    if mode_needs_db_health(&args.mode) {
        verify_database_mandatory(&root)?;
    }

    if let (Some(scan), false) = (&scan, args.dry_run) {
        if scan.img_count > 0 || scan.vid_count > 0 {
            let check_path = if args.in_place {
                &args.inputs[0]
            } else {
                args.output.as_deref().unwrap_or_else(|| {
                    args.inputs
                        .first()
                        .map_or_else(|| Path::new("."), std::path::PathBuf::as_path)
                })
            };
            check_system_resources(
                check_path,
                scan.media_total_size.saturating_add(1024 * 1024 * 1024),
            )?;
        }
    } else if !args.dry_run && !args.inputs.is_empty() {
        check_system_resources(&args.inputs[0], 1024u64 * 1024 * 1024)?;
    }

    let fail_fast = drag_drop_fail_fast_enabled();
    let commands = if let Some(ref scan) = scan {
        if mode_uses_standard_pipeline(&args.mode) {
            plan_routed_pipeline_commands(args, &root, scan, fail_fast)?
        } else {
            plan_cli_invocations(args, &root, session)?
        }
    } else {
        plan_cli_invocations(args, &root, session)?
    };

    let mut first_error: Option<anyhow::Error> = None;
    let mut summary = PipelineSummary::default();
    let total_cmds = commands.len();

    dev::infra::elapsed_spinner::resize_terminal(45, 223);
    dev::infra::elapsed_spinner::hide_cursor();

    let _cursor_guard = CursorGuard;

    let mut spinner = dev::infra::elapsed_spinner::ElapsedSpinner::start();

    if matches!(args.mode, LaunchMode::FastImg) {
        println!("\n\x1B[36m⏳ Pacing start to ensure system stability...\x1B[0m");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        spinner.stop();
        println!("\n\x1B[34m🚀 Launching Rust Fast Mode Pipeline...\x1B[0m");
    }

    if matches!(args.mode, LaunchMode::FastImg | LaunchMode::RestoreJpeg) && !args.dry_run {
        if args.mode == LaunchMode::RestoreJpeg {
            let output = args.output.clone().unwrap_or_else(|| {
                fast_img_restore_output_dir_for_target(args.inputs.first().expect("input"))
            });
            let command = LaunchCommand::from_argv(build_fast_img_restore_command(
                &cli_binary(&root, "img"),
                &args.inputs[0],
                &output,
            ))?;
            draw_separator("Processing (restore-jpeg)");
            match command.run_with_session(false, session) {
                Ok(stats) => {
                    if stats.exit_code != 0 {
                        if fail_fast {
                            bail!("restore-jpeg exited with code {}", stats.exit_code);
                        }
                        summary.img.failed = summary.img.failed.max(1);
                        if first_error.is_none() {
                            first_error = Some(anyhow::anyhow!(
                                "restore-jpeg exited with code {}",
                                stats.exit_code
                            ));
                        }
                    } else {
                        summary.img = stats;
                    }
                }
                Err(err) if fail_fast => return Err(err),
                Err(err) => first_error = Some(err),
            }
            if first_error.is_none() {
                run_fast_img_restore_post_success(
                    args,
                    &root,
                    session.expect("session"),
                    &mut summary,
                    &output,
                )?;
            }
        } else {
            draw_separator("Processing (fast-img)");
            match run_fast_img_with_retry(args, &root, session.expect("session")) {
                Ok((stats, output)) => {
                    summary.img = stats.clone();
                    match fs::read_to_string(&session.expect("session").verbose_log) {
                        Ok(text) => {
                            let metrics = fast_img_session_size_metrics(&text);
                            summary.fast_img_session_source_bytes = metrics.source_bytes_actual;
                            summary.fast_img_session_output_bytes = metrics.output_bytes_actual;
                            // Collect per-file retained names for terminal + session log
                            for (name, disposition) in fast_img_retained_file_names(&text) {
                                if disposition == "failed" {
                                    summary.failed_file_names.push(name);
                                } else if disposition == "skipped" {
                                    summary.skipped_file_names.push(name);
                                }
                            }
                        }
                        Err(err) => eprintln!(
                            "{} fast-img size metrics log read failed: {err}",
                            pick_symbol("⚠️", "[WARN]")
                        ),
                    }
                    if stats.exit_code != 0 {
                        if fail_fast {
                            bail!("fast-img exited with code {}", stats.exit_code);
                        }
                        summary.img.failed = summary.img.failed.max(1);
                        if first_error.is_none() {
                            first_error = Some(anyhow::anyhow!(
                                "fast-img exited with code {}",
                                stats.exit_code
                            ));
                        }
                    } else {
                        run_fast_img_post_success(
                            args,
                            &root,
                            session.expect("session"),
                            &mut summary,
                            &output,
                        )?;
                    }
                }
                Err(err) if fail_fast => return Err(err),
                Err(err) => first_error = Some(err),
            }
        }
    } else {
        for (idx, command) in commands.into_iter().enumerate() {
            update_terminal_title(started.elapsed());
            draw_separator(&command_phase_label(&command.program, idx, total_cmds));
            match command.run_with_session(args.dry_run, session) {
                Ok(stats) => {
                    let is_img = command
                        .program
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n == "img");
                    let is_vid = command
                        .program
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n == "vid");
                    if is_img {
                        summary.img.succeeded += stats.succeeded;
                        summary.img.skipped += stats.skipped;
                        summary.img.ignored += stats.ignored;
                        summary.img.failed += stats.failed;
                        summary.img.exit_code = stats.exit_code;
                    } else if is_vid {
                        summary.vid.succeeded += stats.succeeded;
                        summary.vid.skipped += stats.skipped;
                        summary.vid.ignored += stats.ignored;
                        summary.vid.failed += stats.failed;
                        summary.vid.exit_code = stats.exit_code;
                    } else if summary.img.total() == 0 {
                        summary.img = stats.clone();
                    } else {
                        summary.vid = stats;
                    }
                }
                Err(err) => {
                    let is_img = command
                        .program
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n == "img");
                    let is_vid = command
                        .program
                        .file_name()
                        .and_then(|s| s.to_str())
                        .is_some_and(|n| n == "vid");
                    if is_img {
                        summary.img.failed += 1;
                    } else if is_vid {
                        summary.vid.failed += 1;
                    }
                    if fail_fast {
                        return Err(err);
                    }
                    eprintln!(
                        "{} command error (log-and-continue): {err:#}",
                        pick_symbol("⚠️", "[WARN]")
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }

        let img_vid_pipeline_completed =
            mode_uses_standard_pipeline(&args.mode) && !args.dry_run && first_error.is_none();

        if mode_uses_standard_pipeline(&args.mode) {
            if args.in_place {
                let _ = dir_lock;
            } else if img_vid_pipeline_completed && let (Some(scan), Some(sess)) = (&scan, session)
            {
                let output = adjacent_output_dir(args);
                if output.is_dir()
                    && let Err(err) = run_post_adjacent_steps(args, &root, sess, scan, &mut summary)
                {
                    if fail_fast {
                        return Err(err);
                    }
                    eprintln!(
                        "{} post-pipeline step error: {err:#}",
                        pick_symbol("⚠️", "[WARN]")
                    );
                    if first_error.is_none() {
                        first_error = Some(err);
                    }
                }
            }
        }
    }

    let mut size_summary_block: Option<String> = None;
    if !args.dry_run && (summary.has_image_stats() || summary.has_video_stats()) {
        let (effective_s, effective_f, penalty) = effective_success_failure_counts(
            summary.total_succeeded(),
            summary.total_failed(),
            summary.integrity_state.map(|s| s == "WARNINGS"),
            summary.integrity_issue_count,
        );
        if penalty > 0 {
            eprintln!(
                "{} +{penalty} integrity-derived failures applied to summary",
                pick_symbol("⚠️", "[WARN]")
            );
        }
        if effective_f > 0 {
            eprintln!(
                "\n   {} Exiting with failures: {effective_f} file(s) did not complete \
                 successfully.",
                pick_symbol("❌", "[ERROR]")
            );
            // Enumerate every retained file so the user doesn't have to grep logs
            if !summary.failed_file_names.is_empty() || !summary.skipped_file_names.is_empty() {
                eprintln!("   Retained files:");
                for name in &summary.failed_file_names {
                    eprintln!("     [FAIL] {name}");
                }
                for name in &summary.skipped_file_names {
                    eprintln!("     [SKIP] {name}");
                }
            }
        }
        let _ = effective_s;
        print_summary_report(&summary);
        if let Some(ref scan) = scan {
            let (before, after) = if matches!(args.mode, LaunchMode::FastImg) {
                let before = summary
                    .fast_img_session_source_bytes
                    .unwrap_or(scan.media_total_size);
                let after = if let Some(bytes) = summary
                    .fast_img_size_after_override
                    .or(summary.fast_img_session_output_bytes)
                {
                    bytes
                } else {
                    du_size_recursive(&adjacent_output_dir(args))?
                };
                (before, after)
            } else if args.in_place {
                (scan.media_total_size, scan.media_total_size)
            } else {
                (
                    scan.media_total_size,
                    du_size_recursive(&adjacent_output_dir(args))?,
                )
            };
            let block = build_size_comparison_summary(
                before,
                after,
                if args.in_place { "every" } else { "normal" },
                processing_filter(args).verify_mode_label(),
            );
            println!("\n{block}");
            size_summary_block = Some(block);
        }
    }
    if let Some(sess) = session {
        let _ = sess.finish_log(&summary, size_summary_block.as_deref());
    }
    print_elapsed(started.elapsed());
    eprintln!(
        "{} Session stamp: {}",
        pick_symbol("✓", "[DONE]"),
        session.map_or("-", |s| s.stamp.as_str())
    );

    if let (Some(sess), Some(output_dir)) = (session, &args.output) {
        let output_size = du_size_recursive(output_dir)?;
        if let Some(source_dir) = args.inputs.first() {
            let source_size = du_size_recursive(source_dir)?;
            let ratio = if source_size > 0 {
                foundation::numeric_cast::u64_to_f64(output_size)
                    / foundation::numeric_cast::u64_to_f64(source_size)
            } else {
                0.0
            };
            println!(
                "{} Source: {} → Output: {} ({:.1}%)",
                pick_symbol("📊", "[STATS]"),
                format_bytes(source_size),
                format_bytes(output_size),
                ratio * 100.0
            );
            let _ = &sess;
        }
    }

    if !args.dry_run && (summary.has_image_stats() || summary.has_video_stats()) {
        let (_, effective_f, _) = effective_success_failure_counts(
            summary.total_succeeded(),
            summary.total_failed(),
            summary.integrity_state.map(|s| s == "WARNINGS"),
            summary.integrity_issue_count,
        );
        if effective_f > 0 {
            bail!("exiting with failures: {effective_f} file(s) did not complete successfully");
        }
    }

    if let Some(err) = first_error {
        return Err(err);
    }

    Ok(())
}

fn finalize_handoff_preservation(
    source_root: &Path,
    optimized_root: &Path,
    session: &DragDropSession,
) -> Result<()> {
    draw_separator("Handoff Preserve (optional — post img/vid only)");
    let routed = extract_routed_video_paths_from_audit(&session.session_audit)?;
    if routed.is_empty() {
        session.append_line(
            &session.session_audit,
            "HANDOFF_PRESERVE_SKIP reason=no_video_routes",
        )?;
        return Ok(());
    }

    let candidates = report_handoff_preserve_gaps_from_paths(source_root, optimized_root, &routed)?;
    session.append_line(
        &session.session_audit,
        &format!("HANDOFF_PRESERVE_SCAN candidates={}", candidates.len()),
    )?;

    println!(
        "\n{} Handoff preserve (optional — post img/vid only)",
        pick_symbol("⚠️", "[WARN]")
    );
    println!("   Source:    {}", source_root.display());
    println!("   Optimized: {}", optimized_root.display());

    if candidates.is_empty() {
        println!("   {} No handoff gaps detected", pick_symbol("✓", "[OK]"));
        session.append_line(&session.session_audit, "HANDOFF_PRESERVE_NONE_NEEDED")?;
        return Ok(());
    }

    let total_bytes: u64 = candidates.iter().map(|c| c.size_bytes).sum();
    println!(
        "   {} {} file(s) would be copied ({} total):",
        pick_symbol("⚠️", "[WARN]"),
        candidates.len(),
        format_bytes(total_bytes)
    );
    for candidate in &candidates {
        println!(
            "      • {}  ({})",
            candidate.rel_path,
            format_bytes(candidate.size_bytes)
        );
        session.append_line(
            &session.session_audit,
            &format!(
                "HANDOFF_PRESERVE_CANDIDATE path={} bytes={}",
                candidate.rel_path, candidate.size_bytes
            ),
        )?;
    }

    let choice = read_choice("Preserve handoff gaps? (y/N): ")?;
    let accepted = matches!(choice.to_lowercase().as_str(), "y" | "yes");
    if !accepted {
        session.append_line(&session.session_audit, "HANDOFF_PRESERVE_DECLINED")?;
        println!("   Skipped handoff preserve.");
        return Ok(());
    }

    let mut audit_cb = |line: &str| {
        let _ = session.append_line(&session.session_audit, line);
    };
    let preserved = preserve_handoff_gaps(
        source_root,
        optimized_root,
        &routed,
        Some(&candidates),
        HANDOFF_PRESERVE_PHASE_POST_IMG_VID,
        Some(&mut audit_cb),
    )?;
    println!(
        "   {} Preserved {} handoff file(s)",
        pick_symbol("✓", "[OK]"),
        preserved.len()
    );
    Ok(())
}

fn read_choice(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("flush stdout")?;
    let mut line = String::new();
    io::stdin().read_line(&mut line).context("read stdin")?;
    Ok(line.trim().to_string())
}

fn prompt_for_input() -> Result<Option<PathBuf>> {
    let cyan = if colors_enabled() { "\x1b[0;36m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    println!(
        "\n{cyan}Please drag and drop a folder or file into this terminal window and press \
         Enter.{reset}"
    );
    drain_stdin();
    let path = read_choice(&format!("{cyan} > {reset}"))?;
    if path.is_empty() {
        return Ok(None);
    }
    Ok(Some(validate_drag_drop_path(&path)?))
}

fn wait_enter() {
    let cyan = if colors_enabled() { "\x1b[0;36m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    let _ = read_choice(&format!("\n{cyan}Press Enter to return to menu...{reset}"));
}

fn execute_menu_quick_pick(
    choice: usize,
    session: &DragDropSession,
    dir_lock: &mut Option<DirLock>,
    target: &Path,
) -> Result<()> {
    let (cat, sub) = match choice {
        1 => (0, 0),
        2 => (0, 1),
        3 => (0, 2),
        4 => (1, 0),
        5 => (1, 1),
        6 => (1, 2),
        7 => (2, 0),
        8 => (2, 1),
        9 => (2, 2),
        _ => return Ok(()),
    };
    let _ = execute_menu_selection(
        cat,
        sub,
        sub,
        sub,
        session,
        dir_lock,
        target,
        ProcessingFilter::Both,
    )?;
    Ok(())
}

fn execute_menu_selection(
    selected: usize,
    opt_sub: usize,
    workspace_sub: usize,
    maint_sub: usize,
    session: &DragDropSession,
    dir_lock: &mut Option<DirLock>,
    target: &Path,
    filter: ProcessingFilter,
) -> Result<bool> {
    let root = resolve_runtime_root()?;
    match selected {
        0 => {
            let fastmode_count = if matches!(
                filter,
                ProcessingFilter::ImagesOnly | ProcessingFilter::VideosOnly
            ) {
                3
            } else {
                2
            };
            let actual = map_mode_sub_state(opt_sub, fastmode_count, filter);
            if actual == 0 {
                let output = adjacent_output_for_target(target);
                create_directory_structure(target, &output, Some(&session.session_audit))?;
                println!("\n{} ADJACENT MODE SELECTED", pick_symbol("✅", "[OK]"));
                println!("   Output: {}", output.display());
                let args = build_run_args(
                    target,
                    LaunchMode::Auto,
                    Some(output),
                    false,
                    true,
                    false,
                    filter,
                );
                run_drag_drop(&args, Some(session), dir_lock.as_ref())?;
            } else if actual == 1 {
                if !confirm_in_place()? {
                    return Ok(false);
                }
                *dir_lock = Some(acquire_global_lock(target)?);
                let args =
                    build_run_args(target, LaunchMode::Auto, None, true, false, false, filter);
                run_drag_drop(&args, Some(session), dir_lock.as_ref())?;
            } else if matches!(filter, ProcessingFilter::VideosOnly) {
                let shortest = choose_fast_vid_shortest_path()?;
                let output = dev::infra::fastmode_paths::fast_vid_output_dir_for_target(target);
                let args = build_run_args(
                    target,
                    LaunchMode::FastVid,
                    Some(output),
                    false,
                    true,
                    shortest,
                    filter,
                );
                run_drag_drop(&args, Some(session), dir_lock.as_ref())?;
            } else {
                let action = choose_fast_img_action()?;
                let verify_bin = cli_binary(&root, "verify");
                let output =
                    dev::infra::drag_drop::resolve_output_for_fast_img(target, action, &verify_bin);
                let (mode, shortest) = match action {
                    FastImgAction::RestoreJpeg => (LaunchMode::RestoreJpeg, false),
                    FastImgAction::Normal => (LaunchMode::FastImg, false),
                    FastImgAction::ShortestPath => (LaunchMode::FastImg, true),
                };
                let args =
                    build_run_args(target, mode, Some(output), false, true, shortest, filter);
                run_drag_drop(&args, Some(session), dir_lock.as_ref())?;
            }
        }
        1 => {
            let mode = match workspace_sub {
                1 => LaunchMode::MergeXmp,
                2 => LaunchMode::IcloudImport,
                _ => LaunchMode::Collect,
            };
            let output = if mode == LaunchMode::Collect {
                Some(get_unique_output_path(&target.with_file_name(format!(
                    "{}_collected",
                    target.file_name().unwrap_or_default().to_string_lossy()
                ))))
            } else {
                None
            };
            let args = build_run_args(
                target,
                mode,
                output,
                false,
                false,
                false,
                ProcessingFilter::Both,
            );
            run_drag_drop(&args, Some(session), dir_lock.as_ref())?;
        }
        _ => match maint_sub {
            0 => {
                let args = build_run_args(
                    target,
                    LaunchMode::Diagnostic,
                    None,
                    false,
                    false,
                    false,
                    ProcessingFilter::Both,
                );
                run_drag_drop(&args, Some(session), dir_lock.as_ref())?;
            }
            1 => {
                let bin = cli_binary(&root, "cache_cleaner");
                let status = Command::new(&bin).status()?;
                if !status.success() {
                    eprintln!("{}Cache cleaner failed.", pick_symbol("❌", "[ERROR]"));
                }
            }
            _ => {
                let bin = cli_binary(&root, "database_manager");
                let status = Command::new(&bin).status()?;
                if !status.success() {
                    eprintln!("{}Database manager failed.", pick_symbol("❌", "[ERROR]"));
                }
            }
        },
    }
    Ok(true)
}

const fn map_mode_sub_state(sub: usize, fastmode_count: usize, filter: ProcessingFilter) -> usize {
    let _ = fastmode_count;
    if matches!(
        filter,
        ProcessingFilter::ImagesOnly | ProcessingFilter::VideosOnly
    ) {
        match sub {
            0 => 2,
            2 => 0,
            other => other,
        }
    } else {
        sub
    }
}

fn build_run_args(
    target: &Path,
    mode: LaunchMode,
    output: Option<PathBuf>,
    in_place: bool,
    archive: bool,
    shortest_path: bool,
    filter: ProcessingFilter,
) -> Args {
    Args {
        inputs: vec![target.to_path_buf()],
        mode,
        output,
        archive,
        shortest_path,
        retry: false,
        force: false,
        dry_run: false,
        plain: false,
        vue: false,
        base_dir: None,
        in_place,
        resume: false,
        ultimate: DRAG_DROP_CHILD_ULTIMATE,
        verbose: DRAG_DROP_CHILD_VERBOSE,
        watch: false,
        images_only: matches!(filter, ProcessingFilter::ImagesOnly),
        videos_only: matches!(filter, ProcessingFilter::VideosOnly),
    }
}

fn interactive_menu(args: &Args, session: &mut DragDropSession) -> Result<()> {
    let bold = if colors_enabled() { "\x1b[1m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    let filter = processing_filter(args);

    let target = if args.inputs.is_empty() {
        let Some(path) = prompt_for_input()? else {
            return Ok(());
        };
        safety_check(&path)?;
        session.rename_log_to_project(&path)?;
        path
    } else {
        args.inputs[0].clone()
    };

    let snapshot = probe_system_snapshot(&target);
    print_runtime_panel(&RuntimeDashboard {
        target_path: target.display().to_string(),
        mode_label: "Interactive".to_string(),
        target_type: target_type_label(args).to_string(),
        output_path: None,
        ultimate: args.ultimate,
        watch: args.watch,
        cpu_percent: Some(snapshot.cpu_percent),
        memory_percent: Some(snapshot.memory_percent),
        disk_free_gb: Some(
            foundation::numeric_cast::u64_to_f64(snapshot.disk_free_bytes)
                / (1024.0 * 1024.0 * 1024.0),
        ),
    });

    let mut dir_lock = None;
    let mut selected = 0usize;
    let mut opt_sub = 0usize;
    let mut workspace_sub = 0usize;
    let mut maint_sub = 0usize;
    let fastmode_count = if matches!(
        filter,
        ProcessingFilter::ImagesOnly | ProcessingFilter::VideosOnly
    ) {
        3
    } else {
        2
    };

    loop {
        clear_screen();
        draw_banner(env!("CARGO_PKG_VERSION"));
        println!("{bold}Select Operation Mode:{reset}\n");
        println!("   Target: {}\n", target.display());

        let (opt_title, opt_desc) = optimization_menu_labels(opt_sub, filter);
        print_menu_row(selected == 0, opt_title, opt_desc);

        let ws = [
            (
                "Tool: Collect Optimized Media [Tab to Switch]",
                "Move optimized outputs into a mirrored directory tree.",
            ),
            (
                "Tool: Merge XMP Attachments [Tab to Switch]",
                "Embed XMP sidecars into source media files safely.",
            ),
            (
                "Tool: iCloud Photo Import [Tab to Switch]",
                "Import processed assets into iCloud (osxphotos).",
            ),
        ];
        print_menu_row(selected == 1, ws[workspace_sub].0, ws[workspace_sub].1);

        let mt = [
            (
                "Tool: Diagnostic Analysis [Tab to Switch]",
                "Analyze logs and verify output integrity.",
            ),
            (
                "Tool: Cleanup Cache & Logs [Tab to Switch]",
                "Clear analysis cache, session logs, and task progress.",
            ),
            (
                "Tool: Database Manager [Tab to Switch]",
                "Clean, train, backup and manage database.",
            ),
        ];
        print_menu_row(selected == 2, mt[maint_sub].0, mt[maint_sub].1);

        print_menu_hint();

        let key = read_nav_key()?;
        match key {
            NavKey::Up | NavKey::Left => {
                let menu_item_count = 3;
                selected = (selected + menu_item_count - 1) % menu_item_count;
            }
            NavKey::Down | NavKey::Right => {
                selected = (selected + 1) % 3;
            }
            NavKey::Tab => match selected {
                0 => opt_sub = (opt_sub + 1) % fastmode_count,
                1 => workspace_sub = (workspace_sub + 1) % 3,
                _ => maint_sub = (maint_sub + 1) % 3,
            },
            NavKey::Quit | NavKey::Char('0') => return Ok(()),
            NavKey::Char(c @ '1'..='9') => {
                let n = match c.to_digit(10) {
                    Some(v) => v as usize,
                    None => 0,
                };
                execute_menu_quick_pick(n, session, &mut dir_lock, &target)?;
                wait_enter();
            }
            NavKey::Enter => {
                let _ = execute_menu_selection(
                    selected,
                    opt_sub,
                    workspace_sub,
                    maint_sub,
                    session,
                    &mut dir_lock,
                    &target,
                    filter,
                )?;
                wait_enter();
            }
            NavKey::Unknown | NavKey::Char(_) => {}
        }
    }
}

const fn optimization_menu_labels(
    sub: usize,
    filter: ProcessingFilter,
) -> (&'static str, &'static str) {
    match filter {
        ProcessingFilter::VideosOnly => match sub {
            0 => (
                "Mode: Fast Video Mode [Tab to Switch]",
                "Full LoopIntent path for videos and animated images.",
            ),
            1 => (
                "Mode: In-Place Optimization [Tab to Switch]",
                "Replaces original files. Saves disk space.",
            ),
            _ => (
                "Mode: Output to Adjacent Folder [Tab to Switch]",
                "Safe mode. Keeps originals untouched.",
            ),
        },
        ProcessingFilter::ImagesOnly => match sub {
            0 => (
                "Mode: Fast Image Mode [Tab to Switch]",
                "JPEG→JXL fast path or JXL→JPEG restore.",
            ),
            1 => (
                "Mode: In-Place Optimization [Tab to Switch]",
                "Replaces original files. Saves disk space.",
            ),
            _ => (
                "Mode: Output to Adjacent Folder [Tab to Switch]",
                "Safe mode. Keeps originals untouched.",
            ),
        },
        ProcessingFilter::Both => match sub {
            0 => (
                "Mode: Output to Adjacent Folder [Tab to Switch]",
                "Safe mode. Keeps originals untouched.",
            ),
            _ => (
                "Mode: In-Place Optimization [Tab to Switch]",
                "Replaces original files. Saves disk space.",
            ),
        },
    }
}

const fn apply_mode_overrides(mut args: Args) -> Args {
    if args.images_only {
        args.mode = LaunchMode::Images;
    } else if args.videos_only {
        args.mode = LaunchMode::Videos;
    }
    args
}

fn run_watch_loop(args: &Args, session: &DragDropSession) -> Result<()> {
    let watch_root = args
        .inputs
        .first()
        .context("watch mode requires an input directory")?
        .clone();
    let processing = AtomicBool::new(false);
    if args.watch {
        draw_separator("Watch Mode Enabled");
        println!(
            "{} Monitoring: {}",
            pick_symbol("👁", "[WATCH]"),
            watch_root.display()
        );
        println!("Press Ctrl+C to stop. Debouncing active.\n");
    }
    watch_directory_with_debounce(&watch_root, 2000, move |event| {
        let relevant = event.paths.iter().any(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| is_watch_trigger_ext(&format!(".{ext}")))
        });
        if !relevant {
            return;
        }
        if processing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        eprintln!(
            "{} Watch trigger — re-running pipeline",
            pick_symbol("🔄", "[UPDATE]")
        );
        if let Err(err) = run_drag_drop(args, Some(session), None) {
            eprintln!(
                "{} watch reprocess error: {err:#}",
                pick_symbol("⚠️", "[WARN]")
            );
        }
        processing.store(false, Ordering::SeqCst);
    })
}

fn main() -> Result<()> {
    use std::io::IsTerminal;
    install_signal_handlers()?;
    unsafe {
        std::env::set_var("MFB_GUI_LAUNCH", "1");
        std::env::set_var("FORCE_COLOR", "1");
        std::env::set_var("CLICOLOR_FORCE", "1");
        if std::env::var("MODERN_FORMAT_DISABLE_STRICT_MEDIA_CONVERSION").is_err() {
            std::env::set_var("MODERN_FORMAT_DISABLE_STRICT_MEDIA_CONVERSION", "0");
        }
    }
    resize_terminal_for_gui(35, 110);
    let args = apply_mode_overrides(Args::parse());
    let mut session = DragDropSession::start()?;
    let mut dir_lock = None;

    // Vue launcher check
    if args.vue {
        bail!(
            "Vue launcher is not implemented; run crates/dev/src/vue scripts directly during UI \
             prototyping"
        );
    }

    // ALWAYS show interactive menu when terminal (matches Python's unconditional
    // select_mode())
    if io::stdin().is_terminal() {
        return interactive_menu(&args, &mut session);
    }

    // Non-interactive: require inputs
    if args.inputs.is_empty() {
        bail!("at least one input path is required");
    }

    session.rename_log_to_project(&args.inputs[0])?;

    if args.in_place {
        dir_lock = Some(acquire_global_lock(args.inputs.first().expect("input"))?);
    }

    let run_result = run_drag_drop(&args, Some(&session), dir_lock.as_ref());
    if args.watch {
        if run_result.is_err() {
            report_drag_drop_failure(&run_result);
            return run_result;
        }
        run_watch_loop(&args, &session)?;
        return Ok(());
    }

    match session.archive() {
        Ok(Some(bundle)) => {
            println!(
                "{} Archived session bundle: {}",
                pick_symbol("📦", "[ARCHIVE]"),
                bundle.display()
            );
        }
        Ok(None) => {}
        Err(err) => {
            if run_result.is_ok() {
                return Err(err);
            }
            eprintln!("drag/drop session archive failed: {err:#}");
        }
    }
    if run_result.is_err() {
        report_drag_drop_failure(&run_result);
    }
    run_result
}

fn report_drag_drop_failure(result: &Result<()>) {
    if let Err(err) = result {
        eprintln!("{err:#}");
        print_critical_error_panel("pipeline", 1);
        pause_before_gui_exit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn cli_binary_uses_release_path_for_workspace_like_roots() {
        assert_eq!(
            cli_binary(Path::new("/repo"), "img"),
            PathBuf::from("/repo/target/release/img")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cli_binary_uses_packaged_binary_inside_macos_app_bundle() {
        let resources = Path::new("/Applications/Modern Format Boost.app/Contents/Resources");
        assert!(is_app_bundle_resource_root(resources));
        assert_eq!(
            cli_binary(resources, "img"),
            PathBuf::from("/Applications/Modern Format Boost.app/Contents/Resources/img")
        );
    }

    #[test]
    fn test_auto_directory_defaults_to_rust_img_and_vid_cli() {
        let temp = tempfile::tempdir().unwrap();
        let input = temp.path().join("Album");
        std::fs::create_dir(&input).unwrap();
        let args = Args {
            inputs: vec![input],
            mode: LaunchMode::Auto,
            output: Some(PathBuf::from("/tmp/Album_optimized")),
            archive: true,
            shortest_path: false,
            retry: false,
            force: false,
            dry_run: true,
            resume: false,
            ultimate: false,
            in_place: false,
            verbose: false,
            base_dir: None,
            plain: true,
            vue: false,
            watch: false,
            images_only: false,
            videos_only: false,
        };

        let commands = plan_cli_invocations(&args, Path::new("/repo"), None).unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0].program,
            PathBuf::from("/repo/target/release/img")
        );
        assert_eq!(
            commands[1].program,
            PathBuf::from("/repo/target/release/vid")
        );
        assert!(commands[0].args.contains(&"run".to_string()));
        assert!(commands[1].args.contains(&"run".to_string()));
        assert!(commands[0].args.contains(&"--archive".to_string()));
        assert!(commands[1].args.contains(&"--archive".to_string()));
        assert!(commands[0].args.contains(&"--plain".to_string()));
        assert!(commands[1].args.contains(&"--plain".to_string()));
    }

    #[test]
    fn test_vue_flag_is_scaffold_only() {
        let args = Args {
            inputs: Vec::new(),
            mode: LaunchMode::Auto,
            output: None,
            archive: false,
            shortest_path: false,
            retry: false,
            force: false,
            dry_run: true,
            resume: false,
            ultimate: false,
            in_place: false,
            verbose: false,
            base_dir: None,
            plain: true,
            vue: true,
            watch: false,
            images_only: false,
            videos_only: false,
        };

        let err = plan_cli_invocations(&args, Path::new("/repo"), None).unwrap_err();
        assert!(err.to_string().contains("Vue prototype"));
    }

    #[test]
    fn test_workspace_tool_modes_delegate_to_rust_bins() {
        let target = PathBuf::from("/input/Album");
        let collect_out = PathBuf::from("/input/Album_collected");
        let modes = [
            (
                LaunchMode::Collect,
                Some(collect_out),
                "/repo/target/release/collect_optimized",
                vec!["/input/Album", "/input/Album_collected"],
            ),
            (
                LaunchMode::MergeXmp,
                None,
                "/repo/target/release/merge_xmp",
                vec!["/input/Album"],
            ),
            (
                LaunchMode::IcloudImport,
                None,
                "/repo/target/release/icloud_import",
                vec!["/input/Album"],
            ),
        ];

        for (mode, output, program, expected_args) in modes {
            let args = Args {
                inputs: vec![target.clone()],
                mode,
                output,
                archive: false,
                shortest_path: false,
                retry: false,
                force: false,
                dry_run: true,
                resume: false,
                ultimate: false,
                in_place: false,
                verbose: false,
                base_dir: None,
                plain: true,
                vue: false,
                watch: false,
                images_only: false,
                videos_only: false,
            };
            let commands = plan_cli_invocations(&args, Path::new("/repo"), None).unwrap();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].program, PathBuf::from(program));
            assert_eq!(commands[0].args, expected_args);
        }
    }

    #[test]
    fn test_maintenance_tool_modes_delegate_to_rust_bins() {
        let target = PathBuf::from("/input/Album");
        let modes = [
            (
                LaunchMode::Diagnostic,
                "/repo/target/release/verify",
                vec!["--verify", "/input/Album", "--mode", "both"],
            ),
            (
                LaunchMode::CacheClean,
                "/repo/target/release/cache_cleaner",
                vec![],
            ),
            (
                LaunchMode::DatabaseManager,
                "/repo/target/release/database_manager",
                vec![],
            ),
        ];

        for (mode, program, expected_args) in modes {
            let args = Args {
                inputs: vec![target.clone()],
                mode,
                output: None,
                archive: false,
                shortest_path: false,
                retry: false,
                force: false,
                dry_run: true,
                resume: false,
                ultimate: false,
                in_place: false,
                verbose: false,
                base_dir: None,
                plain: true,
                vue: false,
                watch: false,
                images_only: false,
                videos_only: false,
            };
            let commands = plan_cli_invocations(&args, Path::new("/repo"), None).unwrap();
            assert_eq!(commands.len(), 1);
            assert_eq!(commands[0].program, PathBuf::from(program));
            assert_eq!(commands[0].args, expected_args);
        }
    }

    #[test]
    fn test_session_archive_moves_drag_drop_logs() {
        let temp = tempfile::tempdir().unwrap();
        let session = DragDropSession::start_for_test(temp.path()).unwrap();
        let stamp = session.stamp.clone();

        let bundle = session.archive().unwrap().expect("bundle created");

        assert!(bundle.join(format!("MFB_Session_{stamp}.log")).is_file());
        assert!(bundle.join(format!("verbose_{stamp}.log")).is_file());
        let audit = bundle.join(format!("session_audit_{stamp}.jsonl"));
        assert!(audit.is_file());
        let audit_content = std::fs::read_to_string(audit).unwrap();
        assert!(audit_content.contains("SESSION_STARTED"));
        assert!(audit_content.contains("SESSION_ARCHIVE_BEGIN"));
        assert!(audit_content.contains("SESSION_ARCHIVE_DONE"));
        let manifest = std::fs::read_to_string(bundle.join("manifest.json")).unwrap();
        assert!(manifest.contains(&format!("MFB_Session_{stamp}.log")));
    }
}

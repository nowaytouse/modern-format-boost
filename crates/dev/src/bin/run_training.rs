//! Modern Format Boost - Training Pipeline Driver (Rust port)
//!
//! Strict alignment with Multi-Scenario architecture.
//! References: `crates/dev/scripts/run_training.py` (retained as compat reference).
//!
//! Core responsibilities:
//! - Training corpus collection (static image quality / loop intent)
//! - Rust tier probe (entropy/geometry via `probe_static_still_image`)
//! - Balance high/low tiers by complexity quantiles
//! - Batch ingestion via C-API or CLI fallback
//! - Runtime asset finalization (`LightGBM` / KNN)
//!
//! Safety:
//! - MANDATORY PHYSICAL REPLICAS: shutil.copy2 equivalent via temp dirs
//! - ZERO SILENT FORGERY: only real entropy/physics admitted to DB
//! - Fail-closed: label conflicts → exit 1 (or log-and-continue mode)

use anyhow::{Context, Result, bail};
use clap::Parser;
use dev::infra::background_detach::{BackgroundPidGuard, detach_current_process};
use dev::infra::log_paths::{
    ensure_training_session_stamp, ensure_unified_log_dir, training_lane_slug,
};
use dev::infra::training_session_audit::{TrainingSessionRecorder, summarize_argv};
use dev::run_training::collect::collect_plan_samples;
use dev::run_training::config_parse::load_rules;
use dev::run_training::finalize::fill_runtime_assets;
use dev::run_training::four_lane::run_four_lane_launcher;
use dev::run_training::ingest_profile::{
    apply_ingest_profile, enforce_training_db_caps, validate_ingest_rules,
};
use dev::run_training::isolate::run_training_isolated;
use dev::run_training::types::{Args, FillAssetsConfig, TrainingMode};
use dev::training_pipeline::{
    finalize_image_quality_model, finalize_loop_intent_assets, resolve_connstr,
    run_training_pipeline_subcommand,
};
use foundation::database::reset_training_db;
use serde_json::{Map, Value, json};
use std::env;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::sync::{Arc, Mutex};

const TRAINING_VERBOSE_ENV: &str = "MFB_TRAINING_VERBOSE";

/// Pin training logs to unified home log root (never `target/training_*`).
fn pin_training_log_dir() -> Result<PathBuf> {
    ensure_unified_log_dir()
}

fn lock_rec(
    rec: &Arc<Mutex<TrainingSessionRecorder>>,
) -> Result<std::sync::MutexGuard<'_, TrainingSessionRecorder>> {
    rec.lock()
        .map_err(|_| anyhow::anyhow!("training session recorder lock poisoned"))
}

fn training_session_heartbeat(rec: Option<&Arc<Mutex<TrainingSessionRecorder>>>) {
    if let Some(rec) = rec {
        match rec.lock() {
            Ok(mut guard) => {
                let _ = guard.maybe_heartbeat(None);
            }
            Err(err) => eprintln!("[TRAIN] session recorder lock poisoned: {err}"),
        }
    }
}

/// Mirror py `apply_training_scan_defaults`: set `MFB_PERF_TIER=tight` unless already set.
fn apply_training_scan_defaults() {
    if env::var("MFB_PERF_TIER").is_err() {
        // SAFETY: single-threaded at this point (before any thread spawn in main)
        unsafe { env::set_var("MFB_PERF_TIER", "tight") };
        eprintln!(
            "  [PERF] scan governor default: tight (override with MFB_PERF_TIER=balanced|relaxed)"
        );
    }
}

fn training_verbose_enabled(cli_verbose: bool) -> bool {
    if cli_verbose {
        return true;
    }
    matches!(
        env::var(TRAINING_VERBOSE_ENV)
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

fn project_root() -> Result<PathBuf> {
    let cwd = env::current_dir()?;
    if cwd.join("Cargo.toml").is_file() && cwd.join("crates").is_dir() {
        return Ok(cwd);
    }
    for ancestor in cwd.ancestors() {
        if ancestor.join("Cargo.toml").is_file() && ancestor.join("crates").is_dir() {
            return Ok(ancestor.to_path_buf());
        }
    }
    bail!("could not locate repository root (missing Cargo.toml)")
}

fn validate_finalize_flags(args: &Args) -> Result<()> {
    if args.finalize.finalize_all && args.finalize.finalize_image_quality_model {
        bail!("--finalize-all already includes image_quality finalize");
    }
    if args.finalize.finalize_all && args.finalize.finalize_loop_intent {
        bail!("--finalize-all already includes loop_intent finalize");
    }
    if args.effective_fill_runtime_assets()
        && (args.finalize.finalize_image_quality_model || args.finalize.finalize_loop_intent)
    {
        bail!(
            "Default runtime fill already finalizes loop_intent and image_quality; \
             do not combine with --finalize-image-quality-model or --finalize-loop-intent. \
             For partial finalize only, pass --no-fill-runtime-assets with one of those flags."
        );
    }
    if args.assets.verify_after && !args.effective_fill_runtime_assets() {
        bail!("--verify-after requires runtime fill; omit --no-fill-runtime-assets");
    }
    if args.plan.no_loop && args.label.as_deref() == Some("animated_loop") {
        bail!("--label animated_loop cannot be combined with --no-loop");
    }
    if args.plan.no_loop && args.training_mode == TrainingMode::Loop {
        bail!("--training-mode loop cannot be combined with --no-loop");
    }
    if args.plan.dry_run && args.exec.execute {
        bail!("--dry-run and --execute cannot be combined");
    }
    Ok(())
}

fn finalize_only_with_no_samples(args: &Args, conn_str: &str, root: &Path) -> Result<()> {
    if args.db.repair_schema {
        let code = run_training_pipeline_subcommand(
            root,
            conn_str,
            "repair-multi-scenario-schema",
            &["--drop-legacy-gif-schema"],
        )?;
        if code != 0 {
            exit(code);
        }
    }
    if args.effective_fill_runtime_assets() {
        let code = fill_runtime_assets(
            conn_str,
            FillAssetsConfig {
                state: dev::run_training::types::AssetsState {
                    saw_image_quality: false,
                    saw_loop_samples: false,
                },
                actions: dev::run_training::types::AssetsActions {
                    install_deps: args.misc.install_missing_python_deps,
                    verify_after: args.assets.verify_after,
                },
                training_mode: args.training_mode,
            },
        )?;
        exit(code);
    }
    if args.finalize.finalize_loop_intent {
        exit(finalize_loop_intent_assets(conn_str)?);
    }
    if args.finalize.finalize_image_quality_model {
        exit(finalize_image_quality_model(
            conn_str,
            args.misc.install_missing_python_deps,
        )?);
    }
    exit(2);
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    let _pid_guard = BackgroundPidGuard::from_env();

    if args.multi.four_lane {
        return run_four_lane_launcher(&args);
    }
    if args.multi.stop
        || args.log_root.is_some()
        || !args.lane.is_empty()
        || args.multi.rebuild_dylib
    {
        bail!("--stop, --log-root, --lane, and --rebuild-dylib require --four-lane");
    }

    let log_dir = pin_training_log_dir()?;
    let lane = training_lane_slug(
        match args.training_mode {
            TrainingMode::All => "all",
            TrainingMode::Static => "static",
            TrainingMode::Loop => "loop",
        },
        args.label.as_deref(),
        &args.loop_intent_label,
    );
    // SAFETY: single-threaded before worker threads spawn.
    unsafe {
        env::set_var("MFB_TRAINING_LANE", &lane);
    }

    let stamp = ensure_training_session_stamp();
    let session = TrainingSessionRecorder::new(&log_dir, &stamp, None)?;
    TrainingSessionRecorder::install_handlers(Arc::clone(&session));
    let session: Option<Arc<Mutex<TrainingSessionRecorder>>> = Some(session);

    let mut start_fields = Map::new();
    start_fields.insert("lane".to_string(), Value::String(lane));
    start_fields.insert(
        "training_mode".to_string(),
        Value::String(format!("{:?}", args.training_mode).to_lowercase()),
    );
    if let Some(label) = &args.label {
        start_fields.insert("label".to_string(), Value::String(label.clone()));
    }
    start_fields.insert(
        "loop_intent_label".to_string(),
        Value::String(args.loop_intent_label.clone()),
    );
    start_fields.insert("dry_run".to_string(), json!(args.plan.dry_run));
    start_fields.insert(
        "ingest_planned".to_string(),
        json!(!args.plan.dry_run || args.exec.execute),
    );
    start_fields.insert("argv".to_string(), json!(summarize_argv(None)));
    start_fields.insert("log_dir".to_string(), json!(log_dir.to_string_lossy()));
    start_fields.insert(
        "pg_connstr_set".to_string(),
        json!(
            !env::var("MFB_PG_CONNSTR")
                .unwrap_or_default()
                .trim()
                .is_empty()
        ),
    );
    if let Some(rec) = &session {
        lock_rec(rec)?.emit("session_start", Some(start_fields))?;
    }

    if args.exec.background {
        let pid_file = log_dir.join("run_training.pid");
        let root = project_root()?;
        detach_current_process(&root, &log_dir, &pid_file, "--background")?;
    }

    apply_training_scan_defaults();

    if args.finalize.finalize_all {
        args.assets.no_fill_runtime_assets = false;
    }
    validate_finalize_flags(&args)?;

    let root = project_root()?;
    let conn_str = resolve_connstr(None);

    if !args.plan.dry_run && args.db.reset_db {
        reset_training_db(&conn_str).context("reset training database")?;
    }

    let cwd = env::current_dir()?;
    let rules_file = cwd.join("crates/dev/src/config/training_rules.json");
    let local_rules_file = cwd.join("crates/dev/src/config/training_rules.local.json");
    let rules = load_rules(&rules_file, Some(&local_rules_file))
        .context("Failed to load rules configuration")?;

    apply_ingest_profile(&mut args, rules.ingest.as_ref());
    enforce_training_db_caps(&mut args)?;
    validate_ingest_rules(&rules)?;

    // SAFETY: single-threaded before worker threads spawn.
    unsafe {
        env::set_var("MFB_TIER_AMBIGUOUS_POLICY", &rules.tier_ambiguous_policy);
    }

    if args.training_mode == TrainingMode::Static && args.loop_intent_label != "auto" {
        eprintln!("  [WARN] --loop-intent-label is ignored when --training-mode is static");
    }

    if !args.plan.dry_run && args.db.repair_schema {
        eprintln!("  [FIX] Repairing strict multi-scenario schema before ingestion...");
        let code = run_training_pipeline_subcommand(
            &root,
            conn_str.as_str(),
            "repair-multi-scenario-schema",
            &["--drop-legacy-gif-schema"],
        )?;
        if code != 0 {
            if let Some(rec) = &session {
                lock_rec(rec)?.finalize(code, "repair_schema_failed", false, None)?;
            }
            exit(code);
        }
    }

    if let Some(rec) = &session {
        lock_rec(rec)?.set_phase("collect", None)?;
    }
    training_session_heartbeat(session.as_ref());

    let samples = collect_plan_samples(&args, &rules)?;
    let ingest = !args.plan.dry_run || args.exec.execute;

    if samples.is_empty() {
        eprintln!("No samples found.");
        if ingest {
            if let Some(rec) = &session {
                lock_rec(rec)?.finalize(2, "no_samples", false, None)?;
            }
            finalize_only_with_no_samples(&args, &conn_str, &root)?;
        }
        return Ok(());
    }

    println!(
        "run_training: collected {} samples  training_mode={:?}  dry_run={}  use_api={}  fill_runtime={}  verbose={}",
        samples.len(),
        args.training_mode,
        args.plan.dry_run,
        args.exec.use_api,
        args.effective_fill_runtime_assets(),
        training_verbose_enabled(args.misc.verbose),
    );

    if args.plan.dry_run && !args.exec.execute {
        if let Some(rec) = &session {
            lock_rec(rec)?.finalize(0, "dry_run_plan", false, None)?;
        }
        println!("  [INFO] Dry run complete. Exiting before ingestion.");
        return Ok(());
    }

    if args.exec.execute {
        eprintln!(
            "  [EXECUTE] training ingest (fail-closed: exit 1 on failures, 2 on zero success)"
        );
    }

    if let Some(rec) = &session {
        let mut fields = Map::new();
        fields.insert("sample_count".to_string(), json!(samples.len()));
        lock_rec(rec)?.set_phase("ingest", Some(fields))?;
    }

    let (success, fail_other, fail_lc) = run_training_isolated(&samples, &args)?;
    println!("  [SUMMARY] success={success}, fail_other={fail_other}, fail_lc={fail_lc}");

    let exit_code = if fail_other + fail_lc > 0 && success == 0 {
        2
    } else if fail_other + fail_lc > 0 {
        1
    } else if success == 0 {
        2
    } else {
        0
    };

    if let Some(rec) = &session {
        lock_rec(rec)?.finalize(exit_code, "completed", false, None)?;
    }

    if exit_code != 0 {
        exit(exit_code);
    }
    Ok(())
}

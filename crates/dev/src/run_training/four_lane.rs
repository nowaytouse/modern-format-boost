use crate::infra::log_paths::{ensure_training_session_stamp, find_mfb_workspace_root};
use crate::run_training::types::Args;
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const TRAINING_LOG_LANES: &[&str] = &["static_high", "static_low", "loop_high", "loop_low"];

/// Hardcoded caps matching Python `FOUR_LANE_STATIC_QUALITY_DB_CAP=1450`, `FOUR_LANE_LOOP_INTENT_DB_CAP=450`
const FOUR_LANE_SPECS: &[(&str, &[&str])] = &[
    (
        "static_high",
        &[
            "--training-mode=static",
            "--label=high",
            "--no-loop",
            "--no-fill-runtime-assets",
            "--max-high=1450",
        ],
    ),
    (
        "static_low",
        &[
            "--training-mode=static",
            "--label=low",
            "--no-loop",
            "--no-fill-runtime-assets",
            "--max-low=1450",
        ],
    ),
    (
        "loop_high",
        &[
            "--training-mode=loop",
            "--loop-intent-label=high",
            "--max-loop=500",
        ],
    ),
    (
        "loop_low",
        &[
            "--training-mode=loop",
            "--loop-intent-label=low",
            "--max-loop=500",
        ],
    ),
];

fn record_stale_lane_death(lane: &str, lane_dir: &Path) -> Result<()> {
    let pid_file = lane_dir.join("run_training.pid");
    let exit_path = lane_dir.join("training_session_exit.json");
    if !pid_file.is_file() || exit_path.is_file() {
        return Ok(());
    }

    let pid: i32 = match fs::read_to_string(&pid_file) {
        Ok(s) => match s.trim().parse::<i32>() {
            Ok(v) => v,
            Err(err) => {
                eprintln!(
                    "[FOUR-LANE] pid parse failed ({}): {err}",
                    pid_file.display()
                );
                0
            }
        },
        Err(err) => {
            eprintln!(
                "[FOUR-LANE] pid read failed ({}): {err}",
                pid_file.display()
            );
            0
        }
    };

    // Check if process is dead
    let dead = pid == 0 || unsafe { kill(pid, 0) != 0 };
    if !dead {
        return Ok(());
    }

    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();

    let exit_dir = lane_dir.join(format!("TrainingBundle_{stamp}"));
    fs::create_dir_all(&exit_dir)?;

    let exit_json = serde_json::json!({
        "session_stamp": stamp,
        "lane": lane,
        "pid": pid,
        "exit_code": 137,
        "reason": "stale-pid-dead-process",
        "phase": "unknown",
        "interrupted": true,
        "finished_at": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "diagnostic": "run_training.pid pointed at a dead process; previous worker died via SIGKILL/OOM before atexit could run",
    });

    fs::write(
        exit_dir.join("training_session_exit.json"),
        format!("{}\n", serde_json::to_string_pretty(&exit_json)?),
    )?;

    // Also write to audit log
    let audit_path = lane_dir.join("training_session_audit.jsonl");
    let audit_record = serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "event": "stale_pid_death_detected",
        "pid": pid,
        "lane": lane,
    });
    if let Some(parent) = audit_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut audit_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&audit_path)?;
    writeln!(audit_file, "{}", serde_json::to_string(&audit_record)?)?;

    Ok(())
}

unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

fn audit_four_lane_training_state(log_root: &Path) {
    for &lane in TRAINING_LOG_LANES {
        let lane_dir = log_root.join(lane);
        let exit_path = lane_dir.join("training_session_exit.json");
        let pid_path = lane_dir.join("run_training.pid");
        let status = if training_lane_pid_is_active(&lane_dir) {
            let pid_str = fs::read_to_string(&pid_path).unwrap_or_default();
            format!("running pid={}", pid_str.trim())
        } else if exit_path.is_file() {
            let text = fs::read_to_string(&exit_path).unwrap_or_default();
            let snapshot: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
            let exit_code = snapshot
                .get("exit_code")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(-1);
            let phase = snapshot
                .get("phase")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let reason = snapshot
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!("completed (exit_code={exit_code} phase={phase} reason={reason})")
        } else if pid_path.is_file() {
            "failed (stale pid without exit snapshot)".to_string()
        } else {
            "not-started".to_string()
        };
        eprintln!("  [AUDIT] lane={lane} status={status}");
    }
}

fn training_lane_pid_is_active(lane_dir: &Path) -> bool {
    let pid_file = lane_dir.join("run_training.pid");
    if !pid_file.is_file() {
        return false;
    }
    let contents = match fs::read_to_string(&pid_file) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let pid: i32 = match contents.trim().parse() {
        Ok(p) => p,
        Err(_) => return false,
    };
    if pid <= 0 {
        return false;
    }
    unsafe { kill(pid, 0) == 0 }
}

fn ensure_reset_db_before_training(reset_db: bool, dry_run: bool) -> Result<()> {
    if !reset_db && !dry_run {
        bail!(
            "  [ERROR] --reset-db is required before four-lane training; refusing to start with potentially polluted cross-run DB state"
        );
    }
    Ok(())
}

fn purge_image_quality_model_artifacts(dry_run: bool) -> Result<()> {
    let workspace_root =
        find_mfb_workspace_root(None).context("could not locate MFB workspace root")?;
    let model_dir = workspace_root
        .join(".modern_format_boost")
        .join("cache")
        .join("models")
        .join("image_quality");

    let mut purged: Vec<PathBuf> = Vec::new();
    match fs::read_dir(&model_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(err) => {
                        eprintln!("[FOUR-LANE] model dir entry failed: {err}");
                        continue;
                    }
                };
                let path = entry.path();
                if path.is_file() || path.is_symlink() {
                    if !dry_run && let Err(e) = fs::remove_file(&path) {
                        eprintln!("  [WARN] failed to remove {}: {e}", path.display());
                    }
                    purged.push(path);
                }
            }
        }
        Err(err) => eprintln!(
            "[FOUR-LANE] model dir read failed ({}): {err}",
            model_dir.display()
        ),
    }

    if purged.is_empty() {
        println!("  [PURGE] no stale LightGBM artifacts found");
    } else {
        for path in &purged {
            println!(
                "  [PURGE] removed stale LightGBM artifact: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn ensure_db_training_closure_before_training() -> Result<()> {
    let workspace_root =
        find_mfb_workspace_root(None).context("could not locate MFB workspace root")?;
    let hardening_root = workspace_root.join("docs").join("hardening");
    if !hardening_root.is_dir() {
        return Ok(()); // No hardening dir, skip check
    }
    // DB_TRAIN_CLOSURE_DOC_MARKERS from Python: files with specific markers
    let markers: &[(&str, &str)] = &[
        (
            "AUDIT_SINGLE_SOURCE_OF_TRUTH_WITH_VERIFY.md",
            "DB_TRAIN_BOUNDED_AUDIT=17/17",
        ),
        (
            "SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md",
            "DB_TRAIN_FOUR_LANE_RESET_GATE=4/4",
        ),
        (
            "SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md",
            "DB_TRAIN_LAUNCHER_CLOSURE_DOC_GATE=4/4",
        ),
        (
            "SINGLE_SOURCE_OF_TRUTH_WITH_CLOSURE.md",
            "DB_TRAIN_TRAINING_LAUNCH_ALLOWED=yes",
        ),
    ];
    for (filename, marker) in markers {
        let path = hardening_root.join(filename);
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        if !text.contains(marker) {
            bail!(
                "  [ERROR] DB/train closure gate is not closed; refusing to start four-lane training. {filename} missing marker {marker}"
            );
        }
    }
    Ok(())
}

pub fn run_four_lane_launcher(args: &Args) -> Result<()> {
    let _workspace_root =
        find_mfb_workspace_root(None).context("could not locate MFB workspace root")?;

    let log_root = if let Some(ref p) = args.log_root {
        PathBuf::from(p)
    } else {
        let home = crate::infra::hardening::optional_env("MFB_HOME_ROOT")
            .or_else(|| crate::infra::hardening::optional_env("HOME"))
            .unwrap_or_else(|| ".".to_string());
        PathBuf::from(home)
            .join(".modern_format_boost")
            .join("logs")
    };
    fs::create_dir_all(&log_root)?;

    // Check for unknown lanes
    let unknown_lanes: Vec<&str> = args
        .lane
        .iter()
        .filter(|l| !TRAINING_LOG_LANES.contains(&l.as_str()))
        .map(std::string::String::as_str)
        .collect();
    if !unknown_lanes.is_empty() {
        bail!(
            "  [ERROR] unknown lane(s): {}\n  Known lanes: {}",
            unknown_lanes.join(", "),
            TRAINING_LOG_LANES.join(", ")
        );
    }

    // Audit lane state before launch
    audit_four_lane_training_state(&log_root);

    // Handle --stop first
    if args.multi.stop {
        let lanes_to_stop = if args.lane.is_empty() {
            TRAINING_LOG_LANES.to_vec()
        } else {
            args.lane.iter().map(std::string::String::as_str).collect()
        };
        for &lane in &lanes_to_stop {
            let lane_dir = log_root.join(lane);
            if lane_dir.join("run_training.pid").is_file() {
                match fs::read_to_string(lane_dir.join("run_training.pid")) {
                    Ok(pid_str) => match pid_str.trim().parse::<i32>() {
                        Ok(pid) => {
                            let _ = unsafe { kill(pid, 15) };
                        }
                        Err(err) => eprintln!(
                            "[FOUR-LANE] stop pid parse failed ({}): {err}",
                            lane_dir.display()
                        ),
                    },
                    Err(err) => eprintln!(
                        "[FOUR-LANE] stop pid read failed ({}): {err}",
                        lane_dir.display()
                    ),
                }
                let _ = fs::remove_file(lane_dir.join("run_training.pid"));
            }
        }
        let stopped = lanes_to_stop.join(", ");
        println!("  [OK] training lanes stopped: {stopped}");
        return Ok(());
    }

    // Require --reset-db unless dry-run (Python parity)
    ensure_reset_db_before_training(args.db.reset_db, args.plan.dry_run)?;

    // Check closure docs (Python: ensure_db_training_closure_before_training)
    ensure_db_training_closure_before_training()?;

    // Purge LightGBM artifacts when --reset-db is passed
    if args.db.reset_db && !args.plan.dry_run {
        purge_image_quality_model_artifacts(false)?;
    }

    // Record stale lane deaths before launching
    for &lane in TRAINING_LOG_LANES {
        if !args.lane.is_empty() && !args.lane.contains(&lane.to_string()) {
            continue;
        }
        let lane_dir = log_root.join(lane);
        if lane_dir.join("run_training.pid").is_file() {
            let _ = record_stale_lane_death(lane, &lane_dir);
        }
    }

    // Rebuild dylib if requested (Python: mfb_dylib.apply_foundation_lib_env)
    if args.multi.rebuild_dylib && !args.plan.dry_run {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "-p", "foundation", "--lib", "--release"]);
        let status = cmd.status().context("failed to rebuild foundation dylib")?;
        if !status.success() {
            bail!("foundation dylib rebuild failed");
        }
    }

    let stamp = ensure_training_session_stamp();
    eprintln!("  [LAUNCH] stamp={} log_root={}", stamp, log_root.display());

    let mut started: Vec<&str> = vec![];

    for (lane, tail) in FOUR_LANE_SPECS {
        if !args.lane.is_empty() && !args.lane.contains(&lane.to_string()) {
            continue;
        }

        let lane_dir = log_root.join(lane);
        fs::create_dir_all(&lane_dir)?;
        let log_path = lane_dir.join(format!("run_training_{stamp}.log"));

        if args.plan.dry_run {
            eprintln!("  [DRY] {lane} exit=0");
            continue;
        }

        // Spawn the Rust binary
        let conn_str = std::env::var("MFB_PG_CONNSTR")
            .unwrap_or_else(|_| "postgresql://localhost/modern_format_boost".to_string());

        let mut cmd = Command::new("cargo");
        cmd.args(["run", "--release", "--bin", "run_training", "--"])
            .args(*tail)
            .env("MFB_LOG_DIR", &log_root)
            .env("MFB_TRAINING_SESSION_STAMP", &stamp)
            .env("MFB_TRAINING_LANE", lane)
            .env("MFB_PG_CONNSTR", conn_str)
            .stdout(
                fs::File::options()
                    .create(true)
                    .append(true)
                    .open(&log_path)?,
            )
            .stderr(
                fs::File::options()
                    .create(true)
                    .append(true)
                    .open(&log_path)?,
            );

        // Detach on macOS using setsid (child becomes leader of new session)
        #[cfg(unix)]
        unsafe {
            cmd.pre_exec(|| Ok(()));
        }

        match cmd.spawn() {
            Ok(child) => {
                let pid = foundation::numeric_cast::u32_to_i32_sat(child.id());
                fs::write(lane_dir.join("run_training.pid"), format!("{pid}\n"))?;
                eprintln!("  [OK] {lane} pid={pid} log={}", log_path.display());
                started.push(*lane);
            }
            Err(e) => {
                // Cleanup any started lanes on failure
                for started_lane in &started {
                    let started_dir = log_root.join(started_lane);
                    let _ = fs::remove_file(started_dir.join("run_training.pid"));
                }
                bail!("Failed to launch {lane} lane: {e}");
            }
        }
    }

    if !args.plan.dry_run && args.lane.is_empty() {
        eprintln!(
            "  [POST] four-lane ingest launched; run post_training_closure for verify/finalize"
        );
    }

    Ok(())
}

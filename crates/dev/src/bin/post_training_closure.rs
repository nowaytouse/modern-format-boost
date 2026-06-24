//! Post-four-lane training closure.
//!
//! After four-lane training completes: aggregate ingest evidence from lane logs
//! and run verify-stack-readiness. Does NOT run CI. Fills runtime closure
//! artifacts only.
//!
//! Port of `crates/dev/scripts/post_training_closure.py`.

use anyhow::{Context, Result};
use clap::Parser;
use dev::infra::hardening::delegated_exit_code;
use dev::infra::hardening::parse_usize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const LANES: &[&str] = &["static_high", "static_low", "loop_high", "loop_low"];

#[derive(Parser, Debug)]
#[command(about = "After four-lane training: aggregate evidence and run verify-stack-readiness")]
struct Args {
    /// Training session stamp, e.g. 20260605_215749
    #[arg(long, required = true)]
    stamp: String,

    /// Root directory containing per-lane log subdirectories
    #[arg(long)]
    log_root: Option<PathBuf>,

    /// Skip verify-stack-readiness step
    #[arg(long)]
    skip_verify: bool,

    /// Block until all four lanes log a Finished: line, then run closure
    #[arg(long)]
    wait: bool,

    /// Poll interval when --wait (seconds, default 120)
    #[arg(long, default_value = "120.0")]
    poll_sec: f64,

    /// Optional max wait seconds; exit 1 if ingest not finished in time
    #[arg(long)]
    timeout_sec: Option<f64>,
}

#[derive(Debug, Default)]
#[allow(dead_code)]
struct LaneStat {
    ok: usize,
    fail: usize,
    label_conflict: usize,
    missing: bool,
    pending: bool,
}

fn parse_finished(log_text: &str) -> Option<(usize, usize, usize)> {
    // Match: "Finished: N OK, M FAIL (K label/score conflict)"
    let re_base = regex_simple_finished(log_text)?;
    Some(re_base)
}

fn regex_simple_finished(text: &str) -> Option<(usize, usize, usize)> {
    let marker = "Finished:";
    let pos = text.find(marker)?;
    let rest = &text[pos + marker.len()..];
    let rest = rest.trim_start();
    let (ok_str, rest) = rest.split_once(" OK,")?;
    let ok = parse_usize(ok_str, "Finished OK count")?;
    let rest = rest.trim_start();
    let (fail_str, rest2) = match rest.split_once(" FAIL") {
        Some(pair) => pair,
        None => (rest, ""),
    };
    let fail = parse_usize(fail_str, "Finished FAIL count")?;
    let lc = if let Some(inner) = rest2.find('(').and_then(|s| {
        let after = &rest2[s + 1..];
        after.find(')').map(|e| &after[..e])
    }) {
        match inner.split_whitespace().next() {
            Some(token) => parse_usize(token, "label/score conflict").unwrap_or_default(),
            None => 0,
        }
    } else {
        0
    };
    Some((ok, fail, lc))
}

fn aggregate_stamp(log_root: &Path, stamp: &str) -> HashMap<String, LaneStat> {
    let mut out = HashMap::new();
    for &lane in LANES {
        let log_path = log_root.join(lane).join("run_training.log");
        // Also try stamped filename
        let log_path_stamped = log_root
            .join(lane)
            .join(format!("run_training_{stamp}.log"));
        let actual = if log_path.is_file() {
            log_path
        } else if log_path_stamped.is_file() {
            log_path_stamped
        } else {
            out.insert(
                lane.to_string(),
                LaneStat {
                    missing: true,
                    ..Default::default()
                },
            );
            continue;
        };
        let text = fs::read_to_string(&actual).unwrap_or_default();
        match parse_finished(&text) {
            None => {
                out.insert(
                    lane.to_string(),
                    LaneStat {
                        pending: true,
                        ..Default::default()
                    },
                );
            }
            Some((ok, fail, lc)) => {
                out.insert(
                    lane.to_string(),
                    LaneStat {
                        ok,
                        fail,
                        label_conflict: lc,
                        ..Default::default()
                    },
                );
            }
        }
    }
    out
}

fn all_lanes_finished(log_root: &Path, stamp: &str) -> bool {
    for &lane in LANES {
        let p1 = log_root.join(lane).join("run_training.log");
        let p2 = log_root
            .join(lane)
            .join(format!("run_training_{stamp}.log"));
        let path = if p1.is_file() { p1 } else { p2 };
        if !path.is_file() {
            return false;
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        if parse_finished(&text).is_none() {
            return false;
        }
    }
    true
}

fn wait_for_lanes(log_root: &Path, stamp: &str, poll_sec: f64, timeout_sec: Option<f64>) -> bool {
    let start = Instant::now();
    loop {
        if all_lanes_finished(log_root, stamp) {
            return true;
        }
        if let Some(t) = timeout_sec
            && start.elapsed().as_secs_f64() >= t
        {
            return false;
        }
        eprintln!(
            "  [WAIT] stamp={stamp} lanes still running; poll again in {:.0}s",
            poll_sec
        );
        std::thread::sleep(Duration::from_secs_f64(poll_sec));
    }
}

fn write_runtime_evidence(
    repo_root: &Path,
    stamp: &str,
    summary: &HashMap<String, LaneStat>,
) -> Result<PathBuf> {
    let now = chrono_now_date();
    let total_ok: usize = summary.values().map(|v| v.ok).sum();
    let total_fail: usize = summary.values().map(|v| v.fail).sum();
    let pending = summary.values().any(|v| v.pending);

    let mut lines = vec![
        format!("# RUNTIME_EVIDENCE — stamp {stamp}"),
        String::new(),
        format!("**Date:** {now}"),
        "**Generated by:** post_training_closure (Rust)".to_string(),
        String::new(),
        "| Lane | OK | FAIL | label_conflict | pending |".to_string(),
        "|------|-----|------|----------------|---------|".to_string(),
    ];
    for &lane in LANES {
        let s = summary
            .get(lane)
            .map(|v| (v.ok, v.fail, v.label_conflict, v.pending))
            .unwrap_or((0, 0, 0, false));
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            lane,
            s.0,
            s.1,
            s.2,
            if s.3 { "yes" } else { "no" }
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "| **Total OK** | **{total_ok}** | **Total FAIL** | **{total_fail}** | pending={pending} |"
    ));
    lines.push(String::new());
    lines.push(format!(
        "**Log root:** `{}`",
        log_root_display(repo_root, stamp)
    ));
    lines.push(String::new());

    let evidence_path = repo_root.join(format!("RUNTIME_EVIDENCE_{stamp}.md"));
    fs::write(&evidence_path, lines.join("\n") + "\n")?;

    if !pending && total_ok > 0 {
        let baseline = repo_root.join("RUNTIME_BASELINE.md");
        let append = format!(
            "\n## Auto-append {now} stamp {stamp}\n\n- Four-lane ingest aggregate: **{total_ok} \
             OK**, **{total_fail} FAIL**\n- Detail: \
             [`RUNTIME_EVIDENCE_{stamp}.md`](RUNTIME_EVIDENCE_{stamp}.md)\n"
        );
        if baseline.is_file() {
            let existing = fs::read_to_string(&baseline).unwrap_or_default();
            fs::write(&baseline, existing + &append)?;
        }
    }
    Ok(evidence_path)
}

fn training_pipeline_cmd(repo_root: &Path, subcommand: &str, conn_str: &str) -> Command {
    let bin = repo_root.join("target/release/training_pipeline");
    let mut cmd = if bin.is_file() {
        Command::new(bin)
    } else {
        let mut c = Command::new("cargo");
        c.args([
            "run",
            "--locked",
            "--release",
            "-p",
            "dev",
            "--bin",
            "training_pipeline",
            "--",
        ]);
        c.current_dir(repo_root);
        c
    };
    cmd.arg(subcommand).arg("--connstr").arg(conn_str);
    cmd.current_dir(repo_root);
    cmd
}

fn run_verify_stack(conn_str: &str, repo_root: &Path, stamp: &str, log_dir: &Path) -> Result<i32> {
    let log_path = log_dir.join(format!("runtime_v1_verify_stack_{stamp}.log"));
    fs::create_dir_all(log_dir)?;
    let log_file = fs::File::create(&log_path)?;
    let err_file = log_file.try_clone()?;

    let status = training_pipeline_cmd(repo_root, "verify-stack-readiness", conn_str)
        .stdout(log_file)
        .stderr(err_file)
        .status()
        .context("failed to run training_pipeline verify-stack-readiness")?;
    let code = delegated_exit_code(status, "training_pipeline", "verify-stack-readiness");

    let now = chrono_now_date();
    let status_str = if code == 0 { "PASS" } else { "FAIL" };
    let block = format!(
        "\n## Auto-run {now} stamp {stamp}\n\n| verify-stack-readiness | {status_str} |\n| Exit \
         code | {code} |\n| Log | {} |\n",
        log_path.display()
    );
    let verify_md = repo_root.join("RUNTIME_VERIFY.md");
    if verify_md.is_file() {
        let existing = fs::read_to_string(&verify_md).unwrap_or_default();
        fs::write(&verify_md, existing + &block)?;
    }
    Ok(code)
}

fn finalize_image_quality_model(
    conn_str: &str,
    repo_root: &Path,
    stamp: &str,
    log_dir: &Path,
) -> Result<i32> {
    let log_path = log_dir.join(format!("runtime_v1_finalize_image_quality_{stamp}.log"));
    fs::create_dir_all(log_dir)?;
    let log_file = fs::File::create(&log_path)?;
    let err_file = log_file.try_clone()?;

    let status = training_pipeline_cmd(repo_root, "finalize-image-quality-model", conn_str)
        .stdout(log_file)
        .stderr(err_file)
        .status()
        .context("failed to run training_pipeline finalize-image-quality-model")?;
    let code = delegated_exit_code(status, "training_pipeline", "verify-stack-readiness");

    let now = chrono_now_date();
    let status_str = if code == 0 { "PASS" } else { "FAIL" };
    let block = format!(
        "\n## Auto-run {now} stamp {stamp}\n\n| finalize-image-quality-model | {status_str} |\n| \
         Exit code | {code} |\n| Log | {} |\n",
        log_path.display()
    );
    let finalize_md = repo_root.join("RUNTIME_FINALIZE.md");
    let existing = if finalize_md.is_file() {
        fs::read_to_string(&finalize_md).unwrap_or_default()
    } else {
        String::new()
    };
    fs::write(&finalize_md, existing + &block)?;
    Ok(code)
}

fn write_closure_cycle2(
    repo_root: &Path,
    stamp: &str,
    verify_code: Option<i32>,
    pending: bool,
) -> Result<()> {
    let now = chrono_now_date();
    let verify_status = match (pending, verify_code) {
        (true, _) => "PENDING (lanes running)".to_string(),
        (_, None) => "SKIPPED (--skip-verify)".to_string(),
        (_, Some(0)) => "PASS (exit 0)".to_string(),
        (_, Some(c)) => format!("FAIL (exit {c})"),
    };
    let blocked = pending || verify_code.map(|c| c != 0).unwrap_or(false);
    let verdict = if blocked { "BLOCKED" } else { "PASS" };

    let content = format!(
        "# CLOSURE — Cycle 2 (Runtime) — stamp {stamp}\n\n**Date:** {now}\n**Contract:** \
         [`CLOSURE_CONTRACT_CYCLE2.md`](CLOSURE_CONTRACT_CYCLE2.md)\n\n| Gate | Status \
         |\n|------|--------|\n| Four-lane ingest evidence | {} |\n| verify-stack-readiness | \
         {verify_status} |\n\n**Verdict:** {verdict}\n\nCI quality remains outside this closure \
         (see [`PROJECT_SIGNOFF.md`](PROJECT_SIGNOFF.md)).\n",
        if pending { "PENDING" } else { "RECORDED" }
    );
    fs::write(repo_root.join("CLOSURE_CYCLE2.md"), content)?;
    Ok(())
}

#[allow(dead_code)]
fn find_python(repo_root: &Path) -> PathBuf {
    let venv_py = repo_root.join("crates/.modern_format_boost/.venv/bin/python");
    if venv_py.is_file() {
        venv_py
    } else {
        PathBuf::from("python3")
    }
}

fn default_log_dir(repo_root: &Path) -> PathBuf {
    // Mirror mfb_log_paths.persistent_log_dir()
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".modern_format_boost/logs"))
        .unwrap_or_else(|_| repo_root.join("logs"))
}

fn log_root_display(_repo_root: &Path, stamp: &str) -> String {
    format!("~/.modern_format_boost/logs/*/run_training_{stamp}.log")
}

fn chrono_now_date() -> String {
    // RFC 3339 date without external dep
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = now / 86400;
    // Compute year/month/day from days since epoch (1970-01-01)
    let (y, m, d) = days_to_ymd(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn days_to_ymd(mut days: i64) -> (i64, i64, i64) {
    let mut y = 1970i64;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let dy = if leap { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let months = [
        31i64,
        28 + leap as i64,
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 1i64;
    for &dm in &months {
        if days < dm {
            break;
        }
        days -= dm;
        m += 1;
    }
    (y, m, days + 1)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let repo_root = std::env::current_dir().context("cannot get cwd")?;
    let log_root = args
        .log_root
        .clone()
        .unwrap_or_else(|| default_log_dir(&repo_root));
    let log_root = log_root.as_path();

    if args.wait && !all_lanes_finished(log_root, &args.stamp) {
        if !wait_for_lanes(log_root, &args.stamp, args.poll_sec, args.timeout_sec) {
            eprintln!("  [TIMEOUT] stamp={} — not all lanes Finished:", args.stamp);
            std::process::exit(1);
        }
        println!(
            "  [WAIT] stamp={} all lanes Finished — running closure",
            args.stamp
        );
    }

    let conn_str = std::env::var("MFB_PG_CONNSTR")
        .unwrap_or_else(|_| "postgresql://localhost/modern_format_boost".to_string());
    let conn_str = conn_str.trim().to_string();

    let summary = aggregate_stamp(log_root, &args.stamp);
    let pending = summary.values().any(|v| v.pending);

    let evidence = write_runtime_evidence(&repo_root, &args.stamp, &summary)?;
    println!("  [OK] wrote {}", evidence.display());

    let persistent_log = default_log_dir(&repo_root);
    let mut finalize_code: Option<i32> = None;
    let mut verify_code: Option<i32> = None;

    if !args.skip_verify && !pending {
        let total_ok: usize = summary.values().map(|v| v.ok).sum();
        if total_ok > 0 {
            finalize_code = Some(finalize_image_quality_model(
                &conn_str,
                &repo_root,
                &args.stamp,
                &persistent_log,
            )?);
            println!(
                "  [FINALIZE] finalize-image-quality-model exit={}",
                finalize_code.unwrap()
            );
            verify_code = Some(run_verify_stack(
                &conn_str,
                &repo_root,
                &args.stamp,
                &persistent_log,
            )?);
            println!(
                "  [VERIFY] verify-stack-readiness exit={}",
                verify_code.unwrap()
            );
        } else {
            println!("  [SKIP] verify-stack-readiness — zero OK ingests");
            verify_code = Some(1);
        }
    } else if pending {
        println!("  [SKIP] verify — one or more lanes still running (no Finished line)");
    } else {
        println!("  [SKIP] verify — --skip-verify");
    }

    write_closure_cycle2(&repo_root, &args.stamp, verify_code, pending)?;
    let blocked = pending
        || finalize_code.map(|c| c != 0).unwrap_or(false)
        || verify_code.map(|c| c != 0).unwrap_or(false);
    println!(
        "  [OK] wrote CLOSURE_CYCLE2.md verdict={}",
        if blocked { "BLOCKED" } else { "PASS" }
    );
    std::process::exit(if blocked { 1 } else { 0 });
}

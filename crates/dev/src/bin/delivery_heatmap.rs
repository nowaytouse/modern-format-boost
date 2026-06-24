//! Delivery heatmap auditor for Rust conversion bins.
//!
//! Port of `crates/dev/scripts/media_conversion_delivery_heatmap.py`.
//!
//! Scans `crates/*/src/bin/*.rs` delivery files for production segment tags,
//! cross-checks an allowlist, and prints a coverage report or deep-audits
//! each bin for undeclared segments.
//!
//! Usage:
//!   cargo run --locked -p dev --bin delivery_heatmap
//!   cargo run --locked -p dev --bin delivery_heatmap -- --report
//!   cargo run --locked -p dev --bin delivery_heatmap -- --deep-audit
//!   cargo run --locked -p dev --bin delivery_heatmap -- --check  # exits
//! non-zero on gap

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Tag that marks a Rust bin as owning a delivery segment.
/// Mirrors the Python `PRODUCTION_SEGMENT_TAG` constant.
const PRODUCTION_SEGMENT_TAG: &str = "//! PRODUCTION_SEGMENT:";

/// Default allowlist file relative to project root.
const DEFAULT_ALLOWLIST_PATH: &str = "crates/dev/scripts/delivery_allowlist.toml";

#[derive(Parser, Debug)]
#[command(
    name = "delivery_heatmap",
    about = "Audit Rust conversion bins for delivery segment coverage (port of \
             media_conversion_delivery_heatmap.py)"
)]
struct Args {
    /// Print segment report (default when no flag given)
    #[arg(long)]
    report: bool,

    /// Deep audit: scan every bin and check for missing allowlist entries
    #[arg(long)]
    deep_audit: bool,

    /// Check mode: exit non-zero if any gap found
    #[arg(long)]
    check: bool,

    /// Path to project root (default: auto-detect)
    #[arg(long)]
    root: Option<PathBuf>,

    /// Emit JSON instead of human-readable output
    #[arg(long)]
    json: bool,
}

fn project_root(override_path: Option<&PathBuf>) -> PathBuf {
    if let Some(p) = override_path {
        return p.clone();
    }
    let mut dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    for _ in 0..16 {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return dir;
        }
        if let Some(parent) = dir.parent() {
            dir = parent.to_path_buf();
        } else {
            break;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Extract `//! PRODUCTION_SEGMENT: <value>` from a source file.
fn production_segment(path: &Path) -> Option<String> {
    let text = dev::infra::hardening::read_text_file(path)?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(PRODUCTION_SEGMENT_TAG) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Collect all `*.rs` files under `crates/*/src/bin/` and
/// `crates/*/src/bin/**/*.rs`.
fn iter_delivery_rs(root: &Path) -> Result<Vec<PathBuf>> {
    let crates_dir = root.join("crates");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&crates_dir)
        .with_context(|| format!("reading crates dir: {}", crates_dir.display()))?
    {
        let entry = entry?;
        let bin_dir = entry.path().join("src").join("bin");
        if !bin_dir.is_dir() {
            continue;
        }
        collect_rs_recursive(&bin_dir, &mut out)?;
    }
    out.sort();
    Ok(out)
}

fn collect_rs_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("reading dir: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rs_recursive(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Load allowlist from a simple TOML/text file.
/// Format: one `"segment" = "reason"` pair per line, or bare `segment` lines.
/// Falls back gracefully if file is missing.
fn load_allowlist(root: &Path) -> Vec<(String, String)> {
    let path = root.join(DEFAULT_ALLOWLIST_PATH);
    if !path.is_file() {
        return Vec::new();
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Try `"segment" = "reason"` TOML style
        if let Some((k, v)) = line.split_once('=') {
            let seg = k.trim().trim_matches('"').to_string();
            let reason = v.trim().trim_matches('"').to_string();
            if !seg.is_empty() {
                out.push((seg, reason));
                continue;
            }
        }
        // Bare segment name
        if !line.is_empty() {
            out.push((line.to_string(), String::new()));
        }
    }
    out
}

fn run_report(root: &Path, json: bool) -> Result<usize> {
    let bins = iter_delivery_rs(root)?;
    let mut segment_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut untagged: Vec<String> = Vec::new();

    for path in &bins {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        if let Some(seg) = production_segment(path) {
            segment_map.entry(seg).or_default().push(rel);
        } else {
            untagged.push(rel);
        }
    }

    if json {
        let doc = serde_json_simple_report(&segment_map, &untagged);
        println!("{doc}");
    } else {
        println!("=== Delivery Heatmap Report ===");
        println!("  Total bins: {}", bins.len());
        println!("  Tagged:     {}", bins.len() - untagged.len());
        println!("  Untagged:   {}", untagged.len());
        println!();
        for (seg, files) in &segment_map {
            println!("  [SEGMENT] {seg}");
            for f in files {
                println!("    {f}");
            }
        }
        if !untagged.is_empty() {
            println!();
            println!(
                "  [UNTAGGED] — no {} comment",
                PRODUCTION_SEGMENT_TAG.trim()
            );
            for f in &untagged {
                println!("    {f}");
            }
        }
    }
    Ok(untagged.len())
}

fn run_deep_audit(root: &Path, check: bool, json: bool) -> Result<()> {
    let bins = iter_delivery_rs(root)?;
    let allowlist: HashSet<String> = load_allowlist(root)
        .into_iter()
        .map(|(seg, _)| seg)
        .collect();

    let mut gaps: Vec<(String, String)> = Vec::new();
    let mut covered: Vec<(String, String)> = Vec::new();

    for path in &bins {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        match production_segment(path) {
            Some(seg) => {
                if allowlist.is_empty() || allowlist.contains(&seg) {
                    covered.push((rel, seg));
                } else {
                    gaps.push((rel, seg));
                }
            }
            None => {
                if check {
                    gaps.push((rel, "<untagged>".to_string()));
                }
            }
        }
    }

    if json {
        println!("{{\"covered\":{},\"gaps\":{}}}", covered.len(), gaps.len());
    } else {
        println!("=== Deep Audit ===");
        println!("  Covered: {}", covered.len());
        for (f, seg) in &covered {
            println!("    {} → {}", f, seg);
        }
        if !gaps.is_empty() {
            println!();
            println!("  Gaps (not in allowlist or untagged):");
            for (f, seg) in &gaps {
                println!("    {} → {}", f, seg);
            }
        }
    }

    if check && !gaps.is_empty() {
        eprintln!(
            "  [ERROR] {} gap(s) found — add PRODUCTION_SEGMENT tags or update allowlist",
            gaps.len()
        );
        std::process::exit(1);
    }

    Ok(())
}

fn serde_json_simple_report(
    segment_map: &BTreeMap<String, Vec<String>>,
    untagged: &[String],
) -> String {
    let mut s = String::from("{");
    s.push_str("\"segments\":{");
    for (i, (seg, files)) in segment_map.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(&seg.replace('"', "\\\""));
        s.push_str("\":[");
        for (j, f) in files.iter().enumerate() {
            if j > 0 {
                s.push(',');
            }
            s.push('"');
            s.push_str(&f.replace('"', "\\\""));
            s.push('"');
        }
        s.push(']');
    }
    s.push_str("},\"untagged\":[");
    for (i, f) in untagged.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push('"');
        s.push_str(&f.replace('"', "\\\""));
        s.push('"');
    }
    s.push_str("]}");
    s
}

fn main() -> Result<()> {
    let args = Args::parse();
    let root = project_root(args.root.as_ref());

    let do_report = args.report || (!args.deep_audit && !args.check);

    if do_report {
        let untagged_count = run_report(&root, args.json)?;
        if args.check && untagged_count > 0 {
            eprintln!(
                "  [ERROR] {} untagged bin(s) found — add {} comments",
                untagged_count,
                PRODUCTION_SEGMENT_TAG.trim()
            );
            std::process::exit(1);
        }
    }

    if args.deep_audit || args.check {
        run_deep_audit(&root, args.check, args.json)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_production_segment_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_bin.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "//! PRODUCTION_SEGMENT: img_convert").unwrap();
        writeln!(f, "fn main() {{}}").unwrap();
        assert_eq!(production_segment(&path), Some("img_convert".to_string()));
    }

    #[test]
    fn test_production_segment_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no_tag.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();
        assert_eq!(production_segment(&path), None);
    }

    #[test]
    fn test_iter_delivery_rs_returns_vec() {
        // Just ensure it runs without panic on the workspace root (or returns empty)
        let root = project_root(None);
        let result = iter_delivery_rs(&root);
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_allowlist_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let list = load_allowlist(dir.path());
        assert!(list.is_empty());
    }

    #[test]
    fn test_serde_json_simple_report_valid_json() {
        let mut map = BTreeMap::new();
        map.insert(
            "img".to_string(),
            vec!["crates/dev/src/bin/foo.rs".to_string()],
        );
        let json = serde_json_simple_report(&map, &["bar.rs".to_string()]);
        assert!(json.starts_with('{'));
        assert!(json.contains("\"img\""));
        assert!(json.contains("\"bar.rs\""));
    }
}

//! Drag-and-drop session helpers — parity with `drag_and_drop_processor.py`.

use crate::infra::fastmode_paths::{
    fast_img_output_dir_for_target, fast_img_restore_output_dir_for_target,
    fast_vid_output_dir_for_target,
};
use crate::infra::hardening::delegated_exit_code;
use crate::infra::log_paths::append_jsonl_audit_record;
use crate::infra::rich_panel::draw_separator;
use crate::infra::ui_tokens::{colors_enabled, pick_symbol};
use crate::media::scope::format_bytes;
use crate::media::scope::{
    PIPELINE_IMAGE, PIPELINE_VIDEO, classify_media_owner, detect_true_format, format_audit_event,
    format_session_audit_routed,
};
use anyhow::{Context, Result, bail};
use foundation::process_lock::{DirLock, acquire_dir_lock};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

const FAST_IMG_RETRY_STAGES: &[&str] = &["gate1_failed", "gate2_failed", "gate3_failed"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingFilter {
    Both,
    ImagesOnly,
    VideosOnly,
}

impl ProcessingFilter {
    #[must_use]
    pub const fn accepts_image(&self) -> bool {
        matches!(self, Self::Both | Self::ImagesOnly)
    }

    #[must_use]
    pub const fn accepts_video(&self) -> bool {
        matches!(self, Self::Both | Self::VideosOnly)
    }

    #[must_use]
    pub const fn verify_mode_label(&self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::ImagesOnly => "images_only",
            Self::VideosOnly => "videos_only",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastImgAction {
    ShortestPath,
    Normal,
    RestoreJpeg,
}

impl FastImgAction {
    #[must_use]
    pub const fn shortest_path(self) -> bool {
        matches!(self, Self::ShortestPath)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ContentScan {
    pub total_files: usize,
    pub img_count: usize,
    pub vid_count: usize,
    pub xmp_count: usize,
    pub other_count: usize,
    pub media_total_size: u64,
    pub routed_images: Vec<String>,
    pub routed_videos: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyOutcome {
    pub stdout: String,
    pub warnings: Option<bool>,
    pub issue_count: usize,
    pub exit_code: i32,
}

/// Block system/user root paths (mirrors Python `safety_check`).
pub fn safety_check(target: &Path) -> Result<()> {
    let resolved = target
        .canonicalize()
        .with_context(|| format!("resolve safety target {}", target.display()))?;
    let s = resolved.to_string_lossy();
    let system_unsafe = ["/", "/System", "/usr", "/bin", "/sbin"];
    for root in system_unsafe {
        if s == root || s.starts_with(&format!("{root}/")) {
            bail!("safety block: system or root directories cannot be processed directly");
        }
    }
    if let Some(home) = dirs_home() {
        let home_s = home.to_string_lossy();
        let user_unsafe = [
            home_s.to_string(),
            home.join("Desktop").to_string_lossy().into_owned(),
            home.join("Documents").to_string_lossy().into_owned(),
        ];
        for p in user_unsafe {
            if s == p {
                bail!(
                    "safety block: common user folders cannot be processed directly; use a \
                     subdirectory"
                );
            }
        }
    }
    Ok(())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Unique output path with `` (N)`` suffix (mirrors Python
/// `get_unique_output_path`).
#[must_use]
pub fn get_unique_output_path(base_path: &Path) -> PathBuf {
    if !base_path.exists() {
        return base_path.to_path_buf();
    }
    let parent = base_path.parent().unwrap_or_else(|| Path::new("."));
    let name = base_path.file_name().map_or_else(
        || "output".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut counter = 1usize;
    loop {
        let candidate = parent.join(format!("{name} ({counter})"));
        if !candidate.exists() {
            return candidate;
        }
        counter += 1;
    }
}

fn audit_line(session_audit: Option<&Path>, line: &str) -> Result<()> {
    if let Some(path) = session_audit {
        append_jsonl_audit_record(path, line)?;
    }
    Ok(())
}

/// Mirror directory tree metadata into adjacent output (Python
/// `create_directory_structure`).
pub fn create_directory_structure(
    src: &Path,
    dest: &Path,
    session_audit: Option<&Path>,
) -> Result<()> {
    fs::create_dir_all(dest)
        .with_context(|| format!("create output directory {}", dest.display()))?;
    clone_dir_metadata(src, dest, session_audit)?;
    for entry in WalkDir::new(src)
        .min_depth(1)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_dir() {
            let rel = entry.path().strip_prefix(src).with_context(|| {
                format!(
                    "strip prefix {} from {}",
                    src.display(),
                    entry.path().display()
                )
            })?;
            let dest_dir = dest.join(rel);
            fs::create_dir_all(&dest_dir)
                .with_context(|| format!("create mirrored directory {}", dest_dir.display()))?;
            clone_dir_metadata(entry.path(), &dest_dir, session_audit)?;
        }
    }
    Ok(())
}

fn clone_dir_metadata(src: &Path, dest: &Path, session_audit: Option<&Path>) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(src_meta), Ok(_dest_meta)) = (src.metadata(), dest.metadata()) {
            let times = libc::timespec {
                tv_sec: src_meta.mtime(),
                tv_nsec: src_meta.mtime_nsec(),
            };
            let timespec = [times, times];
            let ret = unsafe {
                libc::utimensat(
                    libc::AT_FDCWD,
                    std::ffi::CString::new(dest.as_os_str().as_encoded_bytes())
                        .map_err(|e| anyhow::anyhow!("path encoding: {e}"))?
                        .as_ptr(),
                    timespec.as_ptr(),
                    0,
                )
            };
            if ret != 0 {
                let mut fields = HashMap::new();
                fields.insert("source".to_string(), src.display().to_string());
                fields.insert("dest".to_string(), dest.display().to_string());
                fields.insert("error".to_string(), "utimensat failed".to_string());
                let _ = audit_line(
                    session_audit,
                    &format_audit_event("DIR_METADATA_CLONE_DEGRADED", &fields),
                );
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (src, dest, session_audit);
    }
    Ok(())
}

/// Scan target tree and record routing audit lines (Python `count_files`).
pub fn scan_content(
    target: &Path,
    filter: ProcessingFilter,
    session_audit: Option<&Path>,
) -> Result<ContentScan> {
    draw_separator("Scanning Content");
    let dim = if colors_enabled() { "\x1b[2m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    println!("{dim}   Analyzing directory structure...{reset}");

    let mut scan = ContentScan::default();
    let mut routed_images = std::collections::HashSet::new();
    let mut routed_videos = std::collections::HashSet::new();

    for entry in WalkDir::new(target).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        scan.total_files += 1;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let route = match classify_media_owner(path) {
            Ok(owner) => owner,
            Err(err) => {
                let mut fields = HashMap::new();
                fields.insert(
                    "path".to_string(),
                    path.strip_prefix(target)
                        .unwrap_or(path)
                        .display()
                        .to_string(),
                );
                fields.insert("error".to_string(), err.to_string());
                let _ = audit_line(
                    session_audit,
                    &format_audit_event("ROUTING_PROBE_FAILED", &fields),
                );
                None
            }
        };
        let is_img = route.as_deref() == Some(PIPELINE_IMAGE);
        let is_vid = route.as_deref() == Some(PIPELINE_VIDEO);
        if is_img {
            scan.img_count += 1;
            routed_images.insert(
                path.strip_prefix(target)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
        if is_vid {
            scan.vid_count += 1;
            routed_videos.insert(
                path.strip_prefix(target)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
        if ext == "xmp" {
            scan.xmp_count += 1;
        } else if !is_img && !is_vid {
            scan.other_count += 1;
        }
        let include_size = (filter.accepts_image() && is_img) || (filter.accepts_video() && is_vid);
        if include_size {
            match path.metadata() {
                Ok(meta) => scan.media_total_size += meta.len(),
                Err(err) => eprintln!("[DRAG] metadata failed ({}): {err}", path.display()),
            }
        }
    }

    if !filter.accepts_video() {
        scan.vid_count = 0;
        routed_videos.clear();
    }
    if !filter.accepts_image() {
        scan.img_count = 0;
        routed_images.clear();
    }

    scan.routed_images = routed_images.into_iter().collect();
    scan.routed_videos = routed_videos.into_iter().collect();
    scan.routed_images.sort();
    scan.routed_videos.sort();

    let bold = if colors_enabled() { "\x1b[1m" } else { "" };
    let cyan = if colors_enabled() { "\x1b[36m" } else { "" };
    let magenta = if colors_enabled() { "\x1b[35m" } else { "" };
    println!(
        "   {} Total Files: {bold}{}{reset}",
        pick_symbol("📁", "[DIR]"),
        scan.total_files
    );
    println!(
        "   {}  Images:      {bold}{cyan}{}{reset}",
        pick_symbol("🖼️", "[IMG]"),
        scan.img_count
    );
    println!(
        "   {} Videos:      {bold}{magenta}{}{reset}",
        pick_symbol("🎬", "[VID]"),
        scan.vid_count
    );
    println!(
        "   {} Metadata:    {bold}{dim}{}{reset}",
        pick_symbol("📋", "[XMP]"),
        scan.xmp_count
    );
    println!(
        "   {} Others:      {bold}{dim}{}{reset} (Copy only)\n",
        pick_symbol("📦", "[OTH]"),
        scan.other_count
    );

    let _ = audit_line(
        session_audit,
        &format!(
            "ROUTING_SUMMARY images={} videos={} mode={}",
            scan.routed_images.len(),
            scan.routed_videos.len(),
            filter.verify_mode_label()
        ),
    );
    for rel in &scan.routed_images {
        let _ = audit_line(
            session_audit,
            &format_session_audit_routed(PIPELINE_IMAGE, rel),
        );
    }
    for rel in &scan.routed_videos {
        let _ = audit_line(
            session_audit,
            &format_session_audit_routed(PIPELINE_VIDEO, rel),
        );
    }
    Ok(scan)
}

pub fn choose_fast_img_strategy() -> Result<String> {
    let green = if colors_enabled() { "\x1b[32m" } else { "" };
    let bold = if colors_enabled() { "\x1b[1m" } else { "" };
    let dim = if colors_enabled() { "\x1b[2m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    println!("\n{green}FAST MODE STRATEGY{reset}");
    println!("   {bold}1{reset} - {green}Default (JXL){reset}");
    println!("       {dim}Fast lossless recompression of JPEGs to JXL.{reset}");
    println!("   {bold}2{reset} - {green}AVIF (表情包模式){reset}");
    println!("       {dim}Meme Mode: Strict static encoding of images to AVIF.{reset}");
    let answer = read_line(&format!(
        "\n   {bold}Choose strategy [1/2] ({green}Enter = Default{reset}{bold}): {reset}"
    ))?;
    Ok(if answer.trim() == "2" {
        "avif".to_string()
    } else {
        "jxl".to_string()
    })
}

pub fn choose_fast_img_action(strategy: &str) -> Result<FastImgAction> {
    let green = if colors_enabled() { "\x1b[32m" } else { "" };
    let cyan = if colors_enabled() { "\x1b[36m" } else { "" };
    let bold = if colors_enabled() { "\x1b[1m" } else { "" };
    let dim = if colors_enabled() { "\x1b[2m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    println!("\n{green}FAST MODE SELECTED{reset}{cyan} [{strategy}]{reset}");
    println!("   {bold}1{reset} - {green}Shortest Path (Default){reset}");
    if strategy == "avif" {
        println!(
            "       {dim}AVIF-only (表情包模式) delivery, strict verification, automatic iCloud Photos import, then \
             local AVIF folder cleanup.{reset}"
        );
    } else {
        println!(
            "       {dim}JXL-only delivery, strict verification, automatic iCloud Photos import, then \
             local JXL folder cleanup.{reset}"
        );
    }
    println!("   {bold}2{reset} - {cyan}Normal Mode{reset}");
    if strategy == "avif" {
        println!(
            "       {dim}AVIF-only adjacent output; user imports manually. Source images are still \
             deleted after strict verification.{reset}"
        );
    } else {
        println!(
            "       {dim}JXL-only adjacent output; user imports manually. Source JPEGs are still \
             deleted after strict verification.{reset}"
        );
    }
    println!("   {bold}3{reset} - {cyan}Restore to JPEG{reset}");
    println!(
        "       {dim}Decode JXL outputs back to adjacent JPEGs with metadata and folder structure \
         preserved.{reset}"
    );
    let answer = read_line(&format!(
        "\n   {bold}Choose Fast Mode path [1/2/3] ({green}Enter = Shortest Path{reset}{bold}): \
         {reset}"
    ))?;
    Ok(match answer.trim() {
        "2" => FastImgAction::Normal,
        "3" => FastImgAction::RestoreJpeg,
        _ => FastImgAction::ShortestPath,
    })
}

pub fn choose_fast_vid_shortest_path(strategy: &str) -> Result<bool> {
    let green = if colors_enabled() { "\x1b[32m" } else { "" };
    let cyan = if colors_enabled() { "\x1b[36m" } else { "" };
    let bold = if colors_enabled() { "\x1b[1m" } else { "" };
    let dim = if colors_enabled() { "\x1b[2m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    println!("\n{green}FAST VIDEO MODE SELECTED{reset}{cyan} [{strategy}]{reset}");
    println!("   {bold}1{reset} - {green}Shortest Path (Default){reset}");
    if strategy == "avif" {
        println!(
            "       {dim}AVIF-only (表情包模式) animated image delivery, no loop intent judgment.{reset}"
        );
    } else {
        println!(
            "       {dim}Full LoopIntent video and animated-image delivery through Rust vid \
             run.{reset}"
        );
    }
    println!("   {bold}2{reset} - {cyan}Normal Mode{reset}");
    println!("       {dim}Full vid pipeline adjacent output with archive-quality settings.{reset}");
    let answer = read_line(&format!(
        "\n   {bold}Choose Fast Video path [1/2] ({green}Enter = Shortest Path{reset}{bold}): \
         {reset}"
    ))?;
    Ok(answer.trim() != "2")
}

pub fn confirm_in_place() -> Result<bool> {
    let red = if colors_enabled() { "\x1b[31m" } else { "" };
    let yellow = if colors_enabled() { "\x1b[33m" } else { "" };
    let bold = if colors_enabled() { "\x1b[1m" } else { "" };
    let white = if colors_enabled() { "\x1b[37m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    println!("\n{red}WARNING: IN-PLACE OPTIMIZATION SELECTED{reset}");
    println!("{bold}{white}   Original files will be replaced after successful conversion.{reset}");
    println!("{yellow}   This action is irreversible if you don't have backups.{reset}\n");
    let confirm = read_line(&format!(
        "   {bold}To proceed, type {red}'yes'{reset}{bold} (case-sensitive) and press Enter: \
         {reset}"
    ))?;
    Ok(confirm == "yes")
}

pub fn acquire_global_lock(dir_path: &Path) -> Result<DirLock> {
    acquire_dir_lock(dir_path).map_err(|err| {
        anyhow::anyhow!(
            "{} Directory already in use: {} — {}",
            pick_symbol("❌", "[ERROR]"),
            dir_path.display(),
            err
        )
    })
}

#[must_use]
pub fn marker_exists(verify_bin: &Path, optimized_dir: &Path) -> bool {
    match load_fast_img_marker_json(verify_bin, optimized_dir) {
        Ok((marker, _, err)) => marker.is_some() && err.is_none(),
        Err(err) => {
            eprintln!(
                "[DRAG] fast-img marker probe failed ({}): {err}",
                optimized_dir.display()
            );
            false
        }
    }
}

pub fn load_fast_img_marker_json(
    verify_bin: &Path,
    optimized_dir: &Path,
) -> Result<(Option<Value>, Option<PathBuf>, Option<String>)> {
    if !verify_bin.is_file() {
        bail!("verify binary missing at {}", verify_bin.display());
    }
    let out = Command::new(verify_bin)
        .arg("--fast-img-marker-json")
        .arg(optimized_dir)
        .output()
        .with_context(|| format!("probe fast-img marker for {}", optimized_dir.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "fast-img marker probe failed ({}): {}",
            delegated_exit_code(out.status, "verify", "load_fast_img_marker_json"),
            stderr.trim()
        );
    }
    let payload: Value = serde_json::from_slice(&out.stdout).context("parse marker JSON")?;
    let marker = payload.get("marker").cloned();
    let marker_path = payload
        .get("marker_path")
        .and_then(Value::as_str)
        .map(PathBuf::from);
    let marker_error = payload
        .get("marker_error")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((marker, marker_path, marker_error))
}

pub fn fast_img_marker_requires_retry(verify_bin: &Path, output_dir: &Path) -> Result<bool> {
    let (marker, _, marker_error) = load_fast_img_marker_json(verify_bin, output_dir)?;
    if marker_error.is_some() || marker.is_none() {
        return Ok(false);
    }
    let stage = marker
        .as_ref()
        .and_then(|m| m.get("stage"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let failed_sources = marker
        .as_ref()
        .and_then(|m| m.get("failed_sources"))
        .and_then(Value::as_object)
        .is_some_and(|failed| !failed.is_empty());
    Ok(FAST_IMG_RETRY_STAGES.contains(&stage.as_str())
        || (stage == "cleanup_complete" && failed_sources))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyFastImgMode {
    None,
    Delivery,
    Restore,
}

#[derive(Debug, Clone, Copy)]
pub struct VerificationFlags {
    pub include_logs: bool,
    pub auto_mode: bool,
    pub fast_img: VerifyFastImgMode,
}

pub fn run_unified_verification(
    verify_bin: &Path,
    src_dir: &Path,
    opt_dir: Option<&Path>,
    processing_filter: ProcessingFilter,
    session_audit: Option<&Path>,
    log_dir: &Path,
    session_stamp: &str,
    flags: VerificationFlags,
) -> Result<VerifyOutcome> {
    let mut cmd = Command::new(verify_bin);
    if let Some(opt) = opt_dir {
        cmd.arg("--verify").arg(src_dir).arg(opt);
        cmd.arg("--mode").arg(processing_filter.verify_mode_label());
        match flags.fast_img {
            VerifyFastImgMode::Delivery => {
                cmd.arg("--fast-img-delivery");
            }
            VerifyFastImgMode::Restore => {
                cmd.arg("--fast-img-restore");
            }
            VerifyFastImgMode::None => {}
        }
    }
    if flags.include_logs {
        let img_log = log_dir.join(format!("img_run_{session_stamp}.log"));
        let vid_log = log_dir.join(format!("vid_run_{session_stamp}.log"));
        let mut added = img_log.is_file();
        if added {
            cmd.arg(img_log);
        }
        if vid_log.is_file() {
            cmd.arg(vid_log);
            added = true;
        }
        if !added {
            cmd.arg(log_dir);
        }
    }
    if let Some(audit) = session_audit {
        cmd.arg("--session-audit").arg(audit);
    }
    if flags.auto_mode {
        cmd.arg("--print-integrity-summary");
    }
    let output = cmd
        .output()
        .with_context(|| format!("run {}", verify_bin.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    if flags.auto_mode && !stdout.is_empty() {
        print!("{stdout}");
        if !stdout.ends_with('\n') {
            println!();
        }
    }
    let warnings = regex_integrity_status(&stdout);
    let issue_count = regex_integrity_issues(&stdout).unwrap_or_default();
    Ok(VerifyOutcome {
        stdout,
        warnings,
        issue_count,
        exit_code: delegated_exit_code(output.status, "verify", "run_unified_verification"),
    })
}

fn regex_integrity_status(stdout: &str) -> Option<bool> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Integrity:") {
            let state = rest.trim();
            return Some(state == "WARNINGS");
        }
    }
    None
}

fn regex_integrity_issues(stdout: &str) -> Option<usize> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Integrity Issues:") {
            return match rest.trim().parse::<usize>() {
                Ok(n) => Some(n),
                Err(err) => {
                    eprintln!("[DRAG] integrity issue count parse failed: {err}");
                    None
                }
            };
        }
    }
    None
}

pub fn sync_non_media_files(
    target: &Path,
    output: &Path,
    scan: &ContentScan,
    img_bin: &Path,
    session_audit: Option<&Path>,
) -> Result<()> {
    draw_separator("Syncing Non-Media Files");
    let rsync = if Path::new("/opt/homebrew/opt/rsync/bin/rsync").is_file() {
        "/opt/homebrew/opt/rsync/bin/rsync"
    } else {
        "rsync"
    };
    let mut excludes = vec!["--exclude=*.[xX][mM][pP]".to_string()];
    if scan.img_count > 0 {
        for rel in &scan.routed_images {
            excludes.push(format!("--exclude=/{}", Path::new(rel).display()));
        }
    }
    if scan.vid_count > 0 {
        for rel in &scan.routed_videos {
            excludes.push(format!("--exclude=/{}", Path::new(rel).display()));
        }
    }
    let _ = audit_line(
        session_audit,
        &format!("RSYNC_START excludes={}", excludes.len()),
    );
    for exclude in &excludes {
        let _ = audit_line(session_audit, &format!("RSYNC_EXCLUDE {exclude}"));
    }
    let src = format!("{}/", target.display());
    let dest = format!("{}/", output.display());
    let mut cmd = vec![
        rsync.to_string(),
        "-av".to_string(),
        "--ignore-existing".to_string(),
    ];
    cmd.extend(excludes);
    cmd.push(src);
    cmd.push(dest);
    let _ = audit_line(session_audit, &format!("RSYNC_CMD {}", cmd.join(" ")));
    let proc = Command::new(rsync)
        .args(&cmd[1..])
        .output()
        .with_context(|| "run rsync for non-media sync")?;
    if proc.status.success() {
        let _ = audit_line(session_audit, "RSYNC_OK");
        println!("   {} Non-media files synced.", pick_symbol("✅", "[OK]"));
    } else {
        let stderr = String::from_utf8_lossy(&proc.stderr);
        let _ = audit_line(
            session_audit,
            &format!(
                "RSYNC_FAIL code={} stderr={}",
                delegated_exit_code(proc.status, rsync, "sync_non_media_files rsync"),
                stderr.chars().take(500).collect::<String>()
            ),
        );
        eprintln!(
            "   {} rsync exited {} (see session log).",
            pick_symbol("⚠", "[WARN]"),
            delegated_exit_code(proc.status, rsync, "sync_non_media_files rsync")
        );
    }

    let ts_cmd = [
        img_bin.to_string_lossy().to_string(),
        "restore-timestamps".to_string(),
        target.display().to_string(),
        output.display().to_string(),
    ];
    let _ = audit_line(
        session_audit,
        &format!("TIMESTAMP_RESTORE_CMD {}", ts_cmd.join(" ")),
    );
    let ts_proc = Command::new(img_bin)
        .arg("restore-timestamps")
        .arg(target)
        .arg(output)
        .output()
        .with_context(|| "run img restore-timestamps")?;
    if ts_proc.status.success() {
        let _ = audit_line(session_audit, "TIMESTAMP_RESTORE_OK");
        println!("   {} Timestamps restored.", pick_symbol("✅", "[OK]"));
    } else {
        let stderr = String::from_utf8_lossy(&ts_proc.stderr);
        let _ = audit_line(
            session_audit,
            &format!(
                "TIMESTAMP_RESTORE_FAIL code={} stderr={}",
                delegated_exit_code(
                    ts_proc.status,
                    "img",
                    "sync_non_media_files restore-timestamps"
                ),
                stderr.chars().take(300).collect::<String>()
            ),
        );
        eprintln!(
            "   {} Timestamp restore exited {} (see session log).",
            pick_symbol("⚠", "[WARN]"),
            delegated_exit_code(
                ts_proc.status,
                "img",
                "sync_non_media_files restore-timestamps"
            )
        );
    }
    Ok(())
}

#[must_use]
pub fn effective_success_failure_counts(
    total_success: usize,
    total_failed: usize,
    verify_warnings: Option<bool>,
    verify_issue_count: usize,
) -> (usize, usize, usize) {
    let integrity_penalty = if verify_warnings == Some(true) {
        verify_issue_count.max(1)
    } else {
        0
    };
    let effective_success = total_success.saturating_sub(integrity_penalty);
    let effective_failed = total_failed + integrity_penalty;
    (effective_success, effective_failed, integrity_penalty)
}

#[must_use]
pub fn build_size_comparison_summary(
    before_bytes: u64,
    after_bytes: u64,
    operation_mode: &str,
    processing_type: &str,
) -> String {
    let diff = foundation::numeric_cast::u64_to_i64_sat(after_bytes)
        - foundation::numeric_cast::u64_to_i64_sat(before_bytes);
    let change_pct = if before_bytes > 0 {
        Some(
            ((foundation::numeric_cast::u64_to_f64(after_bytes)
                / foundation::numeric_cast::u64_to_f64(before_bytes))
                - 1.0)
                * 100.0,
        )
    } else {
        None
    };
    format!(
        "{} Before/After Size Comparison\n   Operation Mode:  {operation_mode}\n   Processing Type: {processing_type}\n   Total Before:    {}\n   Total After:     {}\n   Difference:      {}\n   Change:          {}",
        pick_symbol("📊", "[STATS]"),
        format_bytes(before_bytes),
        format_bytes(after_bytes),
        signed_bytes_label(diff),
        signed_percent_label(change_pct),
    )
}

fn signed_bytes_label(diff_bytes: i64) -> String {
    match diff_bytes.cmp(&0) {
        std::cmp::Ordering::Less => {
            format!("-{}", format_bytes(diff_bytes.unsigned_abs()))
        }
        std::cmp::Ordering::Greater => {
            format!("+{}", format_bytes(diff_bytes.unsigned_abs()))
        }
        std::cmp::Ordering::Equal => format_bytes(0),
    }
}

fn signed_percent_label(value: Option<f64>) -> String {
    match value {
        None => "N/A".to_string(),
        Some(v) if v >= 0.0 => format!("+{v:.1}%"),
        Some(v) => format!("{v:.1}%"),
    }
}

#[must_use]
pub fn adjacent_output_for_target(target: &Path) -> PathBuf {
    let name = target.file_name().map_or_else(
        || "output".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    get_unique_output_path(&target.with_file_name(format!("{name}_optimized")))
}

#[must_use]
pub fn resolve_output_for_fast_img(
    target: &Path,
    action: FastImgAction,
    verify_bin: &Path,
) -> PathBuf {
    match action {
        FastImgAction::RestoreJpeg => fast_img_restore_output_dir_for_target(target),
        FastImgAction::ShortestPath | FastImgAction::Normal => fast_img_output_dir_for_target(
            target,
            Some(&|dir: &Path| marker_exists(verify_bin, dir)),
        ),
    }
}

#[must_use]
pub fn resolve_output_for_fast_vid(target: &Path) -> PathBuf {
    fast_vid_output_dir_for_target(target)
}

const FAST_IMG_CLEANUP_IGNORABLE_FILES: &[&str] = &[".DS_Store"];

#[derive(Debug, Clone, Copy, Default)]
pub struct FastImgIntegrityCounts {
    pub source_count: usize,
    pub optimized_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FastImgRestoreIntegrityCounts {
    pub source_jxl_count: usize,
    pub restored_jpeg_count: usize,
    pub source_remaining_jxls: usize,
    pub verified_deleted_jxls: usize,
}

fn integrity_summary_int(summary: &str, label: &str) -> Option<usize> {
    let needle = format!("{label}:");
    for line in summary.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&needle) {
            return match rest.trim().parse::<usize>() {
                Ok(n) => Some(n),
                Err(err) => {
                    eprintln!("[DRAG] summary int parse failed for {label}: {err}");
                    None
                }
            };
        }
    }
    None
}

/// Parse fast-img delivery integrity summary lines (mirrors Python
/// `fast_img_integrity_counts`).
#[must_use]
pub fn fast_img_integrity_counts(summary: &str) -> Option<FastImgIntegrityCounts> {
    let source_count = integrity_summary_int(summary, "Recorded source JPEGs")?;
    let optimized_count = integrity_summary_int(summary, "Optimized JXL files")?;
    let skipped_count = integrity_summary_int(summary, "Recorded skipped JPEGs")?;
    let failed_count = integrity_summary_int(summary, "Recorded failed JPEGs").unwrap_or_default();
    Some(FastImgIntegrityCounts {
        source_count,
        optimized_count,
        skipped_count,
        failed_count,
    })
}

/// Parsed fast-img session size metrics from `[SIZE]` stdout lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FastImgSessionSizeMetrics {
    pub source_bytes_actual: Option<u64>,
    pub output_bytes_actual: Option<u64>,
    pub files_converted: Option<u64>,
    pub resume_reused_count: Option<u64>,
}

fn parse_size_line_exact_bytes(rest: &str) -> Option<u64> {
    let open = rest.rfind('(')?;
    let close = rest.rfind(" bytes)")?;
    if close <= open {
        return None;
    }
    let digits: String = rest[open + 1..close]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u64>().map_or_else(
        |err| {
            eprintln!("failed to parse fast-img byte count `{digits}`: {err}");
            None
        },
        Some,
    )
}

fn parse_size_metric_u64(label: &str, rest: &str) -> Option<u64> {
    let value = rest.trim();
    value.parse::<u64>().map_or_else(
        |err| {
            eprintln!("failed to parse fast-img metric `{label}` from `{value}`: {err}");
            None
        },
        Some,
    )
}

/// Parse fast-img `[SIZE]` session accounting lines from child stdout/log text.
#[must_use]
pub fn fast_img_session_size_metrics(summary: &str) -> FastImgSessionSizeMetrics {
    let mut metrics = FastImgSessionSizeMetrics::default();
    for line in summary.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("[SIZE    ] source_bytes_actual:") {
            metrics.source_bytes_actual = parse_size_line_exact_bytes(rest);
        } else if let Some(rest) = trimmed.strip_prefix("[SIZE    ] output_bytes_actual:") {
            metrics.output_bytes_actual = parse_size_line_exact_bytes(rest);
        } else if let Some(rest) = trimmed.strip_prefix("[SIZE    ] files_converted:") {
            metrics.files_converted = parse_size_metric_u64("files_converted", rest);
        } else if let Some(rest) = trimmed.strip_prefix("[SIZE    ] resume_reused_count:") {
            metrics.resume_reused_count = parse_size_metric_u64("resume_reused_count", rest);
        }
    }
    metrics
}

/// Parse per-file retained entries from fast-img `[FAIL    ]   rel: reason` and
/// `[SKIP    ]   rel: reason  [SOURCE RETAINED]` lines.
///
/// Returns a `Vec<(String, String)>` where each entry is `("filename: reason", "failed"|"skipped")`,
/// ready for terminal and session-log display.
#[must_use]
pub fn fast_img_retained_file_names(log_text: &str) -> Vec<(String, String)> {
    log_text
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("[FAIL    ]   ")
                && rest.contains(':')
            {
                return Some((rest.to_owned(), "failed".to_owned()));
            } else if let Some(rest) = trimmed.strip_prefix("[SKIP    ]   ")
                && rest.contains(':')
            {
                // Strip the "  [SOURCE RETAINED]" suffix if present for cleaner display
                let clean = rest.strip_suffix("  [SOURCE RETAINED]").unwrap_or(rest);
                return Some((clean.to_owned(), "skipped".to_owned()));
            }
            None
        })
        .collect()
}

/// Parse fast-img restore integrity summary lines (mirrors Python
/// `fast_img_restore_integrity_counts`).
#[must_use]
pub fn fast_img_restore_integrity_counts(summary: &str) -> Option<FastImgRestoreIntegrityCounts> {
    let source_jxl_count = integrity_summary_int(summary, "Source JXL files")?;
    let restored_jpeg_count = integrity_summary_int(summary, "Restored JPEG files")?;
    let source_remaining_jxls = match integrity_summary_int(summary, "Source remaining JXL files") {
        Some(v) => v,
        None => source_jxl_count,
    };
    let verified_deleted_jxls =
        integrity_summary_int(summary, "Manifest verified deleted source JXLs").unwrap_or_default();
    Some(FastImgRestoreIntegrityCounts {
        source_jxl_count,
        restored_jpeg_count,
        source_remaining_jxls,
        verified_deleted_jxls,
    })
}

/// Count true JXL outputs under fast-img directory (mirrors Python
/// `count_fast_img_jxl_outputs`).
pub fn count_fast_img_jxl_outputs(output_dir: &Path) -> Result<(usize, u64)> {
    let mut count = 0usize;
    let mut total_size = 0u64;
    if !output_dir.is_dir() {
        return Ok((count, total_size));
    }
    for entry in WalkDir::new(output_dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let true_format = detect_true_format(path)
            .with_context(|| format!("fast-img output probe failed for {}", path.display()))?;
        if true_format == "jxl" {
            count += 1;
            total_size += path
                .metadata()
                .with_context(|| {
                    format!(
                        "fast-img output stat failed for true JXL {}",
                        path.display()
                    )
                })?
                .len();
        }
    }
    Ok((count, total_size))
}

fn fast_img_marker_entry_out_rel(source_rel: &str, entry: &Value) -> String {
    if let Some(out_rel) = entry.get("out_rel").and_then(Value::as_str)
        && !out_rel.trim().is_empty()
    {
        return out_rel.to_string();
    }
    Path::new(source_rel)
        .with_extension("JXL")
        .to_string_lossy()
        .into_owned()
}

fn fast_img_safe_output_path(output_root: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        bail!("fast-img marker contains unsafe output path: {rel}");
    }
    let root = output_root.canonicalize().with_context(|| {
        format!(
            "canonicalize fast-img output root {}",
            output_root.display()
        )
    })?;
    let target = root.join(rel_path);
    target
        .strip_prefix(&root)
        .with_context(|| format!("fast-img marker output escapes optimized directory: {rel}"))?;
    Ok(target)
}

fn fast_img_marker_cleanup_targets(output_dir: &Path, verify_bin: &Path) -> Result<Vec<PathBuf>> {
    let (marker, marker_path, marker_error) = load_fast_img_marker_json(verify_bin, output_dir)?;
    if let Some(err) = marker_error {
        bail!("{err}");
    }
    let marker = marker.context("fast-img marker missing for optimized directory")?;
    let marker_path = marker_path.context("fast-img marker path missing")?;
    let blake3_log = marker
        .get("blake3_log")
        .and_then(Value::as_object)
        .with_context(|| {
            format!(
                "fast-img marker has invalid blake3_log: {}",
                marker_path.display()
            )
        })?;
    let mut targets = Vec::new();
    for (source_rel, entry) in blake3_log {
        let entry_obj = entry
            .as_object()
            .with_context(|| format!("fast-img marker entry is not an object for {source_rel}"))?;
        let library_asset = entry_obj.get("library_asset");
        if library_asset.is_none() || library_asset.unwrap().is_null() {
            bail!(
                "fast-img cleanup aborted: JXL output was not successfully imported to Photos/iCloud (missing library_asset proof for {source_rel})"
            );
        }
        targets.push(fast_img_safe_output_path(
            output_dir,
            &fast_img_marker_entry_out_rel(source_rel, entry),
        )?);
    }
    targets.sort();
    Ok(targets)
}

fn fast_img_prune_empty_dirs(
    output_root: &Path,
    candidate_dirs: &std::collections::HashSet<PathBuf>,
) -> Result<usize> {
    if !output_root.exists() {
        return Ok(0);
    }
    let root = output_root.canonicalize().with_context(|| {
        format!(
            "canonicalize fast-img output root {}",
            output_root.display()
        )
    })?;
    let mut dirs = std::collections::HashSet::new();
    for candidate in candidate_dirs {
        let mut current = candidate.clone();
        loop {
            let normalized = current.canonicalize().unwrap_or_else(|_| current.clone());
            if normalized.strip_prefix(&root).is_err() {
                break;
            }
            dirs.insert(normalized.clone());
            if normalized == root {
                break;
            }
            current = normalized
                .parent()
                .map_or_else(|| root.clone(), Path::to_path_buf);
        }
    }
    let mut pruned = 0usize;
    let mut sorted: Vec<_> = dirs.into_iter().collect();
    sorted.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in sorted {
        if !directory.is_dir() {
            continue;
        }
        if directory.read_dir()?.next().is_none() {
            fs::remove_dir(&directory)
                .with_context(|| format!("prune empty fast-img dir {}", directory.display()))?;
            pruned += 1;
        }
    }
    Ok(pruned)
}

fn fast_img_remove_ignorable_cleanup_files(output_root: &Path) -> Result<usize> {
    if !output_root.exists() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for entry in WalkDir::new(output_root).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if FAST_IMG_CLEANUP_IGNORABLE_FILES.contains(&name) {
            fs::remove_file(path).with_context(|| {
                format!("remove ignorable fast-img cleanup file {}", path.display())
            })?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Remove marker-recorded JXL outputs after shortest-path iCloud import (Python
/// parity).
pub fn delete_fast_img_shortest_path_output_dir(
    output_dir: &Path,
    verify_bin: &Path,
) -> Result<bool> {
    if !output_dir.exists() {
        return Ok(true);
    }
    if !output_dir.is_dir() {
        bail!(
            "fast-img cleanup target is not a directory: {}",
            output_dir.display()
        );
    }
    let mut deleted = 0usize;
    let mut already_absent = 0usize;
    let mut prune_candidates = std::collections::HashSet::from([output_dir.to_path_buf()]);
    for target in fast_img_marker_cleanup_targets(output_dir, verify_bin)? {
        if !target.exists() {
            already_absent += 1;
            if let Some(parent) = target.parent() {
                prune_candidates.insert(parent.to_path_buf());
            }
            continue;
        }
        if !target.is_file() {
            bail!(
                "fast-img cleanup target is not a file: {}",
                target.display()
            );
        }
        let true_format = detect_true_format(&target)
            .with_context(|| format!("fast-img cleanup probe failed for {}", target.display()))?;
        if true_format != "jxl" {
            bail!(
                "fast-img cleanup refused non-JXL marker output {} (true_format={true_format})",
                target.display()
            );
        }
        fs::remove_file(&target)
            .with_context(|| format!("remove fast-img cleanup target {}", target.display()))?;
        deleted += 1;
        if let Some(parent) = target.parent() {
            prune_candidates.insert(parent.to_path_buf());
        }
    }
    let ignored_removed = fast_img_remove_ignorable_cleanup_files(output_dir)?;
    prune_candidates.insert(output_dir.to_path_buf());
    for entry in WalkDir::new(output_dir).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_dir() {
            prune_candidates.insert(entry.path().to_path_buf());
        }
    }
    let pruned = fast_img_prune_empty_dirs(output_dir, &prune_candidates)?;
    let fully_removed = !output_dir.exists();
    if fully_removed {
        println!(
            "   {} Shortest Path cleanup: removed {deleted} imported JXL file(s) and empty output \
             folder after verified iCloud import: {}",
            pick_symbol("✓", "[OK]"),
            output_dir.display()
        );
    } else {
        println!(
            "   {} Shortest Path cleanup: removed {deleted} imported JXL file(s), already \
             absent={already_absent}, ignored files removed={ignored_removed}, empty dirs \
             pruned={pruned}; preserved residual files in {}",
            pick_symbol("✓", "[OK]"),
            output_dir.display()
        );
    }
    Ok(fully_removed)
}

fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("flush stdout")?;
    let mut line = String::new();
    io::stdin().read_line(&mut line).context("read stdin")?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_counts_apply_integrity_penalty() {
        let (s, f, p) = effective_success_failure_counts(10, 2, Some(true), 3);
        assert_eq!(p, 3);
        assert_eq!(s, 7);
        assert_eq!(f, 5);
    }

    #[test]
    fn unique_output_path_appends_counter_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("Album_optimized");
        fs::create_dir(&base).unwrap();
        let unique = get_unique_output_path(&base);
        assert_eq!(unique, tmp.path().join("Album_optimized (1)"));
    }

    #[test]
    fn fast_img_integrity_counts_parse_delivery_summary() {
        let summary = "Recorded source JPEGs: 10\nOptimized JXL files: 8\nRecorded skipped JPEGs: \
                       1\nRecorded failed JPEGs: 1\n";
        let counts = fast_img_integrity_counts(summary).expect("counts");
        assert_eq!(counts.source_count, 10);
        assert_eq!(counts.optimized_count, 8);
        assert_eq!(counts.skipped_count, 1);
        assert_eq!(counts.failed_count, 1);
    }

    #[test]
    fn fast_img_session_size_metrics_parse_size_lines() {
        let summary = "\
[SIZE    ] resume_reused_count:  12
[SIZE    ] files_converted:      0
[SIZE    ] source_bytes_actual:  11.70 GiB (12,567,890,123 bytes)
[SIZE    ] output_bytes_actual:  8.20 GiB (8,804,321,456 bytes)
";
        let metrics = fast_img_session_size_metrics(summary);
        assert_eq!(metrics.resume_reused_count, Some(12));
        assert_eq!(metrics.files_converted, Some(0));
        assert_eq!(metrics.source_bytes_actual, Some(12_567_890_123));
        assert_eq!(metrics.output_bytes_actual, Some(8_804_321_456));
    }

    #[test]
    fn fast_img_restore_integrity_counts_parse_restore_summary() {
        let summary = "Source JXL files: 5\nRestored JPEG files: 5\n";
        let counts = fast_img_restore_integrity_counts(summary).expect("counts");
        assert_eq!(counts.source_jxl_count, 5);
        assert_eq!(counts.restored_jpeg_count, 5);
    }
}

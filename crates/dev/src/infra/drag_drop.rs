//! Drag-and-drop session helpers — parity with `drag_and_drop_processor.py`.

use crate::infra::fastmode_paths::fast_vid_output_dir_for_target;
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
use serde::{Deserialize, Serialize};
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
    pub entries: Vec<ScanEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanRoute {
    Image,
    Video,
    Sidecar,
    CopyOnly,
}

#[derive(Debug, Clone)]
pub struct ScanEntry {
    pub relative_path: PathBuf,
    pub route: ScanRoute,
    pub selected: bool,
    pub source: foundation::batch::DiscoveredSource,
}

impl ContentScan {
    #[must_use]
    pub fn selected_sources(&self, route: ScanRoute) -> Vec<foundation::batch::DiscoveredSource> {
        self.entries
            .iter()
            .filter(|entry| entry.selected && entry.route == route)
            .map(|entry| entry.source.clone())
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct VerifyOutcome {
    pub warnings: Option<bool>,
    pub issue_count: usize,
    pub integrity_summary: Option<IntegritySummaryMachine>,
}

pub const INTEGRITY_SUMMARY_JSON_PREFIX: &str = "MFB_INTEGRITY_SUMMARY_JSON ";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntegritySummaryMachine {
    pub has_warnings: bool,
    pub issue_count: usize,
    pub source_count: usize,
    pub optimized_count: usize,
    pub skipped_count: usize,
    pub failed_count: usize,
    pub source_remaining_count: usize,
    pub verified_deleted_count: usize,
    #[serde(default)]
    pub count_status: Option<String>,
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
    scan_content_with_link_check(target, filter, session_audit, |resolved| {
        safety_check(resolved)?;
        foundation::safety::check_dangerous_directory(resolved).map_err(anyhow::Error::msg)
    })
}

/// The same inventory policy without progress output, audit writes, or processing.
pub fn inspect_content(target: &Path, filter: ProcessingFilter) -> Result<ContentScan> {
    scan_content_with_options(target, filter, None, false, |resolved| {
        safety_check(resolved)?;
        foundation::safety::check_dangerous_directory(resolved).map_err(anyhow::Error::msg)
    })
}

fn scan_content_with_link_check(
    target: &Path,
    filter: ProcessingFilter,
    session_audit: Option<&Path>,
    check_link: impl Fn(&Path) -> Result<()>,
) -> Result<ContentScan> {
    scan_content_with_options(target, filter, session_audit, true, check_link)
}

fn scan_content_with_options(
    target: &Path,
    filter: ProcessingFilter,
    session_audit: Option<&Path>,
    display_summary: bool,
    check_link: impl Fn(&Path) -> Result<()>,
) -> Result<ContentScan> {
    let dim = if colors_enabled() { "\x1b[2m" } else { "" };
    let reset = if colors_enabled() { "\x1b[0m" } else { "" };
    if display_summary {
        draw_separator("Scanning Content");
        println!("{dim}   Analyzing directory structure...{reset}");
    }

    let mut scan = ContentScan::default();
    let mut routed_images = std::collections::HashSet::new();
    let mut routed_videos = std::collections::HashSet::new();

    let mut root = Some(target.to_path_buf());
    let mut directories = Vec::<(PathBuf, PathBuf, fs::ReadDir)>::new();
    loop {
        let path = if let Some(path) = root.take() {
            path
        } else {
            let Some((directory, _, entries)) = directories.last_mut() else {
                break;
            };
            match entries.next() {
                Some(entry) => entry
                    .with_context(|| {
                        format!(
                            "Cannot read content scan directory {}; inventory is incomplete",
                            directory.display()
                        )
                    })?
                    .path(),
                None => {
                    directories.pop();
                    continue;
                }
            }
        };
        let link_metadata = fs::symlink_metadata(&path).with_context(|| {
            format!(
                "Cannot inspect content scan path {}; inventory is incomplete",
                path.display()
            )
        })?;
        let metadata = if link_metadata.file_type().is_symlink() {
            let resolved = fs::canonicalize(&path).with_context(|| {
                format!(
                    "Cannot resolve content scan link {}; inventory is incomplete",
                    path.display()
                )
            })?;
            check_link(&resolved).map_err(|error| {
                anyhow::anyhow!(
                    "Unsafe content scan link {} -> {}: {error}; inventory is incomplete",
                    path.display(),
                    resolved.display()
                )
            })?;
            fs::metadata(&path).with_context(|| {
                format!(
                    "Cannot stat content scan path {}; inventory is incomplete",
                    path.display()
                )
            })?
        } else {
            link_metadata
        };
        if metadata.is_dir() {
            let resolved = fs::canonicalize(&path).with_context(|| {
                format!(
                    "Cannot resolve content scan directory {}; inventory is incomplete",
                    path.display()
                )
            })?;
            if directories
                .iter()
                .any(|(_, ancestor, _)| ancestor == &resolved)
            {
                bail!(
                    "Cannot complete content scan at {}; link loop; inventory is incomplete",
                    path.display()
                );
            }
            // Validate links before opening their target directories.
            let entries = fs::read_dir(&path).with_context(|| {
                format!(
                    "Cannot open content scan directory {}; inventory is incomplete",
                    path.display()
                )
            })?;
            directories.push((path, resolved, entries));
            continue;
        }
        let path = path.as_path();
        path.file_name().and_then(|n| n.to_str()).with_context(|| {
            format!(
                "Cannot represent content scan path {}; inventory is incomplete",
                path.display()
            )
        })?;
        if foundation::is_hidden_file(path) {
            continue;
        }
        if path.to_str().is_none() {
            bail!(
                "Cannot represent content scan path {}; inventory is incomplete",
                path.display()
            );
        }
        if !metadata.is_file() {
            continue;
        }
        scan.total_files = scan
            .total_files
            .checked_add(1)
            .context("content scan total file count overflow")?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let observed = foundation::batch::DiscoveredSource::from_metadata(path, &metadata)
            .with_context(|| {
                format!(
                    "observe content scan source {}; inventory is incomplete",
                    path.display()
                )
            })?;
        let route = classify_media_owner(path).with_context(|| {
            format!(
                "Cannot classify content scan path {}; inventory is incomplete",
                path.display()
            )
        })?;
        observed.verify_discovery_metadata().with_context(|| {
            format!(
                "source changed during content scan {}; inventory is incomplete",
                path.display()
            )
        })?;
        let is_img = route.as_deref() == Some(PIPELINE_IMAGE);
        let is_vid = route.as_deref() == Some(PIPELINE_VIDEO);
        if is_img {
            scan.img_count = scan
                .img_count
                .checked_add(1)
                .context("content scan image count overflow")?;
            routed_images.insert(
                path.strip_prefix(target)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
        if is_vid {
            scan.vid_count = scan
                .vid_count
                .checked_add(1)
                .context("content scan video count overflow")?;
            routed_videos.insert(
                path.strip_prefix(target)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
            );
        }
        if ext == "xmp" && !is_img && !is_vid {
            scan.xmp_count = scan
                .xmp_count
                .checked_add(1)
                .context("content scan sidecar count overflow")?;
        } else if !is_img && !is_vid {
            scan.other_count = scan
                .other_count
                .checked_add(1)
                .context("content scan other file count overflow")?;
        }
        let include_size = (filter.accepts_image() && is_img) || (filter.accepts_video() && is_vid);
        let relative = path
            .strip_prefix(target)
            .context("content scan source is outside its root")?;
        let relative_path = if relative.as_os_str().is_empty() {
            PathBuf::from(path.file_name().context("content scan file has no name")?)
        } else {
            relative.to_path_buf()
        };
        scan.entries.push(ScanEntry {
            relative_path,
            route: if is_img {
                ScanRoute::Image
            } else if is_vid {
                ScanRoute::Video
            } else if ext == "xmp" {
                ScanRoute::Sidecar
            } else {
                ScanRoute::CopyOnly
            },
            selected: include_size,
            source: observed,
        });
        if include_size {
            scan.media_total_size = scan
                .media_total_size
                .checked_add(metadata.len())
                .context("content scan media byte total overflow")?;
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
    scan.entries
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));

    if display_summary {
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
    }

    audit_line(
        session_audit,
        &format!(
            "ROUTING_SUMMARY images={} videos={} mode={}",
            scan.routed_images.len(),
            scan.routed_videos.len(),
            filter.verify_mode_label()
        ),
    )
    .context("record content scan routing summary")?;
    for rel in &scan.routed_images {
        audit_line(
            session_audit,
            &format_session_audit_routed(PIPELINE_IMAGE, rel),
        )
        .with_context(|| format!("record content scan image route {rel}"))?;
    }
    for rel in &scan.routed_videos {
        audit_line(
            session_audit,
            &format_session_audit_routed(PIPELINE_VIDEO, rel),
        )
        .with_context(|| format!("record content scan video route {rel}"))?;
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
    println!("       {dim}Fast lossless encoding of JPEGs to JXL.{reset}");
    println!("   {bold}2{reset} - {green}AVIF (Meme Mode){reset}");
    println!(
        "       {dim}Meme Mode (表情包模式): lossy static AVIF encoding; may lose metadata or \
         transparency and can fall back when output grows. It ignores Ultimate/Extreme flags and \
         is not an archival mode.{reset}"
    );
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
            "       {dim}AVIF-only (Meme Mode) delivery, verified Photos import, then local AVIF folder cleanup.{reset}"
        );
    } else {
        println!(
            "       {dim}JXL-only delivery, verified Photos import, then local JXL folder cleanup.{reset}"
        );
    }
    println!("   {bold}2{reset} - {cyan}Normal Mode{reset}");
    if strategy == "avif" {
        println!(
            "       {dim}AVIF-only adjacent output; user imports manually. Source images are still deleted after strict verification.{reset}"
        );
    } else {
        println!(
            "       {dim}JXL-only adjacent output; user imports manually. Source JPEGs are still deleted after strict verification.{reset}"
        );
        println!("   {bold}3{reset} - {cyan}Restore to JPEG{reset}");
        println!(
            "       {dim}Decode JXL outputs back to adjacent JPEGs with metadata and folder structure preserved.{reset}"
        );
    }
    let choices = if strategy == "avif" { "1/2" } else { "1/2/3" };
    let answer = read_line(&format!(
        "\n   {bold}Choose Fast Mode path [{choices}] ({green}Enter = Shortest Path{reset}{bold}): {reset}"
    ))?;
    Ok(match answer.trim() {
        "2" => FastImgAction::Normal,
        "3" if strategy != "avif" => FastImgAction::RestoreJpeg,
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
            "       {dim}Meme Mode skips loop-intent judgment. This launcher enables Apple \
             compatibility, so delivery is forced to GIF even when AVIF is selected.{reset}"
        );
    } else {
        println!("       {dim}LoopIntent-gated GIF delivery for animated images and video.{reset}");
    }
    println!("   {bold}2{reset} - {cyan}Normal Mode{reset}");
    println!(
        "       {dim}Verified adjacent GIF/AVIF output without automatic Photos import.{reset}"
    );
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
        .get("marker_database_path")
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
    if let Some(error) = marker_error {
        bail!(
            "fast-img marker is invalid for {}: {error}",
            output_dir.display()
        );
    }
    let Some(marker) = marker else {
        return Ok(false);
    };
    let stage = marker
        .get("stage")
        .and_then(Value::as_str)
        .context("fast-img marker is missing a string stage")?
        .trim()
        .to_ascii_lowercase();
    let has_failed_sources = !marker
        .get("failed_sources")
        .and_then(Value::as_object)
        .context("fast-img marker is missing failed_sources")?
        .is_empty();
    Ok(FAST_IMG_RETRY_STAGES.contains(&stage.as_str())
        || (stage == "cleanup_complete" && has_failed_sources))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyFastImgMode {
    None,
    Delivery,
    Restore,
}

#[derive(Debug, Clone)]
pub struct VerificationFlags {
    pub include_logs: bool,
    pub auto_mode: bool,
    pub fast_img: VerifyFastImgMode,
    pub strategy: Option<String>,
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
        cmd.arg("--print-integrity-json");
    }
    if let Some(strategy) = flags.strategy {
        cmd.arg("--strategy").arg(strategy);
    }
    let output = cmd
        .output()
        .with_context(|| format!("run {}", verify_bin.display()))?;
    let raw_stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    let (stdout, integrity_summary) = if flags.auto_mode {
        let (display, summary) = parse_machine_integrity_summary(&raw_stdout)?;
        (display, Some(summary))
    } else {
        (raw_stdout, None)
    };
    if flags.auto_mode && !stdout.is_empty() {
        print!("{stdout}");
        if !stdout.ends_with('\n') {
            println!();
        }
    }
    let (warnings, issue_count) = match &integrity_summary {
        Some(summary) => (Some(summary.has_warnings), summary.issue_count),
        None => (None, 0),
    };
    let exit_code = delegated_exit_code(output.status, "verify", "run_unified_verification");
    if let Some(summary) = &integrity_summary {
        let expected_success = !summary.has_warnings;
        if output.status.success() != expected_success {
            bail!(
                "verify exit status disagrees with its machine integrity result: exit_code={exit_code} warnings={}",
                summary.has_warnings
            );
        }
    }
    Ok(VerifyOutcome {
        warnings,
        issue_count,
        integrity_summary,
    })
}

fn parse_machine_integrity_summary(stdout: &str) -> Result<(String, IntegritySummaryMachine)> {
    let mut display_lines = Vec::new();
    let mut summary = None;
    for line in stdout.lines() {
        if let Some(json) = line.strip_prefix(INTEGRITY_SUMMARY_JSON_PREFIX) {
            if summary.is_some() {
                bail!("verify emitted more than one machine integrity summary");
            }
            summary =
                Some(serde_json::from_str(json).context("parse verify machine integrity summary")?);
        } else {
            display_lines.push(line);
        }
    }
    let summary = summary.context("verify omitted its machine integrity summary")?;
    let mut display = display_lines.join("\n");
    if stdout.ends_with('\n') && !display.is_empty() {
        display.push('\n');
    }
    Ok((display, summary))
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
pub fn resolve_output_for_fast_vid(target: &Path) -> PathBuf {
    fast_vid_output_dir_for_target(target)
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
    let parsed = (|| {
        let (_, value) = rest.trim_end().rsplit_once('(')?;
        let digits = value.strip_suffix(" bytes)")?;
        let mut groups = digits.split(',');
        let first = groups.next()?;
        if first.is_empty() || !first.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if digits.contains(',')
            && (first.len() > 3
                || groups.any(|group| {
                    group.len() != 3 || !group.bytes().all(|byte| byte.is_ascii_digit())
                }))
        {
            return None;
        }
        parse_size_metric_u64("exact bytes", &digits.replace(',', ""))
    })();
    if parsed.is_none() {
        eprintln!("invalid fast-img exact byte count: {rest}");
    }
    parsed
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
    let byte_evidence = crate::infra::process_stream::parse_stats_from_output(summary);
    if byte_evidence.converted_bytes_invalid
        || byte_evidence
            .converted_bytes
            .is_some_and(|bytes| bytes.input_bytes.is_none() || bytes.output_bytes.is_none())
    {
        return FastImgSessionSizeMetrics::default();
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

/// A partial exit only authorizes independent verification, never cleanup by itself.
pub fn fast_img_partial_cleanup_verified(
    text: &str,
    stats: &crate::infra::process_stream::ProcessorStats,
) -> Option<usize> {
    if stats.exit_code != 1
        || stats.failed == 0
        || !stats.counts_complete()
        || stats.unprocessed != Some(0)
    {
        return None;
    }
    let mut reports = text
        .lines()
        .filter_map(|line| line.strip_prefix("MFB_FAST_IMG_RESULT="));
    let payload = reports.next()?;
    if payload.len() > 16_384 || reports.next().is_some() {
        return None;
    }
    let report: Value = match serde_json::from_str(payload) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("[WARN] invalid fast-img partial-cleanup receipt: {error}");
            return None;
        }
    };
    let count = |key| {
        let value = report.get(key)?.as_u64()?;
        match usize::try_from(value) {
            Ok(value) => Some(value),
            Err(error) => {
                eprintln!("[WARN] fast-img partial-cleanup count {key} is out of range: {error}");
                None
            }
        }
    };
    if count("schema_version") != Some(1)
        || count("succeeded") != Some(stats.succeeded)
        || count("failed") != Some(stats.failed)
        || count("skipped") != Some(stats.skipped)
        || count("ignored") != Some(stats.ignored)
        || count("unprocessed") != Some(0)
        || count("photos_verified") != Some(stats.succeeded)
        || count("encoded").is_none_or(|encoded| encoded > stats.succeeded)
        || count("failed_sources_retained") != Some(stats.failed)
        || count("failed_sources_verified") != Some(stats.failed)
        || report
            .get("primary_cleanup_complete")
            .and_then(Value::as_bool)
            != Some(true)
        || !report.get("finalization_error")?.is_null()
        || !report.get("retention_unknown_reason")?.is_null()
        || !report.get("failed_source_verification_error")?.is_null()
        || count("source_retained").is_none_or(|retained| retained < stats.failed)
    {
        return None;
    }
    Some(stats.failed)
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

fn fast_img_marker_cleanup_targets(output_dir: &Path) -> Result<Vec<(PathBuf, String)>> {
    use foundation::pipeline::verification::{FastImgStageName, read_marker};
    let marker = read_marker(output_dir).context("read fast-img cleanup ownership checkpoint")?;
    anyhow::ensure!(
        marker.stage == FastImgStageName::CleanupComplete
            && !marker.tier2_in_progress
            && marker.gate1_checks.count.0
            && marker.gate1_checks.blake3.0
            && marker.gate1_checks.metadata.0
            && marker.gate1_checks.size.0
            && marker.gate1_checks.orient.0
            && marker.gate1_checks.decode.0
            && marker.gate2_checks.count.0
            && marker.gate2_checks.blake3_sample.0
            && marker.gate2_checks.no_error.0
            && marker.gate3_checks.count_x3.0
            && marker.gate3_checks.sync.0
            && marker.gate3_checks.quarantine.0
            && marker.gate3_checks.chain.0,
        "fast-img cleanup refused an incomplete or unverified checkpoint"
    );
    let mut targets = Vec::new();
    for (source_rel, entry) in &marker.blake3_log {
        anyhow::ensure!(
            entry.out.len() == 64
                && entry.out.bytes().all(|byte| byte.is_ascii_hexdigit())
                && entry.library_asset.as_ref() == Some(&entry.out),
            "fast-img cleanup missing matching Photos custody hash for {source_rel}"
        );
        let relative = entry.out_rel.as_ref().map_or_else(
            || {
                Path::new(source_rel)
                    .with_extension("JXL")
                    .to_string_lossy()
                    .into_owned()
            },
            Clone::clone,
        );
        let target = fast_img_safe_output_path(output_dir, &relative)?;
        targets.push((target, entry.out.clone()));
    }
    targets.sort();
    Ok(targets)
}

#[cfg(unix)]
fn remove_verified_fast_img_output(root: &Path, target: &Path, expected: &str) -> Result<()> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let relative = target.strip_prefix(root)?;
    let mut parent = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        anyhow::ensure!(
            matches!(component, std::path::Component::Normal(_)),
            "unsafe cleanup path"
        );
        let name = CString::new(component.as_os_str().as_bytes())?;
        let is_file = components.peek().is_none();
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | if is_file {
                libc::O_NONBLOCK
            } else {
                libc::O_DIRECTORY
            };
        // Resolve every component relative to an already opened directory; never follow links.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        anyhow::ensure!(
            fd >= 0,
            "open cleanup path {}: {}",
            target.display(),
            io::Error::last_os_error()
        );
        let mut file = unsafe { fs::File::from_raw_fd(fd) };
        if !is_file {
            parent = file;
            continue;
        }
        let before = file.metadata()?;
        anyhow::ensure!(
            before.is_file(),
            "cleanup target is not a regular file: {}",
            target.display()
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update_reader(&mut file)?;
        let after = file.metadata()?;
        anyhow::ensure!(
            hasher.finalize().to_hex().as_str() == expected
                && before.len() == after.len()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "fast-img cleanup refused changed output {}",
            target.display()
        );
        let current_fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        anyhow::ensure!(
            current_fd >= 0,
            "recheck cleanup target {}: {}",
            target.display(),
            io::Error::last_os_error()
        );
        let current = unsafe { fs::File::from_raw_fd(current_fd) }.metadata()?;
        anyhow::ensure!(
            current.dev() == after.dev()
                && current.ino() == after.ino()
                && current.len() == after.len()
                && current.ctime() == after.ctime()
                && current.ctime_nsec() == after.ctime_nsec(),
            "fast-img cleanup target replaced: {}",
            target.display()
        );
        anyhow::ensure!(
            unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0,
            "remove cleanup target {}: {}",
            target.display(),
            io::Error::last_os_error()
        );
        return Ok(());
    }
    bail!("fast-img cleanup target has no file name")
}

#[cfg(not(unix))]
fn remove_verified_fast_img_output(_root: &Path, target: &Path, _expected: &str) -> Result<()> {
    bail!(
        "verified output cleanup requires no-follow directory handles: {}",
        target.display()
    )
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputCleanupOutcome {
    Removed,
    AlreadyAbsent,
    Residual,
    Failed,
    NotRequested,
    VerificationBlocked,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[must_use = "inspect the cleanup outcome and preserve any partial failure"]
pub struct OutputCleanupReceipt {
    pub schema_version: u8,
    pub run_id: Option<String>,
    pub output_path: PathBuf,
    pub outcome: OutputCleanupOutcome,
    pub removed_files: Option<usize>,
    pub already_absent_files: Option<usize>,
    pub pruned_directories: Option<usize>,
    pub error: Option<String>,
}

impl OutputCleanupReceipt {
    pub fn not_requested(output_path: &Path) -> Self {
        Self::without_operation(output_path, OutputCleanupOutcome::NotRequested)
    }

    pub fn verification_blocked(output_path: &Path) -> Self {
        Self::without_operation(output_path, OutputCleanupOutcome::VerificationBlocked)
    }

    fn without_operation(output_path: &Path, outcome: OutputCleanupOutcome) -> Self {
        Self {
            schema_version: 1,
            run_id: None,
            output_path: output_path.to_path_buf(),
            outcome,
            removed_files: None,
            already_absent_files: None,
            pruned_directories: None,
            error: None,
        }
    }
}

fn fast_img_prune_empty_dirs(
    output_root: &Path,
    candidate_dirs: &std::collections::HashSet<PathBuf>,
) -> Result<usize> {
    match fs::symlink_metadata(output_root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Ok(_) => bail!(
            "fast-img empty-directory cleanup root is not a directory: {}",
            output_root.display()
        ),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "inspect fast-img empty-directory cleanup root {}",
                    output_root.display()
                )
            });
        }
    }
    let candidates = candidate_dirs.iter().cloned().collect::<Vec<_>>();
    foundation::io_utils::prune_empty_directories_within(output_root, &candidates).with_context(
        || {
            format!(
                "fast-img refused unsafe empty-directory cleanup under {}",
                output_root.display()
            )
        },
    )
}

pub fn prune_completed_fast_img_empty_outputs(source: &Path, strategy: &str) -> Result<usize> {
    use foundation::pipeline::verification::{FastImgStageName, matching_working_copy_markers};
    let mut removed = 0;
    for marker in matching_working_copy_markers(source, strategy)? {
        if marker.stage != FastImgStageName::CleanupComplete || marker.tier2_in_progress {
            continue;
        }
        if !marker.working_copy.try_exists()? {
            continue;
        }
        let _lock = foundation::process_lock::acquire_output_dir_lock(&marker.working_copy)?;
        let current = foundation::pipeline::verification::read_marker(&marker.working_copy)?;
        anyhow::ensure!(
            current == marker
                && current.stage == FastImgStageName::CleanupComplete
                && !current.tier2_in_progress,
            "checkpoint changed before empty-output cleanup"
        );
        fast_img_marker_cleanup_targets(&marker.working_copy)?;
        match fs::remove_dir(&marker.working_copy) {
            Ok(()) => {
                removed += 1;
                println!(
                    "[CLEANUP ] removed empty checkpoint-owned output {}",
                    marker.working_copy.display()
                );
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "remove empty checkpoint-owned output {}",
                        marker.working_copy.display()
                    )
                });
            }
        }
    }
    Ok(removed)
}

/// Remove only unchanged, marker-recorded outputs after verified local Photos delivery.
pub fn delete_fast_img_shortest_path_output_dir(output_dir: &Path) -> OutputCleanupReceipt {
    let mut receipt =
        OutputCleanupReceipt::without_operation(output_dir, OutputCleanupOutcome::Failed);
    let mut prune_candidates = std::collections::HashSet::new();
    let result = (|| -> Result<bool> {
        let metadata = match fs::symlink_metadata(output_dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                receipt.outcome = OutputCleanupOutcome::AlreadyAbsent;
                return Ok(true);
            }
            Err(error) => return Err(error.into()),
        };
        if !metadata.file_type().is_dir() {
            bail!(
                "fast-img cleanup target is not a directory: {}",
                output_dir.display()
            );
        }
        let canonical_output = output_dir
            .canonicalize()
            .context("canonicalize fast-img cleanup output")?;
        let output_dir = canonical_output.as_path();
        let _lock = foundation::process_lock::acquire_output_dir_lock(output_dir)?;
        let targets = fast_img_marker_cleanup_targets(output_dir)?;
        let mut removed = 0usize;
        let mut already_absent = 0usize;
        receipt.removed_files = Some(removed);
        receipt.already_absent_files = Some(already_absent);
        prune_candidates.insert(output_dir.to_path_buf());
        for (target, expected) in targets {
            match fs::symlink_metadata(&target) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    already_absent += 1;
                    receipt.already_absent_files = Some(already_absent);
                    prune_candidates.extend(
                        target
                            .ancestors()
                            .skip(1)
                            .take_while(|path| path.starts_with(output_dir))
                            .map(Path::to_path_buf),
                    );
                    continue;
                }
                Err(error) => return Err(error.into()),
                Ok(_) => {}
            }
            if !target.is_file() {
                bail!(
                    "fast-img cleanup target is not a file: {}",
                    target.display()
                );
            }
            let true_format = detect_true_format(&target).with_context(|| {
                format!("fast-img cleanup probe failed for {}", target.display())
            })?;
            if !matches!(
                true_format.as_str(),
                "jxl" | "avif" | "webp" | "jpeg" | "png" | "heic" | "gif" | "bmp" | "tiff"
            ) {
                bail!(
                    "fast-img cleanup refused non-image marker output {} (true_format={true_format})",
                    target.display()
                );
            }
            remove_verified_fast_img_output(output_dir, &target, &expected)
                .with_context(|| format!("remove fast-img cleanup target {}", target.display()))?;
            removed += 1;
            receipt.removed_files = Some(removed);
            prune_candidates.extend(
                target
                    .ancestors()
                    .skip(1)
                    .take_while(|path| path.starts_with(output_dir))
                    .map(Path::to_path_buf),
            );
        }
        prune_candidates.insert(output_dir.to_path_buf());
        let pruned = fast_img_prune_empty_dirs(output_dir, &prune_candidates)?;
        receipt.pruned_directories = Some(pruned);
        match fs::symlink_metadata(output_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Ok(metadata) if metadata.file_type().is_dir() => Ok(false),
            Ok(_) => bail!(
                "fast-img cleanup target was replaced with a non-directory: {}",
                output_dir.display()
            ),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "inspect fast-img cleanup target {} after pruning",
                    output_dir.display()
                )
            }),
        }
    })();
    match result {
        Ok(true) if receipt.outcome == OutputCleanupOutcome::AlreadyAbsent => {}
        Ok(true) => receipt.outcome = OutputCleanupOutcome::Removed,
        Ok(false) => receipt.outcome = OutputCleanupOutcome::Residual,
        Err(error) => {
            receipt.outcome = OutputCleanupOutcome::Failed;
            receipt.error = Some(format!("{error:#}"));
        }
    }
    receipt
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
    #[serial_test::serial]
    fn empty_output_cleanup_requires_checkpoint_gates_and_keeps_unknown_contents() {
        use foundation::pipeline::verification::{
            FastImgStageName, WorkingCopyMarker, write_marker_atomic,
        };
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let output = root.path().join("source_optimized");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        let _state = foundation::common_utils::EnvGuard::set(
            "MFB_HOME_ROOT",
            root.path().join("state").to_str().unwrap(),
        );
        let mut marker = WorkingCopyMarker::new(source.clone(), output.clone(), 0);
        marker.stage = FastImgStageName::CleanupComplete;
        write_marker_atomic(&marker).unwrap();
        assert!(prune_completed_fast_img_empty_outputs(&source, "jxl").is_err());
        assert!(output.exists());
        for check in [
            &mut marker.gate1_checks.count,
            &mut marker.gate1_checks.blake3,
            &mut marker.gate1_checks.metadata,
            &mut marker.gate1_checks.size,
            &mut marker.gate1_checks.orient,
            &mut marker.gate1_checks.decode,
            &mut marker.gate2_checks.count,
            &mut marker.gate2_checks.blake3_sample,
            &mut marker.gate2_checks.no_error,
            &mut marker.gate3_checks.count_x3,
            &mut marker.gate3_checks.sync,
            &mut marker.gate3_checks.quarantine,
            &mut marker.gate3_checks.chain,
        ] {
            *check = true.into();
        }
        write_marker_atomic(&marker).unwrap();
        let unknown = output.join("user.txt");
        fs::write(&unknown, b"user data").unwrap();
        assert_eq!(
            prune_completed_fast_img_empty_outputs(&source, "jxl").unwrap(),
            0
        );
        assert_eq!(fs::read(&unknown).unwrap(), b"user data");
        fs::remove_file(unknown).unwrap();
        assert_eq!(
            prune_completed_fast_img_empty_outputs(&source, "jxl").unwrap(),
            1
        );
        assert_eq!(
            prune_completed_fast_img_empty_outputs(&source, "jxl").unwrap(),
            0
        );
    }

    #[test]
    #[serial_test::serial]
    fn output_cleanup_receipt_distinguishes_removed_residual_and_unmeasured_states() {
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgStageName, WorkingCopyMarker, write_marker_atomic,
        };

        let root = tempfile::tempdir().unwrap();
        let _state = foundation::common_utils::EnvGuard::set(
            "MFB_HOME_ROOT",
            root.path().join("state").to_str().unwrap(),
        );
        let missing = root.path().join("missing-output");
        let absent = delete_fast_img_shortest_path_output_dir(&missing);
        assert_eq!(absent.outcome, OutputCleanupOutcome::AlreadyAbsent);
        assert!(absent.removed_files.is_none());
        assert!(absent.already_absent_files.is_none());
        assert!(absent.pruned_directories.is_none());

        let not_requested = OutputCleanupReceipt::not_requested(&missing);
        let blocked = OutputCleanupReceipt::verification_blocked(&missing);
        for receipt in [not_requested, blocked] {
            assert!(receipt.removed_files.is_none());
            assert!(receipt.already_absent_files.is_none());
            assert!(receipt.pruned_directories.is_none());
            assert!(receipt.error.is_none());
        }

        let source = root.path().join("source");
        let output = root.path().join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        let unknown = output.join("user.txt");
        fs::write(&unknown, b"preserve user data").unwrap();
        let source = source.canonicalize().unwrap();
        let output = output.canonicalize().unwrap();
        let mut marker = WorkingCopyMarker::new(source, output.clone(), 0);
        marker.stage = FastImgStageName::CleanupComplete;
        for check in [
            &mut marker.gate1_checks.count,
            &mut marker.gate1_checks.blake3,
            &mut marker.gate1_checks.metadata,
            &mut marker.gate1_checks.size,
            &mut marker.gate1_checks.orient,
            &mut marker.gate1_checks.decode,
            &mut marker.gate2_checks.count,
            &mut marker.gate2_checks.blake3_sample,
            &mut marker.gate2_checks.no_error,
            &mut marker.gate3_checks.count_x3,
            &mut marker.gate3_checks.sync,
            &mut marker.gate3_checks.quarantine,
            &mut marker.gate3_checks.chain,
        ] {
            *check = true.into();
        }
        write_marker_atomic(&marker).unwrap();

        let residual = delete_fast_img_shortest_path_output_dir(&output);
        assert_eq!(
            residual.outcome,
            OutputCleanupOutcome::Residual,
            "cleanup failure: {:?}",
            residual.error
        );
        assert_eq!(residual.removed_files, Some(0));
        assert_eq!(residual.already_absent_files, Some(0));
        assert_eq!(residual.pruned_directories, Some(0));
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve user data");

        fs::remove_file(unknown).unwrap();
        let first_bytes = [0xFF, 0x0A];
        let second_actual_bytes = [0xFF, 0x0A, 0x02];
        let second_expected_bytes = [0xFF, 0x0A, 0x01];
        let first_output = output.join("a.JXL");
        let second_output = output.join("b.JXL");
        fs::write(&first_output, first_bytes).unwrap();
        fs::write(&second_output, second_actual_bytes).unwrap();
        let first_hash = blake3::hash(&first_bytes).to_hex().to_string();
        let second_expected_hash = blake3::hash(&second_expected_bytes).to_hex().to_string();
        let entry = |source: &str, out_rel: &str, hash: String| Blake3Entry {
            out_rel: Some(out_rel.to_owned()),
            src: format!("source/{source}"),
            out: hash.clone(),
            library_asset: Some(hash),
        };
        marker
            .blake3_log
            .insert("a.jpg".to_owned(), entry("a.jpg", "a.JXL", first_hash));
        marker.blake3_log.insert(
            "b.jpg".to_owned(),
            entry("b.jpg", "b.JXL", second_expected_hash),
        );
        write_marker_atomic(&marker).unwrap();

        let failed = delete_fast_img_shortest_path_output_dir(&output);
        assert_eq!(failed.outcome, OutputCleanupOutcome::Failed);
        assert_eq!(failed.removed_files, Some(1));
        assert_eq!(failed.pruned_directories, None);
        assert!(
            failed
                .error
                .as_deref()
                .is_some_and(|error| error.contains("refused changed output"))
        );
        assert!(!first_output.exists());
        assert_eq!(fs::read(&second_output).unwrap(), second_actual_bytes);

        let second_actual_hash = blake3::hash(&second_actual_bytes).to_hex().to_string();
        marker.blake3_log.insert(
            "b.jpg".to_owned(),
            entry("b.jpg", "b.JXL", second_actual_hash),
        );
        write_marker_atomic(&marker).unwrap();
        let removed = delete_fast_img_shortest_path_output_dir(&output);
        assert_eq!(removed.outcome, OutputCleanupOutcome::Removed);
        assert_eq!(removed.removed_files, Some(1));
        assert_eq!(removed.already_absent_files, Some(1));
        assert!(removed.pruned_directories.is_some());
        assert!(!output.exists());
    }

    #[test]
    fn partial_cleanup_requires_complete_current_receipt_and_retained_original_proof() {
        let stats = crate::infra::process_stream::ProcessorStats {
            succeeded: 2,
            failed: 1,
            reported: [true; 4],
            unprocessed: Some(0),
            exit_code: 1,
            ..Default::default()
        };
        let receipt = serde_json::json!({"schema_version": 1, "succeeded": 2, "skipped": 0,
            "ignored": 0, "failed": 1, "unprocessed": 0, "photos_verified": 2, "encoded": 2,
            "failed_sources_retained": 1, "failed_sources_verified": 1, "source_retained": 1,
            "primary_cleanup_complete": true, "finalization_error": null,
            "retention_unknown_reason": null, "failed_source_verification_error": null});
        let event = |value: &serde_json::Value| format!("MFB_FAST_IMG_RESULT={value}");
        assert_eq!(
            fast_img_partial_cleanup_verified(&event(&receipt), &stats),
            Some(1)
        );
        for (key, value) in [
            ("failed_sources_verified", serde_json::Value::Null),
            ("primary_cleanup_complete", false.into()),
            ("photos_verified", 1.into()),
            ("finalization_error", "marker persistence failed".into()),
        ] {
            let mut invalid = receipt.clone();
            invalid[key] = value;
            assert_eq!(
                fast_img_partial_cleanup_verified(&event(&invalid), &stats),
                None,
                "{key}"
            );
        }
        assert_eq!(
            fast_img_partial_cleanup_verified(
                &format!("{}\n{}", event(&receipt), event(&receipt)),
                &stats
            ),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_cleanup_rechecks_content_and_refuses_symlink_components() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("owned.JXL");
        fs::write(&target, b"original").unwrap();
        let expected = blake3::hash(b"original").to_hex().to_string();
        fs::write(&target, b"user replacement").unwrap();
        assert!(remove_verified_fast_img_output(root.path(), &target, &expected).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"user replacement");
        let external = tempfile::tempdir().unwrap();
        fs::write(external.path().join("owned.JXL"), b"original").unwrap();
        std::os::unix::fs::symlink(external.path(), root.path().join("linked")).unwrap();
        assert!(
            remove_verified_fast_img_output(
                root.path(),
                &root.path().join("linked/owned.JXL"),
                &expected
            )
            .is_err()
        );
        assert!(external.path().join("owned.JXL").exists());
        fs::write(&target, b"original").unwrap();
        remove_verified_fast_img_output(root.path(), &target, &expected).unwrap();
        assert!(!target.exists());
    }

    #[test]
    fn read_only_inventory_retains_every_visible_entry_and_selected_source() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("nested")).unwrap();
        fs::write(
            tmp.path().join("nested/spoofed.bin"),
            b"\xff\xd8\xff\xe0synthetic",
        )
        .unwrap();
        fs::write(tmp.path().join("notes.txt"), "synthetic notes").unwrap();
        fs::write(tmp.path().join("metadata.xmp"), "synthetic sidecar").unwrap();
        fs::write(tmp.path().join(".hidden.jpg"), b"\xff\xd8\xff\xe0synthetic").unwrap();
        let before = fs::read(tmp.path().join("nested/spoofed.bin")).unwrap();
        let scan = inspect_content(tmp.path(), ProcessingFilter::Both).unwrap();
        assert_eq!(
            (
                scan.total_files,
                scan.img_count,
                scan.xmp_count,
                scan.other_count
            ),
            (3, 1, 1, 1)
        );
        assert_eq!(scan.entries.len(), scan.total_files);
        assert_eq!(scan.entries[0].route, ScanRoute::Sidecar);
        assert_eq!(
            scan.entries[1].relative_path,
            Path::new("nested/spoofed.bin")
        );
        assert!(scan.entries[1].selected);
        assert_eq!(scan.selected_sources(ScanRoute::Image).len(), 1);
        assert!(scan.selected_sources(ScanRoute::Video).is_empty());
        for source in scan.selected_sources(ScanRoute::Image) {
            source.verify_unchanged().unwrap();
        }
        assert_eq!(
            fs::read(tmp.path().join("nested/spoofed.bin")).unwrap(),
            before
        );
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 4);

        let filtered = inspect_content(tmp.path(), ProcessingFilter::VideosOnly).unwrap();
        assert_eq!(filtered.entries.len(), 3);
        assert!(filtered.entries.iter().all(|entry| !entry.selected));
        assert_eq!(filtered.media_total_size, 0);
        let single = inspect_content(
            &tmp.path().join("nested/spoofed.bin"),
            ProcessingFilter::ImagesOnly,
        )
        .unwrap();
        assert_eq!(single.entries[0].relative_path, Path::new("spoofed.bin"));
        fs::write(tmp.path().join("nested/spoofed.bin"), b"changed source").unwrap();
        assert!(
            scan.selected_sources(ScanRoute::Image)[0]
                .verify_unchanged()
                .is_err()
        );
    }

    #[test]
    fn media_disguised_as_sidecar_is_counted_once_as_media() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("disguised.xmp"),
            b"\xff\xd8\xff\xe0synthetic",
        )
        .unwrap();
        let scan = inspect_content(tmp.path(), ProcessingFilter::Both).unwrap();
        assert_eq!(
            (
                scan.total_files,
                scan.img_count,
                scan.xmp_count,
                scan.other_count
            ),
            (1, 1, 0, 0)
        );
        assert_eq!(scan.entries[0].route, ScanRoute::Image);
    }

    #[test]
    fn scan_content_rejects_incomplete_inventory() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("notes.txt"), "synthetic document").unwrap();
        fs::write(tmp.path().join("metadata.xmp"), "synthetic sidecar").unwrap();
        let scan = scan_content(tmp.path(), ProcessingFilter::Both, None).unwrap();
        assert_eq!(scan.total_files, 2);
        assert_eq!(scan.other_count, 1);
        assert_eq!(scan.xmp_count, 1);
        assert_eq!(scan.img_count, 0);
        assert_eq!(scan.vid_count, 0);

        let missing = tmp.path().join("missing-root");
        let error = scan_content(&missing, ProcessingFilter::Both, None).unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("inventory is incomplete"));
        assert!(diagnostic.contains(&missing.display().to_string()));
        assert!(error.downcast_ref::<io::Error>().is_some());
    }

    #[test]
    fn scan_content_rejects_probe_and_audit_failures() {
        let tmp = tempfile::tempdir().unwrap();
        let malformed = tmp.path().join("truncated.bin");
        for bytes in [&b"GIF89a\x01\x00"[..], &b"\x89PNG\r\n\x1a\n"[..]] {
            fs::write(&malformed, bytes).unwrap();
            let error = scan_content(tmp.path(), ProcessingFilter::Both, None).unwrap_err();
            let diagnostic = format!("{error:#}");
            assert!(diagnostic.contains("inventory is incomplete"));
            assert!(diagnostic.contains(&malformed.display().to_string()));
        }

        fs::remove_file(&malformed).unwrap();
        fs::write(tmp.path().join("notes.txt"), "synthetic document").unwrap();
        let audit_directory = tempfile::tempdir().unwrap();
        let error = scan_content(
            tmp.path(),
            ProcessingFilter::Both,
            Some(audit_directory.path()),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("record content scan"));
    }

    #[cfg(unix)]
    #[test]
    fn scan_content_rejects_unreadable_file_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let dangling = tmp.path().join("dangling.jpg");
        std::os::unix::fs::symlink(tmp.path().join("missing.jpg"), &dangling).unwrap();
        let error = scan_content(tmp.path(), ProcessingFilter::Both, None).unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("inventory is incomplete"));
        assert!(diagnostic.contains(&dangling.display().to_string()));
        let error = scan_content(&dangling, ProcessingFilter::Both, None).unwrap_err();
        assert!(format!("{error:#}").contains("Cannot resolve content scan link"));
    }

    #[cfg(unix)]
    #[test]
    fn scan_content_includes_legal_linked_directory_media() {
        let selected = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let jpeg = b"\xff\xd8\xff\xe0jpeg";
        fs::write(external.path().join("still.bin"), jpeg).unwrap();
        fs::write(external.path().join("sidecar.xmp"), "synthetic sidecar").unwrap();
        std::os::unix::fs::symlink(external.path(), selected.path().join("linked")).unwrap();

        let scan = scan_content(selected.path(), ProcessingFilter::Both, None).unwrap();
        assert_eq!(scan.total_files, 2);
        assert_eq!(scan.img_count, 1);
        assert_eq!(scan.vid_count, 0);
        assert_eq!(scan.xmp_count, 1);
        assert_eq!(scan.other_count, 0);
        assert_eq!(scan.media_total_size, u64::try_from(jpeg.len()).unwrap());
        assert_eq!(scan.routed_images, ["linked/still.bin"]);
        assert!(scan.routed_videos.is_empty());

        let images = foundation::batch::scan_image_files(selected.path(), &["jpg"], true).unwrap();
        assert_eq!(
            images,
            [fs::canonicalize(selected.path())
                .unwrap()
                .join("linked/still.bin")]
        );

        let root_scan = scan_content(
            &selected.path().join("linked"),
            ProcessingFilter::Both,
            None,
        )
        .unwrap();
        assert_eq!(root_scan.total_files, 2);
        assert_eq!(root_scan.routed_images, ["still.bin"]);

        let image = external.path().join("still.bin");
        let linked_file = selected.path().join("single.bin");
        std::os::unix::fs::symlink(&image, &linked_file).unwrap();
        for input in [&image, &linked_file] {
            let scan = scan_content(input, ProcessingFilter::Both, None).unwrap();
            assert_eq!(scan.total_files, 1);
            assert_eq!(scan.img_count, 1);
            assert_eq!(scan.media_total_size, u64::try_from(jpeg.len()).unwrap());
            assert_eq!(scan.routed_images, [""]);
        }
        fs::remove_file(linked_file).unwrap();

        std::os::unix::fs::symlink(external.path(), selected.path().join("alias")).unwrap();
        let aliases = scan_content(selected.path(), ProcessingFilter::Both, None).unwrap();
        assert_eq!(aliases.total_files, 4);
        assert_eq!(aliases.img_count, 2);
        assert_eq!(aliases.xmp_count, 2);
        assert_eq!(
            aliases.routed_images,
            ["alias/still.bin", "linked/still.bin"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn scan_content_rejects_linked_directory_loops_and_dangerous_targets() {
        let selected = tempfile::tempdir().unwrap();
        let looping = selected.path().join("loop");
        std::os::unix::fs::symlink(selected.path(), &looping).unwrap();
        let error = scan_content(selected.path(), ProcessingFilter::Both, None).unwrap_err();
        assert!(format!("{error:#}").contains("inventory is incomplete"));
        assert!(format!("{error:#}").contains(&looping.display().to_string()));
        assert!(format!("{error:#}").contains("link loop"));

        fs::remove_file(&looping).unwrap();
        let rejected = tempfile::tempdir().unwrap();
        fs::write(
            rejected.path().join("must-not-probe.gif"),
            b"GIF89a\x01\x00",
        )
        .unwrap();
        let rejected_path = fs::canonicalize(rejected.path()).unwrap();
        let dangerous = selected.path().join("rejected-target");
        std::os::unix::fs::symlink(rejected.path(), &dangerous).unwrap();
        let reject_target = |resolved: &Path| {
            if resolved == rejected_path {
                bail!("synthetic target rejected");
            }
            Ok(())
        };
        let error = scan_content_with_link_check(
            selected.path(),
            ProcessingFilter::Both,
            None,
            reject_target,
        )
        .unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("Unsafe content scan link"));
        assert!(diagnostic.contains("inventory is incomplete"));
        assert!(diagnostic.contains(&dangerous.display().to_string()));
        assert!(diagnostic.contains("synthetic target rejected"));
        let error =
            scan_content_with_link_check(&dangerous, ProcessingFilter::Both, None, reject_target)
                .unwrap_err();
        assert!(format!("{error:#}").contains("synthetic target rejected"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scan_content_rejects_unrepresentable_paths() {
        use std::os::unix::ffi::OsStringExt;

        let tmp = tempfile::tempdir().unwrap();
        let invalid_name = std::ffi::OsString::from_vec(b"invalid-\xff.jpg".to_vec());
        fs::write(tmp.path().join(invalid_name), "synthetic document").unwrap();
        let error = scan_content(tmp.path(), ProcessingFilter::Both, None).unwrap_err();
        assert!(format!("{error:#}").contains("Cannot represent content scan path"));

        let tmp = tempfile::tempdir().unwrap();
        let invalid_directory = std::ffi::OsString::from_vec(b"invalid-\xff".to_vec());
        let directory = tmp.path().join(invalid_directory);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("notes.txt"), "synthetic document").unwrap();
        let error = scan_content(tmp.path(), ProcessingFilter::Both, None).unwrap_err();
        assert!(format!("{error:#}").contains("Cannot represent content scan path"));
    }

    #[test]
    fn scan_content_preserves_content_routing_and_exact_filtered_size() {
        let tmp = tempfile::tempdir().unwrap();
        let jpeg = b"\xff\xd8\xff\xe0jpeg";
        fs::write(tmp.path().join("still.bin"), jpeg).unwrap();
        fs::write(tmp.path().join("fake.jpg"), "not media").unwrap();
        fs::write(tmp.path().join("sidecar.xmp"), "synthetic sidecar").unwrap();
        fs::write(tmp.path().join(".hidden.jpg"), jpeg).unwrap();
        let scan = scan_content(tmp.path(), ProcessingFilter::ImagesOnly, None).unwrap();
        assert_eq!(scan.total_files, 3);
        assert_eq!(scan.img_count, 1);
        assert_eq!(scan.other_count, 1);
        assert_eq!(scan.xmp_count, 1);
        assert_eq!(scan.media_total_size, u64::try_from(jpeg.len()).unwrap());
        assert_eq!(scan.routed_images, ["still.bin"]);

        let scan = scan_content(tmp.path(), ProcessingFilter::VideosOnly, None).unwrap();
        assert_eq!(scan.total_files, 3);
        assert_eq!(scan.img_count, 0);
        assert_eq!(scan.media_total_size, 0);
        assert!(scan.routed_images.is_empty());
    }

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
    fn machine_integrity_summary_is_required_and_removed_from_display_output() {
        let stdout = concat!(
            "[CHECK] Integrity summary\n",
            "Integrity:      CLEAN\n",
            "MFB_INTEGRITY_SUMMARY_JSON {\"has_warnings\":false,\"issue_count\":0,",
            "\"source_count\":56,\"optimized_count\":56,\"skipped_count\":0,",
            "\"failed_count\":0,\"source_remaining_count\":0,",
            "\"verified_deleted_count\":56}\n",
        );
        let (display, summary) = parse_machine_integrity_summary(stdout).unwrap();
        assert!(!display.contains("MFB_INTEGRITY_SUMMARY_JSON"));
        assert_eq!(summary.source_count, 56);
        assert_eq!(summary.optimized_count, 56);
        assert_eq!(summary.verified_deleted_count, 56);
        assert_eq!(summary.count_status, None);
        let current = stdout.replace(
            "\"verified_deleted_count\":56}",
            "\"verified_deleted_count\":56,\"count_status\":\"MATCH (1 expected handoff gap)\"}",
        );
        assert_eq!(
            parse_machine_integrity_summary(&current)
                .unwrap()
                .1
                .count_status
                .as_deref(),
            Some("MATCH (1 expected handoff gap)")
        );
        assert!(parse_machine_integrity_summary("Integrity: CLEAN\n").is_err());
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
        let rejected_outputs = format!(
            "{summary}\nMFB_CONVERTED_BYTES={{\"schema_version\":1,\"scope\":\"converted_this_run\",\"input_bytes\":null,\"output_bytes\":null}}\n"
        );
        let metrics = fast_img_session_size_metrics(&rejected_outputs);
        assert_eq!(metrics.source_bytes_actual, None);
        assert_eq!(metrics.output_bytes_actual, None);
        assert_eq!(metrics.files_converted, None);
        for invalid in [
            "1 KiB (-100 bytes)",
            "1 KiB (1x00 bytes)",
            "1 KiB (1,00 bytes)",
            "1 KiB (1000,000 bytes)",
            "1 KiB (100 bytes) trailing",
            "1 KiB (18446744073709551616 bytes)",
        ] {
            assert_eq!(parse_size_line_exact_bytes(invalid), None, "{invalid}");
        }
        assert_eq!(parse_size_line_exact_bytes("0 B (0 bytes)"), Some(0));
        assert_eq!(
            parse_size_line_exact_bytes("1 KiB (1024 bytes)"),
            Some(1024)
        );
    }
}

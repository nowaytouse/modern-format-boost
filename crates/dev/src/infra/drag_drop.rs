//! Drag-and-drop session helpers — parity with `drag_and_drop_processor.py`.

use crate::infra::fastmode_paths::fast_vid_output_dir_for_target;
use crate::infra::hardening::delegated_exit_code;
use crate::infra::log_paths::append_jsonl_audit_record;
use crate::infra::rich_panel::draw_separator;
use crate::infra::ui_tokens::{colors_enabled, pick_symbol};
use crate::media::scope::format_bytes;
use crate::media::scope::{
    PIPELINE_IMAGE, PIPELINE_VIDEO, classify_media_owner, format_audit_event,
    format_session_audit_routed,
};
use anyhow::{Context, Result, bail};
use foundation::io_utils::remove_verified_fast_img_output_from_root;
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
    pub source_root: Option<PathBuf>,
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
    /// The selected media inventory at discovery time. Sidecars and other files
    /// are outside this history grouping identity.
    #[must_use]
    pub fn history_source_set(
        &self,
    ) -> Option<foundation::batch::history_source_set::HistorySourceSet> {
        let root = self.source_root.as_deref()?;
        let count = self.img_count.checked_add(self.vid_count)?;
        let files = self
            .entries
            .iter()
            .filter(|entry| {
                entry.selected && matches!(entry.route, ScanRoute::Image | ScanRoute::Video)
            })
            .map(|entry| (entry.relative_path.as_path(), entry.source.content_blake3()));
        foundation::batch::history_source_set::HistorySourceSet::from_observed(root, count, files)
    }

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
pub const INTEGRITY_UNAVAILABLE_JSON_PREFIX: &str = "MFB_INTEGRITY_UNAVAILABLE_JSON ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrityUnavailableReason {
    Startup,
    DirectoryPair,
    UncachedVerification,
    Checkpoint,
    PhotosCustody,
    Report,
    Verification,
}

impl std::fmt::Display for IntegrityUnavailableReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Startup => "could not initialize the verifier or acquire the task lease",
            Self::DirectoryPair => "could not resolve or inspect the source and output directories",
            Self::UncachedVerification => "could not establish uncached verification",
            Self::Checkpoint => "could not read or bind the delivery checkpoint to this task",
            Self::PhotosCustody => "could not verify current Photos original-resource custody",
            Self::Report => "could not prepare or save the verification report",
            Self::Verification => "could not complete the integrity check",
        })
    }
}

impl std::error::Error for IntegrityUnavailableReason {}

/// A controlled unavailable result from Fast IMG's independent delivery verifier.
#[derive(Debug)]
pub struct FastImgIndependentVerificationUnavailable(pub IntegrityUnavailableReason);

impl std::fmt::Display for FastImgIndependentVerificationUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("independent Fast IMG delivery verification unavailable")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntegrityUnavailableMachine {
    Unavailable {
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<IntegrityUnavailableReason>,
    },
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retained_local_aae: Option<usize>,
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
            if path == target {
                scan.source_root = Some(resolved.clone());
            }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyFastImgMode {
    None,
    Delivery,
    PhotosCleanupRecovery(String),
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
    let live_progress = flags.auto_mode
        && matches!(
            &flags.fast_img,
            VerifyFastImgMode::Delivery | VerifyFastImgMode::PhotosCleanupRecovery(_)
        )
        && std::env::var_os("MFB_SESSION_ID").is_some_and(|id| !id.is_empty());
    if let Some(opt) = opt_dir {
        cmd.arg("--verify").arg(src_dir).arg(opt);
        cmd.arg("--mode").arg(processing_filter.verify_mode_label());
        match flags.fast_img {
            VerifyFastImgMode::Delivery => {
                cmd.arg("--fast-img-delivery");
            }
            VerifyFastImgMode::PhotosCleanupRecovery(checkpoint_digest) => {
                cmd.arg("--fast-img-delivery")
                    .arg("--fast-img-photos-cleanup-recovery")
                    .arg(checkpoint_digest);
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
    foundation::batch_control::checkpoint().context("admit independent verifier")?;
    let (status, raw_stdout, stderr) = if live_progress {
        let output = foundation::process_runner::ManagedProcess::spawn_captured_relaying(
            &mut cmd,
            relay_independent_verify_progress,
        )
        .and_then(foundation::process_runner::ManagedProcess::wait)
        .with_context(|| format!("run {}", verify_bin.display()))?;
        (output.status, output.stdout, output.stderr)
    } else {
        foundation::production_lock::inherit_active_lease(&mut cmd)
            .context("inherit the parent production-task lease for verification")?;
        let output = cmd
            .output()
            .with_context(|| format!("run {}", verify_bin.display()))?;
        (
            output.status,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };
    if !stderr.trim().is_empty() {
        eprint!("{stderr}");
    }
    let (stdout, integrity_summary) = if flags.auto_mode {
        let (display, summary) = parse_machine_integrity_summary(&raw_stdout).map_err(|error| {
            if status.success() && error.downcast_ref::<IntegrityUnavailableReason>().is_some() {
                anyhow::anyhow!(
                    "verify reported unavailable integrity with a successful exit status"
                )
            } else {
                error
            }
        })?;
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
    let exit_code = delegated_exit_code(status, "verify", "run_unified_verification");
    if let Some(summary) = &integrity_summary {
        let expected_success = !summary.has_warnings;
        if status.success() != expected_success {
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

/// Only relay the independent verifier's session events; keep diagnostics and authoritative results captured.
fn relay_independent_verify_progress(pid: u32, line: &str) -> bool {
    let Some((marker, payload)) = line.split_once('=') else {
        return false;
    };
    if !matches!(marker, "MFB_LOG_PHASE" | "MFB_GATE_ACTIVITY") {
        return false;
    }
    if valid_independent_verify_progress(pid, marker, payload) {
        let stdout = io::stdout();
        let mut stdout = stdout.lock();
        if let Err(error) = writeln!(stdout, "\n{line}").and_then(|()| stdout.flush()) {
            eprintln!("[WARN] Could not forward verifier progress: {error}");
        }
    }
    true
}

fn valid_independent_verify_progress(pid: u32, marker: &str, payload: &str) -> bool {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Files {
        processed: usize,
        total: usize,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Event {
        schema_version: u8,
        stage_id: String,
        gate: String,
        check: String,
        completed: usize,
        total: usize,
        state: String,
        file_progress: Option<Files>,
    }

    if marker == "MFB_LOG_PHASE" {
        return payload == r#"{"schema_version":1,"phase":"verification"}"#;
    }
    if payload.len() > 1_024 {
        return false;
    }
    let Ok(event) = serde_json::from_str::<Event>(payload) else {
        return false;
    };
    let stage_bound = event
        .stage_id
        .split_once('-')
        .is_some_and(|(owner, sequence)| {
            owner.parse::<u32>() == Ok(pid) && sequence.parse::<usize>().is_ok()
        });
    if event.schema_version != 1
        || !stage_bound
        || event.gate != "photos_custody"
        || event.total != 1
    {
        return false;
    }
    match (
        event.check.as_str(),
        event.state.as_str(),
        event.completed,
        event.file_progress,
    ) {
        ("inventory", "running" | "failed", 0, None) => true,
        ("original_bytes", state, completed, Some(files))
            if files.total > 0 && files.processed <= files.total =>
        {
            match state {
                "running" => completed == 0,
                "passed" => completed == 1 && files.processed == files.total,
                "failed" => completed == 1,
                _ => false,
            }
        }
        _ => false,
    }
}

fn parse_machine_integrity_summary(stdout: &str) -> Result<(String, IntegritySummaryMachine)> {
    let mut display_lines = Vec::new();
    let mut summary = None;
    let mut unavailable = None;
    for line in stdout.lines() {
        if let Some(json) = line.strip_prefix(INTEGRITY_UNAVAILABLE_JSON_PREFIX) {
            anyhow::ensure!(
                summary.is_none() && unavailable.is_none(),
                "verify emitted more than one machine integrity result"
            );
            anyhow::ensure!(
                json.len() <= 256,
                "invalid verify unavailable integrity result"
            );
            unavailable = Some(
                serde_json::from_str::<IntegrityUnavailableMachine>(json)
                    .map_err(|_| anyhow::anyhow!("invalid verify unavailable integrity result"))?,
            );
        } else if let Some(json) = line.strip_prefix(INTEGRITY_SUMMARY_JSON_PREFIX) {
            anyhow::ensure!(
                summary.is_none() && unavailable.is_none(),
                "verify emitted more than one machine integrity result"
            );
            summary =
                Some(serde_json::from_str(json).context("parse verify machine integrity summary")?);
        } else {
            display_lines.push(line);
        }
    }
    if let Some(IntegrityUnavailableMachine::Unavailable { reason }) = unavailable {
        let reason = reason.unwrap_or(IntegrityUnavailableReason::Verification);
        return Err(anyhow::Error::new(reason));
    }
    let summary: IntegritySummaryMachine =
        summary.context("verify omitted its machine integrity summary")?;
    if let Some(retained) = summary.retained_local_aae {
        anyhow::ensure!(
            retained > 0
                && retained <= summary.optimized_count
                && !summary.has_warnings
                && summary.issue_count == 0,
            "verify emitted inconsistent local companion custody"
        );
    }
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
    let proc = {
        let _transaction = foundation::batch_control::begin_transaction()?;
        Command::new(rsync)
            .args(&cmd[1..])
            .output()
            .context("run rsync for non-media sync")?
    };
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
    let ts_proc = run_timestamp_restore(img_bin, target, output)?;
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

fn run_timestamp_restore(
    img_bin: &Path,
    target: &Path,
    output: &Path,
) -> Result<std::process::Output> {
    let _transaction = foundation::batch_control::begin_transaction()?;
    let mut command = Command::new(img_bin);
    command.arg("restore-timestamps").arg(target).arg(output);
    foundation::production_lock::inherit_active_lease(&mut command)
        .context("inherit the parent production-task lease for timestamp restore")?;
    command.output().context("run img restore-timestamps")
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
    let mut entries = WalkDir::new(output_dir)
        .follow_root_links(false)
        .into_iter();
    while let Some(entry) = entries.next() {
        let entry = entry?;
        let path = entry.path();
        if entry.depth() > 0
            && entry.file_type().is_dir()
            && foundation::safety::check_apple_photos_library(path).is_err()
        {
            entries.skip_current_dir();
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let true_format = foundation::image::format_detect::detect_regular_file_format(path)
            .with_context(|| format!("fast-img output probe failed for {}", path.display()))?;
        if true_format == foundation::image::format_detect::FormatKind::Jxl {
            count += 1;
            total_size += fs::symlink_metadata(path)
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
fn fast_img_file_failure_receipt(
    text: &str,
    stats: &crate::infra::process_stream::ProcessorStats,
) -> Option<Value> {
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
        || count("failed_sources_retained") != Some(stats.failed)
        || count("failed_sources_verified") != Some(stats.failed)
        || !report.get("finalization_error")?.is_null()
        || !report.get("retention_unknown_reason")?.is_null()
        || !report.get("failed_source_verification_error")?.is_null()
        || count("source_retained").is_none_or(|retained| retained < stats.failed)
    {
        return None;
    }
    Some(report)
}

pub fn fast_img_partial_cleanup_verified(
    text: &str,
    stats: &crate::infra::process_stream::ProcessorStats,
) -> Option<usize> {
    let report = fast_img_file_failure_receipt(text, stats)?;
    let succeeded = u64::try_from(stats.succeeded).ok()?;
    (report["primary_cleanup_complete"].as_bool() == Some(true)
        && report["retained_only_complete"].as_bool() != Some(true)
        && report["photos_verified"].as_u64() == Some(succeeded)
        && report["encoded"]
            .as_u64()
            .is_some_and(|encoded| encoded <= succeeded))
    .then_some(stats.failed)
}

pub fn fast_img_retained_only_verified(
    text: &str,
    stats: &crate::infra::process_stream::ProcessorStats,
    source: &Path,
    output: &Path,
) -> bool {
    let Some(report) = fast_img_file_failure_receipt(text, stats) else {
        return false;
    };
    stats.succeeded == 0
        && report["encoded"].as_u64() == Some(0)
        && report["photos_verified"].as_u64() == Some(0)
        && report["retained_only_complete"].as_bool() == Some(true)
        && report["primary_cleanup_complete"].as_bool() == Some(false)
        && report["photos_delivery_deferred"].as_bool() == Some(false)
        && stats
            .failed
            .checked_add(stats.skipped)
            .and_then(|count| u64::try_from(count).ok())
            == report["source_retained"].as_u64()
        && report["source_directory"].as_str().map(Path::new) == Some(source)
        && report["output_directory"].as_str().map(Path::new) == Some(output)
}

#[derive(Debug)]
pub struct PhotosQuarantineBlocked {
    pub count: usize,
}

impl std::fmt::Display for PhotosQuarantineBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Final Photos verification blocked by quarantine on {} item(s); sources and working copy retained",
            self.count
        )
    }
}

impl std::error::Error for PhotosQuarantineBlocked {}

impl PhotosQuarantineBlocked {
    pub fn from_error(error: &anyhow::Error) -> Option<&Self> {
        (error.chain().count() == 1)
            .then(|| error.downcast_ref::<Self>())
            .flatten()
    }

    pub fn from_receipt(
        report: &Value,
        stats: &crate::infra::process_stream::ProcessorStats,
        source: &Path,
        output: &Path,
    ) -> Option<Self> {
        let count = usize::try_from(report["photos_quarantine_blocked"].as_u64()?).ok()?;
        if let Some(locations) = report
            .get("photos_quarantine_locations")
            .filter(|value| !value.is_null())
        {
            let locations: foundation::fast_img::PhotosQuarantineLocations =
                serde_json::from_value(locations.clone()).ok()?;
            if !locations.agrees_with(count) {
                return None;
            }
        }
        let unprocessed = stats.unprocessed?;
        let retained = u64::try_from(stats.skipped.checked_add(unprocessed)?).ok()?;
        let pending = u64::try_from(unprocessed).ok()?;
        (stats.counts_complete()
            && stats.exit_code == 2
            && stats.succeeded == 0
            && stats.failed == 0
            && count > 0
            && count <= unprocessed
            && report["encoded"].as_u64() == Some(pending)
            && report["photos_verified"].as_u64() == Some(pending)
            && report["source_retained"].as_u64() == Some(retained)
            && report["failed_sources_retained"].as_u64() == Some(0)
            && report["failed_sources_verified"].as_u64() == Some(0)
            && report.get("failed_source_verification_error")?.is_null()
            && report.get("retention_unknown_reason")?.is_null()
            && report["finalization_error"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty())
            && report["photos_delivery_deferred"].as_bool() == Some(false)
            && report["primary_verification_complete"].as_bool() == Some(true)
            && report["primary_cleanup_complete"].as_bool() == Some(false)
            && report["retained_only_complete"].as_bool() == Some(false)
            && report["source_directory"].as_str().map(Path::new) == Some(source)
            && report["output_directory"].as_str().map(Path::new) == Some(output))
        .then_some(Self { count })
    }
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

fn completed_photos_cleanup_marker(
    output_dir: &Path,
) -> Result<foundation::pipeline::verification::WorkingCopyMarker> {
    use foundation::pipeline::verification::read_marker;
    let marker = read_marker(output_dir).context("read fast-img cleanup ownership checkpoint")?;
    anyhow::ensure!(
        marker.working_copy == output_dir
            && marker.is_terminal()
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
    Ok(marker)
}

/// Bind a cleanup-only retry to one ordinary completed Photos task and its exact absent source.
pub fn exact_fast_img_photos_cleanup_recovery(
    source: &Path,
    output: &Path,
    strategy: &str,
) -> Result<foundation::pipeline::verification::WorkingCopyMarker> {
    use foundation::pipeline::verification::canonical_working_copy_path;
    anyhow::ensure!(
        matches!(fs::symlink_metadata(source), Err(error) if error.kind() == io::ErrorKind::NotFound),
        "Fast IMG cleanup recovery requires the saved source directory to be absent"
    );
    for path in [source, output] {
        anyhow::ensure!(
            path.is_absolute()
                && path.file_name().is_some()
                && path.as_os_str() == canonical_working_copy_path(path)?.as_os_str(),
            "Fast IMG cleanup recovery requires exact absolute saved paths without links"
        );
        foundation::safety::check_dangerous_directory(path).map_err(anyhow::Error::msg)?;
        foundation::safety::check_apple_photos_library(path).map_err(anyhow::Error::msg)?;
    }
    anyhow::ensure!(
        !source.starts_with(output) && !output.starts_with(source),
        "Fast IMG cleanup recovery source and work copy overlap"
    );
    let marker = completed_photos_cleanup_marker(output)?;
    anyhow::ensure!(
        marker.src_dir.as_os_str() == source.as_os_str()
            && marker.strategy == strategy
            && marker.failed_retry_manifest_blake3.is_none()
            && marker.source_disposition_is_complete()
            && marker.skipped_sources.is_empty()
            && marker.failed_sources.is_empty()
            && marker.tier2_failed_sources.is_empty()
            && marker.photos_library_path.is_some()
            && (!marker.photos_imported_assets.is_empty()
                || !marker.tier2_imported_assets.is_empty()),
        "Fast IMG cleanup recovery lacks exact saved Photos delivery and complete source disposition"
    );
    marker
        .validate_checkpoint_path_contract(output)
        .map_err(anyhow::Error::msg)?;
    marker
        .validate_source_disposition_disjoint()
        .map_err(anyhow::Error::msg)?;
    Ok(marker)
}

pub fn fast_img_cleanup_checkpoint_digest(
    marker: &foundation::pipeline::verification::WorkingCopyMarker,
) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    serde_json::to_writer(&mut hasher, marker)?;
    Ok(hasher.finalize().to_hex().to_string())
}

fn fast_img_marker_cleanup_targets(
    output_dir: &Path,
) -> Result<(
    foundation::pipeline::verification::WorkingCopyMarker,
    Vec<(PathBuf, String)>,
)> {
    let marker = completed_photos_cleanup_marker(output_dir)?;
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
    Ok((marker, targets))
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub residual_inventory: Option<OutputResidualInventory>,
}

impl OutputCleanupReceipt {
    pub fn not_requested(output_path: &Path) -> Self {
        Self::without_operation(output_path, OutputCleanupOutcome::NotRequested)
    }

    pub fn verification_blocked(output_path: &Path) -> Self {
        Self::without_operation(output_path, OutputCleanupOutcome::VerificationBlocked)
    }

    pub fn retained_local_companions(output_path: &Path) -> Self {
        let mut receipt = Self::without_operation(output_path, OutputCleanupOutcome::Residual);
        receipt.removed_files = Some(0);
        receipt.already_absent_files = Some(0);
        receipt.pruned_directories = Some(0);
        if let Ok(metadata) = fs::symlink_metadata(output_path)
            && metadata.is_dir()
        {
            receipt.residual_inventory = Some(output_residual_inventory(output_path, &metadata));
        }
        receipt
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
            residual_inventory: None,
        }
    }
}

const RESIDUAL_SAMPLE_LIMIT: usize = 12;
const RESIDUAL_SAMPLE_PATH_BYTES: usize = 512;
const RESIDUAL_ENTRY_LIMIT: usize = 16_384;
const RESIDUAL_DEPTH_LIMIT: usize = 32;
const CLEANUP_INVENTORY_HASH_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CleanupInventoryCategory {
    RegenerableOwned,
    RecoveryProtected,
    Unknown,
}

#[derive(Debug, Serialize)]
struct CleanupInventoryGroup {
    category: CleanupInventoryCategory,
    files: u64,
    bytes: u64,
    sample: Vec<String>,
    sample_truncated: bool,
}

impl CleanupInventoryGroup {
    const fn new(category: CleanupInventoryCategory) -> Self {
        Self {
            category,
            files: 0,
            bytes: 0,
            sample: Vec::new(),
            sample_truncated: false,
        }
    }

    fn record(&mut self, relative: &Path, size: u64) -> Result<(), &'static str> {
        self.files += 1; // The shared walker has already bounded the total.
        self.bytes = self.bytes.checked_add(size).ok_or("count_overflow")?;
        if let Some(name) = relative
            .to_str()
            .filter(|name| name.len() <= RESIDUAL_SAMPLE_PATH_BYTES)
            && self.sample.len() < RESIDUAL_SAMPLE_LIMIT
        {
            self.sample.push(name.to_owned());
        } else {
            self.sample_truncated = true;
        }
        Ok(())
    }
}

type CleanupInventoryGroups = [CleanupInventoryGroup; 3];

const fn cleanup_inventory_groups() -> CleanupInventoryGroups {
    [
        CleanupInventoryGroup::new(CleanupInventoryCategory::RegenerableOwned),
        CleanupInventoryGroup::new(CleanupInventoryCategory::RecoveryProtected),
        CleanupInventoryGroup::new(CleanupInventoryCategory::Unknown),
    ]
}

struct CleanupInventoryClassification<'a> {
    marker: &'a foundation::pipeline::verification::WorkingCopyMarker,
    by_output:
        HashMap<&'a str, Option<(&'a str, &'a foundation::pipeline::verification::Blake3Entry)>>,
    groups: CleanupInventoryGroups,
    hash_bytes_remaining: u64,
    allow_rebuild: bool,
}

impl<'a> CleanupInventoryClassification<'a> {
    fn new(marker: &'a foundation::pipeline::verification::WorkingCopyMarker) -> Self {
        use foundation::pipeline::verification::FastImgStageName;
        // A local delivery and anything that may have reached Photos can carry
        // recovery obligations beyond the existence of a matching source copy.
        let allow_rebuild = matches!(
            marker.stage,
            FastImgStageName::OutputPrepared
                | FastImgStageName::TranscodeComplete
                | FastImgStageName::Gate1Passed
                | FastImgStageName::Gate1Failed
        ) && marker.processing_contract.is_some()
            && marker.failed_retry_manifest_blake3.is_none()
            && marker.photos_submission_history().is_some_and(|history| {
                history.complete_from_creation && history.intents.is_empty()
            })
            && marker.photos_imported_assets.is_empty()
            && marker.tier2_imported_assets.is_empty()
            && !marker.tier2_in_progress;
        let mut by_output = HashMap::new();
        for (source, proof) in &marker.blake3_log {
            if let Some(output) = proof.out_rel.as_deref() {
                by_output
                    .entry(output)
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some((source.as_str(), proof)));
            }
        }
        Self {
            marker,
            by_output,
            groups: cleanup_inventory_groups(),
            hash_bytes_remaining: CLEANUP_INVENTORY_HASH_BYTES,
            allow_rebuild,
        }
    }

    fn take_hash_budget(&mut self, size: u64) -> bool {
        let Some(remaining) = self.hash_bytes_remaining.checked_sub(size) else {
            return false;
        };
        self.hash_bytes_remaining = remaining;
        true
    }
}

/// A bounded current observation of one checkpoint's exact working-copy path.
#[derive(Debug, Serialize)]
pub struct FastImgCleanupInventory {
    schema_version: u8,
    kind: &'static str,
    output_path: PathBuf,
    state: &'static str,
    reason: Option<&'static str>,
    categories: Vec<CleanupInventoryGroup>,
    directories: Option<u64>,
    symlinks: Option<u64>,
    other_entries: Option<u64>,
}

/// Read-only metadata measured after a cleanup leaves its output directory behind.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct OutputResidualInventory {
    state: &'static str,
    regular_files: Option<u64>,
    regular_bytes: Option<u64>,
    directories: Option<u64>,
    symlinks: Option<u64>,
    other_entries: Option<u64>,
    error: Option<&'static str>,
    sample: Vec<OutputResidualEntry>,
    sample_truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct OutputResidualEntry {
    relative_path: String,
    kind: ResidualEntryKind,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ResidualEntryKind {
    RegularFile,
    Directory,
    Symlink,
    Other,
}

#[derive(Debug, Default)]
struct ResidualInventoryCounts {
    entries: usize,
    regular_files: u64,
    regular_bytes: u64,
    directories: u64,
    symlinks: u64,
    other_entries: u64,
    sample: Vec<OutputResidualEntry>,
    sample_truncated: bool,
}

impl ResidualInventoryCounts {
    fn record(
        &mut self,
        path: &Path,
        kind: ResidualEntryKind,
        size: u64,
    ) -> Result<(), &'static str> {
        self.entries += 1;
        if self.entries > RESIDUAL_ENTRY_LIMIT {
            return Err("entry_limit");
        }
        let count = match kind {
            ResidualEntryKind::RegularFile => {
                self.regular_bytes = self
                    .regular_bytes
                    .checked_add(size)
                    .ok_or("count_overflow")?;
                &mut self.regular_files
            }
            ResidualEntryKind::Directory => &mut self.directories,
            ResidualEntryKind::Symlink => &mut self.symlinks,
            ResidualEntryKind::Other => &mut self.other_entries,
        };
        *count += 1; // Every category is bounded by RESIDUAL_ENTRY_LIMIT.
        if let Some(relative_path) = path
            .to_str()
            .filter(|value| value.len() <= RESIDUAL_SAMPLE_PATH_BYTES)
            && self.sample.len() < RESIDUAL_SAMPLE_LIMIT
        {
            self.sample.push(OutputResidualEntry {
                relative_path: relative_path.to_owned(),
                kind,
            });
        } else {
            self.sample_truncated = true;
        }
        Ok(())
    }
}

fn output_residual_inventory(root: &Path, root_metadata: &fs::Metadata) -> OutputResidualInventory {
    match observe_output_residuals(root, root_metadata) {
        Ok(mut counts) => {
            counts
                .sample
                .sort_unstable_by(|a, b| a.relative_path.cmp(&b.relative_path));
            OutputResidualInventory {
                state: "observed",
                regular_files: Some(counts.regular_files),
                regular_bytes: Some(counts.regular_bytes),
                directories: Some(counts.directories),
                symlinks: Some(counts.symlinks),
                other_entries: Some(counts.other_entries),
                error: None,
                sample: counts.sample,
                sample_truncated: counts.sample_truncated,
            }
        }
        Err(error) => OutputResidualInventory {
            state: "unknown",
            regular_files: None,
            regular_bytes: None,
            directories: None,
            symlinks: None,
            other_entries: None,
            error: Some(error),
            sample: Vec::new(),
            sample_truncated: false,
        },
    }
}

fn observe_output_residuals(
    root: &Path,
    root_metadata: &fs::Metadata,
) -> Result<ResidualInventoryCounts, &'static str> {
    observe_output_residuals_with(root, root_metadata, None)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn observe_output_residuals_with(
    root: &Path,
    root_metadata: &fs::Metadata,
    mut classification: Option<&mut CleanupInventoryClassification<'_>>,
) -> Result<ResidualInventoryCounts, &'static str> {
    use std::ffi::{CStr, CString, OsStr};
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    use std::path::Component;

    const DIR_FLAGS: libc::c_int =
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

    fn open_directory(parent: libc::c_int, name: &CStr) -> Result<fs::File, &'static str> {
        // Match the repository Photos guard's lexical suffix check without
        // resolving paths outside the already opened directory descriptor.
        if name
            .to_str()
            .ok()
            .map(str::to_ascii_lowercase)
            .is_some_and(|name| name.ends_with(".photoslibrary") || name.ends_with(".photolibrary"))
        {
            return Err("inspection_failed");
        }
        // SAFETY: the descriptor belongs to the live parent and name is NUL-terminated.
        let fd = unsafe { libc::openat(parent, name.as_ptr(), DIR_FLAGS) };
        if fd < 0 {
            return Err("inspection_failed");
        }
        // SAFETY: successful openat returns a new owned descriptor.
        Ok(unsafe { fs::File::from_raw_fd(fd) })
    }

    fn open_root(path: &Path) -> Result<fs::File, &'static str> {
        let mut components = path.components();
        if components.next() != Some(Component::RootDir) {
            return Err("inspection_failed");
        }
        let mut parent = open_directory(libc::AT_FDCWD, c"/")?;
        for component in components {
            let Component::Normal(name) = component else {
                return Err("inspection_failed");
            };
            let name = CString::new(name.as_bytes()).map_err(|_| "inspection_failed")?;
            parent = open_directory(parent.as_raw_fd(), &name)?;
        }
        Ok(parent)
    }

    fn unchanged(before: &fs::Metadata, after: &fs::Metadata) -> bool {
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.mtime() == after.mtime()
            && before.mtime_nsec() == after.mtime_nsec()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }

    fn same_entry(before: &libc::stat, after: &libc::stat) -> bool {
        before.st_dev == after.st_dev
            && before.st_ino == after.st_ino
            && before.st_mode == after.st_mode
            && before.st_size == after.st_size
            && before.st_mtime == after.st_mtime
            && before.st_mtime_nsec == after.st_mtime_nsec
            && before.st_ctime == after.st_ctime
            && before.st_ctime_nsec == after.st_ctime_nsec
    }

    struct DirectoryStream(*mut libc::DIR);

    impl Drop for DirectoryStream {
        fn drop(&mut self) {
            // SAFETY: a successful fdopendir transferred descriptor ownership to this stream.
            unsafe { libc::closedir(self.0) };
        }
    }

    fn entry_metadata(parent: &fs::File, name: &CStr) -> Result<libc::stat, &'static str> {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstatat writes the stat buffer on success and never follows the final link.
        if unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err("inspection_failed");
        }
        Ok(unsafe { metadata.assume_init() })
    }

    fn exact_file_bytes(
        parent: &fs::File,
        name: &CStr,
        expected_entry: &libc::stat,
        expected_hash: &str,
    ) -> bool {
        use std::io::Read;

        let verify = || -> io::Result<bool> {
            // SAFETY: name is NUL-terminated and the parent descriptor is live.
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: successful openat returns a new owned file descriptor.
            let file = unsafe { fs::File::from_raw_fd(fd) };
            let before = file.metadata()?;
            let mut opened = std::mem::MaybeUninit::<libc::stat>::uninit();
            // SAFETY: the file descriptor is live and fstat fills the buffer on success.
            if unsafe { libc::fstat(file.as_raw_fd(), opened.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            let opened = unsafe { opened.assume_init() };
            let observed_size = u64::try_from(expected_entry.st_size)
                .map_err(|_| io::Error::other("invalid inventory file size"))?;
            if !before.is_file()
                || before.nlink() != 1
                || !same_entry(expected_entry, &opened)
                || !same_entry(
                    expected_entry,
                    &entry_metadata(parent, name).map_err(io::Error::other)?,
                )
            {
                return Ok(false);
            }
            let mut bounded = file.take(observed_size.saturating_add(1));
            let mut hash = blake3::Hasher::new();
            if io::copy(&mut bounded, &mut hash)? != observed_size {
                return Ok(false);
            }
            let after = bounded.get_ref().metadata()?;
            Ok(unchanged(&before, &after)
                && same_entry(
                    expected_entry,
                    &entry_metadata(parent, name).map_err(io::Error::other)?,
                )
                && hash.finalize().to_hex().as_str() == expected_hash)
        };
        verify().unwrap_or(false)
    }

    fn exact_retained_source(
        root: &Path,
        source: &str,
        expected_size: u64,
        expected_hash: &str,
    ) -> bool {
        let Some((name, parents)) = Path::new(source)
            .components()
            .next_back()
            .zip(Path::new(source).parent())
        else {
            return false;
        };
        let Component::Normal(name) = name else {
            return false;
        };
        let Ok(name) = CString::new(name.as_bytes()) else {
            return false;
        };
        let Ok(parent) = open_root(&root.join(parents)) else {
            return false;
        };
        let Ok(metadata) = entry_metadata(&parent, &name) else {
            return false;
        };
        metadata.st_mode & libc::S_IFMT == libc::S_IFREG
            && u64::try_from(metadata.st_size).ok() == Some(expected_size)
            && exact_file_bytes(&parent, &name, &metadata, expected_hash)
    }

    fn classify_file(
        classification: &mut CleanupInventoryClassification<'_>,
        directory: &fs::File,
        name: &CStr,
        path: &Path,
        metadata: &libc::stat,
        size: u64,
    ) -> Result<(), &'static str> {
        let entry = path
            .to_str()
            .and_then(|relative| classification.by_output.get(relative))
            .copied()
            .flatten();
        let mut group = 2;
        if let Some((source, proof)) = entry
            && classification.take_hash_budget(size)
            && exact_file_bytes(directory, name, metadata, &proof.out)
        {
            group = 1;
            if classification.allow_rebuild
                && proof.src == proof.out
                && classification.take_hash_budget(size)
                && exact_retained_source(&classification.marker.src_dir, source, size, &proof.src)
            {
                group = 0;
            }
        }
        classification.groups[group].record(path, size)
    }

    fn walk(
        directory: &fs::File,
        relative: &Path,
        depth: usize,
        counts: &mut ResidualInventoryCounts,
        classification: &mut Option<&mut CleanupInventoryClassification<'_>>,
    ) -> Result<(), &'static str> {
        let before = directory.metadata().map_err(|_| "inspection_failed")?;
        let listing = directory.try_clone().map_err(|_| "inspection_failed")?;
        // SAFETY: listing is an owned directory descriptor and remains live on failure.
        let stream = unsafe { libc::fdopendir(listing.as_raw_fd()) };
        if stream.is_null() {
            return Err("inspection_failed");
        }
        let _ = listing.into_raw_fd();
        let stream = DirectoryStream(stream);
        let mut entries_to_recheck = Vec::new();
        loop {
            #[cfg(target_os = "macos")]
            let errno = unsafe { libc::__error() };
            #[cfg(target_os = "linux")]
            let errno = unsafe { libc::__errno_location() };
            // SAFETY: errno is thread-local and readdir uses the exclusively owned live stream.
            let entry = unsafe {
                *errno = 0;
                libc::readdir(stream.0)
            };
            if entry.is_null() {
                if unsafe { *errno } != 0 {
                    return Err("inspection_failed");
                }
                break;
            }
            // SAFETY: readdir supplies a NUL-terminated name valid until the next call on this stream.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            let metadata = entry_metadata(directory, name)?;
            let kind = match metadata.st_mode & libc::S_IFMT {
                libc::S_IFREG => ResidualEntryKind::RegularFile,
                libc::S_IFDIR => ResidualEntryKind::Directory,
                libc::S_IFLNK => ResidualEntryKind::Symlink,
                _ => ResidualEntryKind::Other,
            };
            let path = relative.join(OsStr::from_bytes(name.to_bytes()));
            let size = if kind == ResidualEntryKind::RegularFile {
                u64::try_from(metadata.st_size).map_err(|_| "count_overflow")?
            } else {
                0
            };
            counts.record(&path, kind, size)?;
            if kind == ResidualEntryKind::RegularFile {
                if let Some(classification) = classification.as_deref_mut() {
                    classify_file(classification, directory, name, &path, &metadata, size)?;
                }
                entries_to_recheck.push((name.to_owned(), metadata));
            } else if kind == ResidualEntryKind::Directory {
                if depth >= RESIDUAL_DEPTH_LIMIT {
                    return Err("depth_limit");
                }
                let child = open_directory(directory.as_raw_fd(), name)?;
                let opened = child.metadata().map_err(|_| "inspection_failed")?;
                if opened.dev() != before.dev() {
                    return Err("inspection_failed");
                }
                if !same_entry(&metadata, &entry_metadata(&child, c".")?) {
                    return Err("directory_changed");
                }
                walk(&child, &path, depth + 1, counts, classification)?;
                entries_to_recheck.push((name.to_owned(), metadata));
            }
        }
        for (name, before) in entries_to_recheck {
            let after = entry_metadata(directory, &name)?;
            if !same_entry(&before, &after) {
                return Err("directory_changed");
            }
        }
        let after = directory.metadata().map_err(|_| "inspection_failed")?;
        if !unchanged(&before, &after) {
            return Err("directory_changed");
        }
        Ok(())
    }

    let directory = open_root(root)?;
    let before = directory.metadata().map_err(|_| "inspection_failed")?;
    if !unchanged(root_metadata, &before) {
        return Err("directory_changed");
    }
    let mut counts = ResidualInventoryCounts::default();
    walk(
        &directory,
        Path::new(""),
        0,
        &mut counts,
        &mut classification,
    )?;
    let after = open_root(root)?
        .metadata()
        .map_err(|_| "inspection_failed")?;
    if !unchanged(&before, &after) {
        return Err("directory_changed");
    }
    Ok(counts)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn observe_output_residuals_with(
    _root: &Path,
    _root_metadata: &fs::Metadata,
    classification: Option<&mut CleanupInventoryClassification<'_>>,
) -> Result<ResidualInventoryCounts, &'static str> {
    Err(if classification.is_some() {
        "unsupported_platform"
    } else {
        "inspection_failed"
    })
}

/// Observe one exact saved working copy without writing or authorizing cleanup.
pub fn inspect_fast_img_cleanup_inventory(output: &Path) -> FastImgCleanupInventory {
    use std::path::Component;

    let mut report = FastImgCleanupInventory {
        schema_version: 1,
        kind: "fast_img_cleanup_inventory",
        output_path: output.to_path_buf(),
        state: "unknown",
        reason: None,
        categories: Vec::new(),
        directories: None,
        symlinks: None,
        other_entries: None,
    };
    let observation = (|| -> Result<Option<_>, &'static str> {
        if !output.is_absolute()
            || output.file_name().is_none()
            || output
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err("unsafe_path");
        }
        foundation::safety::check_dangerous_directory(output).map_err(|_| "unsafe_path")?;
        foundation::safety::check_apple_photos_library(output).map_err(|_| "unsafe_path")?;
        if !cfg!(any(target_os = "macos", target_os = "linux")) {
            return Err("unsupported_platform");
        }
        let metadata = match fs::symlink_metadata(output) {
            Ok(metadata) if metadata.is_dir() => metadata,
            Ok(_) => return Err("unsafe_path"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                for ancestor in output.ancestors().skip(1) {
                    match fs::symlink_metadata(ancestor) {
                        Ok(metadata) if metadata.is_dir() => {
                            if ancestor
                                .canonicalize()
                                .map_err(|_| "inspection_failed")?
                                .as_os_str()
                                != ancestor.as_os_str()
                            {
                                return Err("unsafe_path");
                            }
                            return Ok(None);
                        }
                        Ok(_) => return Err("unsafe_path"),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(_) => return Err("inspection_failed"),
                    }
                }
                return Err("inspection_failed");
            }
            Err(_) => return Err("inspection_failed"),
        };
        if output
            .canonicalize()
            .map_err(|_| "inspection_failed")?
            .as_os_str()
            != output.as_os_str()
        {
            return Err("unsafe_path");
        }
        let marker = foundation::fast_img_store::read_marker(output)
            .map_err(|_| "checkpoint_unavailable")?;
        if marker.working_copy.as_os_str() != output.as_os_str()
            || marker.src_dir.starts_with(output)
            || output.starts_with(&marker.src_dir)
            || foundation::safety::check_apple_photos_library(&marker.src_dir).is_err()
        {
            return Err("unsafe_path");
        }
        let mut classification = CleanupInventoryClassification::new(&marker);
        let counts = observe_output_residuals_with(output, &metadata, Some(&mut classification))?;
        let current = foundation::fast_img_store::read_marker(output)
            .map_err(|_| "checkpoint_unavailable")?;
        if current != marker {
            return Err("directory_changed");
        }
        Ok(Some((counts, classification.groups)))
    })();
    match observation {
        Ok(Some((counts, mut groups))) => {
            for group in &mut groups {
                group.sample.sort_unstable();
            }
            report.state = "observed";
            report.categories = Vec::from(groups);
            report.directories = Some(counts.directories);
            report.symlinks = Some(counts.symlinks);
            report.other_entries = Some(counts.other_entries);
        }
        Ok(None) => {
            report.state = "absent";
            report.categories = Vec::from(cleanup_inventory_groups());
            report.directories = Some(0);
            report.symlinks = Some(0);
            report.other_entries = Some(0);
        }
        Err(reason) => report.reason = Some(reason),
    }
    report
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
    // An empty branch outside the recorded deliveries can belong to another writer.
    let candidates = candidate_dirs.iter().cloned().collect::<Vec<_>>();
    foundation::io_utils::prune_delivered_directories_within(output_root, &candidates).with_context(
        || {
            format!(
                "fast-img refused unsafe empty-directory cleanup under {}",
                output_root.display()
            )
        },
    )
}

#[derive(Debug, Serialize)]
pub struct HistoricalOutputCleanup {
    schema_version: u8,
    current_output: PathBuf,
    entries: Vec<HistoricalOutputObservation>,
    omitted: usize,
    enumeration_failed: bool,
    source_absent: bool,
}

#[derive(Debug, Serialize)]
struct HistoricalOutputObservation {
    output_path: PathBuf,
    outcome: &'static str,
    regular_files: Option<u64>,
    regular_bytes: Option<u64>,
}

/// Inspect recorded sibling paths without taking ownership of any current directory.
pub fn observe_historical_fast_img_outputs(
    source: &Path,
    strategy: &str,
    current_output: &Path,
) -> HistoricalOutputCleanup {
    use foundation::pipeline::verification::{FastImgStageName, matching_working_copy_markers};
    let mut report = HistoricalOutputCleanup {
        schema_version: 1,
        current_output: current_output.to_path_buf(),
        entries: Vec::new(),
        omitted: 0,
        enumeration_failed: false,
        source_absent: false,
    };
    let source_metadata = fs::symlink_metadata(source);
    if matches!(&source_metadata, Err(error) if error.kind() == io::ErrorKind::NotFound) {
        report.source_absent = true;
        return report;
    }
    let markers = match source_metadata.and_then(|metadata| {
        if !metadata.is_dir() || source.canonicalize()? != source {
            return Err(io::Error::other(
                "historical cleanup source identity changed",
            ));
        }
        matching_working_copy_markers(source, strategy)
    }) {
        Ok(markers) => markers,
        Err(error) => {
            eprintln!("[WARN] Historical working-copy inventory unavailable: {error:#}");
            report.enumeration_failed = true;
            return report;
        }
    };
    let current_identity = current_output
        .parent()
        .zip(current_output.file_name())
        .and_then(|(parent, name)| parent.canonicalize().ok().map(|parent| parent.join(name)));
    let Some(current_identity) = current_identity else {
        report.enumeration_failed = true;
        return report;
    };
    for marker in markers {
        let output = &marker.working_copy;
        if output == &current_identity {
            continue;
        }
        // shortcut: cap per-run observations until History can page through older records.
        if report.entries.len() == 8 || output.to_str().is_none_or(|path| path.len() > 1024) {
            report.omitted += 1;
            continue;
        }
        let inspect = || -> Result<(&'static str, Option<OutputResidualInventory>)> {
            anyhow::ensure!(
                output != source && output.parent() == source.parent(),
                "historical output is outside the selected source's sibling scope"
            );
            let metadata = match fs::symlink_metadata(output) {
                Ok(metadata) if metadata.is_dir() => metadata,
                Ok(_) => bail!("historical output is not a real directory"),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(("absent", None));
                }
                Err(error) => return Err(error.into()),
            };
            foundation::safety::check_dangerous_directory(output).map_err(anyhow::Error::msg)?;
            foundation::safety::check_apple_photos_library(output).map_err(anyhow::Error::msg)?;
            let inventory = output_residual_inventory(output, &metadata);
            let outcome =
                if marker.stage != FastImgStageName::CleanupComplete || marker.tier2_in_progress {
                    "retained_recovery"
                } else if marker.photos_imported_assets.is_empty()
                    && !marker.gate2_checks.count.0
                    && marker.gate1_checks.count.0
                    && marker.gate1_checks.blake3.0
                    && marker.gate1_checks.metadata.0
                    && marker.gate1_checks.size.0
                    && marker.gate1_checks.orient.0
                    && marker.gate1_checks.decode.0
                {
                    "retained_local_delivery"
                } else {
                    match (
                        inventory.regular_files,
                        inventory.directories,
                        inventory.symlinks,
                        inventory.other_entries,
                    ) {
                        (Some(0), Some(0), Some(0), Some(0)) => "retained_empty",
                        (Some(_), Some(_), Some(_), Some(_)) => "retained_nonempty",
                        _ => "blocked",
                    }
                };
            Ok((outcome, Some(inventory)))
        };
        let (outcome, inventory) = match inspect() {
            Ok(observation) => observation,
            Err(error) => {
                eprintln!(
                    "[WARN] Historical working-copy inspection failed: {}: {error:#}",
                    output.display()
                );
                ("blocked", None)
            }
        };
        report.entries.push(HistoricalOutputObservation {
            output_path: output.clone(),
            outcome,
            regular_files: inventory.as_ref().and_then(|value| value.regular_files),
            regular_bytes: inventory.and_then(|value| value.regular_bytes),
        });
    }
    report
}

/// Remove only unchanged, marker-recorded outputs after verified local Photos delivery.
pub fn delete_fast_img_shortest_path_output_dir(output_dir: &Path) -> OutputCleanupReceipt {
    delete_fast_img_photos_outputs(output_dir, None)
}

/// Continue cleanup only while the checkpoint still matches the recovered delivery.
pub fn recover_fast_img_shortest_path_output_dir(
    expected: &foundation::pipeline::verification::WorkingCopyMarker,
) -> OutputCleanupReceipt {
    let output = &expected.working_copy;
    let mut receipt = delete_fast_img_photos_outputs(output, Some(expected));
    if receipt.outcome == OutputCleanupOutcome::AlreadyAbsent {
        let recheck = (|| {
            let current = exact_fast_img_photos_cleanup_recovery(
                &expected.src_dir,
                output,
                &expected.strategy,
            )?;
            anyhow::ensure!(
                current == *expected,
                "saved checkpoint changed after cleanup recovery verification"
            );
            let _custody = foundation::fast_img::verify_output_photos_custody(&current, false)?;
            foundation::fast_img_store::verify_tier2_cleanup_complete(&current)?;
            anyhow::ensure!(
                matches!(fs::symlink_metadata(output), Err(error) if error.kind() == io::ErrorKind::NotFound),
                "working copy reappeared during cleanup recovery"
            );
            Ok::<_, anyhow::Error>(())
        })();
        if let Err(error) = recheck {
            receipt.outcome = OutputCleanupOutcome::Failed;
            receipt.error = Some(format!("{error:#}"));
        }
    }
    receipt
}

fn delete_fast_img_photos_outputs(
    output_dir: &Path,
    expected: Option<&foundation::pipeline::verification::WorkingCopyMarker>,
) -> OutputCleanupReceipt {
    delete_fast_img_outputs_with(output_dir, |marker| {
        if let Some(expected) = expected {
            anyhow::ensure!(
                marker == expected
                    && matches!(fs::symlink_metadata(&marker.src_dir),
                        Err(error) if error.kind() == io::ErrorKind::NotFound),
                "saved checkpoint or source state changed after cleanup recovery verification"
            );
        }
        let custody = foundation::fast_img::verify_output_photos_custody(marker, false)?;
        foundation::fast_img_store::verify_tier2_cleanup_complete(marker)?;
        let marker_source = marker.src_dir.clone();
        Ok(move |target: &Path| {
            custody.verify_before_removal(target)?;
            if expected.is_some() {
                anyhow::ensure!(
                    matches!(fs::symlink_metadata(&marker_source),
                        Err(error) if error.kind() == io::ErrorKind::NotFound),
                    "saved source path reappeared; remaining outputs retained"
                );
            }
            Ok(())
        })
    })
}

fn check_fast_img_cleanup_root(path: &Path, observed: &fs::Metadata) -> Result<fs::Metadata> {
    let current = fs::symlink_metadata(path).with_context(|| {
        format!(
            "inspect admitted fast-img cleanup directory {}",
            path.display()
        )
    })?;
    anyhow::ensure!(
        current.file_type().is_dir(),
        "fast-img cleanup directory is no longer a real directory: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(
            current.dev() == observed.dev() && current.ino() == observed.ino(),
            "fast-img cleanup output root changed: {}",
            path.display()
        );
    }
    #[cfg(not(unix))]
    let _ = observed;
    Ok(current)
}

fn delete_fast_img_outputs_with<V>(
    output_dir: &Path,
    prepare: impl FnOnce(&foundation::pipeline::verification::WorkingCopyMarker) -> Result<V>,
) -> OutputCleanupReceipt
where
    V: FnMut(&Path) -> Result<()>,
{
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
        check_fast_img_cleanup_root(output_dir, &metadata)?;
        let (marker, targets) = fast_img_marker_cleanup_targets(output_dir)?;
        let mut verify_custody = prepare(&marker)?;
        check_fast_img_cleanup_root(output_dir, &metadata)?;
        let mut removed = 0usize;
        let mut already_absent = 0usize;
        receipt.removed_files = Some(removed);
        receipt.already_absent_files = Some(already_absent);
        prune_candidates.insert(output_dir.to_path_buf());
        for (target, expected) in targets {
            let _transaction = foundation::batch_control::begin_transaction()?;
            check_fast_img_cleanup_root(output_dir, &metadata)?;
            foundation::safety::check_apple_photos_library(&target).map_err(anyhow::Error::msg)?;
            let observed_file = match fs::symlink_metadata(&target) {
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
                Ok(observed_file) => observed_file,
            };
            if !observed_file.is_file() {
                bail!(
                    "fast-img cleanup target is not a file: {}",
                    target.display()
                );
            }
            let true_format = foundation::image::format_detect::detect_regular_file_format(&target)
                .with_context(|| {
                    format!("fast-img cleanup probe failed for {}", target.display())
                })?;
            if true_format.canonical_extension() != Some(marker.strategy.as_str()) {
                bail!(
                    "fast-img cleanup output format differs from its selected strategy {} (true_format={true_format:?}, expected={})",
                    target.display(),
                    marker.strategy
                );
            }
            verify_custody(&target).context("fresh Photos custody before output removal")?;
            check_fast_img_cleanup_root(output_dir, &metadata)?;
            if foundation::metadata::find_aae_sidecar(&target)?.is_some() {
                continue;
            }
            remove_verified_fast_img_output_from_root(
                output_dir,
                &target,
                &expected,
                &metadata,
                &observed_file,
            )
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
        let _transaction = foundation::batch_control::begin_transaction()?;
        check_fast_img_cleanup_root(output_dir, &metadata)?;
        let pruned = fast_img_prune_empty_dirs(output_dir, &prune_candidates)?;
        receipt.pruned_directories = Some(pruned);
        match fs::symlink_metadata(output_dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
            Ok(current) if current.file_type().is_dir() => {
                let current = check_fast_img_cleanup_root(output_dir, &metadata)?;
                receipt.residual_inventory = Some(output_residual_inventory(output_dir, &current));
                Ok(false)
            }
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
    #[cfg(unix)]
    use foundation::io_utils::remove_verified_fast_img_output;

    #[cfg(unix)]
    #[test]
    fn verification_lease_subprocess_helper() -> Result<()> {
        let Some(operation) = std::env::var_os("MFB_TEST_VERIFY_OPERATION") else {
            return Ok(());
        };
        let output_path =
            PathBuf::from(std::env::var_os("MFB_TEST_VERIFY_REPLY").context("test reply path")?);
        let optimized =
            PathBuf::from(std::env::var_os("MFB_TEST_VERIFY_OUTPUT").context("test output path")?);
        let reply = if operation == "marker" {
            // Match verify's read-only early dispatch: inspect formal Core state
            // without taking a production lease or creating a missing store.
            let marker = match foundation::pipeline::verification::read_marker(&optimized) {
                Ok(marker) => Some(marker),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            serde_json::to_string(&serde_json::json!({"marker": marker,
                "marker_database_path": null, "marker_error": null}))?
        } else {
            anyhow::ensure!(operation == "verify", "unknown test verifier operation");
            let _lease = foundation::production_lock::acquire_production_lease()?;
            let source = PathBuf::from(
                std::env::var_os("MFB_TEST_VERIFY_SOURCE").context("test source path")?,
            );
            let count = |path: &Path| -> Result<usize> {
                Ok(fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?.len())
            };
            let source_count = count(&source)?;
            let optimized_count = count(&optimized)?;
            anyhow::ensure!(
                source_count == 0 && optimized_count == 0,
                "lease regression only accepts its empty synthetic directories"
            );
            format!(
                "{INTEGRITY_SUMMARY_JSON_PREFIX}{}\n",
                serde_json::json!({
                    "has_warnings": false, "issue_count": 0, "source_count": source_count,
                    "optimized_count": optimized_count, "skipped_count": 0, "failed_count": 0,
                    "source_remaining_count": 0, "verified_deleted_count": 0, "count_status": "MATCH"
                })
            )
        };
        fs::write(output_path, reply)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn timestamp_restore_inherits_production_lease() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        const STATE: &str = "MFB_TEST_TIMESTAMP_LEASE_STATE";
        let thread = std::thread::current();
        let test = thread.name().context("libtest timestamp test name")?;
        let executable = std::env::current_exe()?;
        match std::env::var(STATE).as_deref() {
            Ok("worker") => {
                let _lease = foundation::production_lock::acquire_production_lease()?;
            }
            Ok("launcher") => {
                let temporary = tempfile::tempdir()?;
                let root = temporary.path().canonicalize()?;
                let source = root.join("source");
                let output = root.join("output");
                fs::create_dir(&source)?;
                fs::create_dir(&output)?;
                let adapter = root.join("img");
                fs::write(
                    &adapter,
                    r#"#!/bin/sh
[ "$1" = "restore-timestamps" ] && [ -d "$2" ] && [ -d "$3" ] || exit 3
export MFB_TEST_TIMESTAMP_LEASE_STATE=worker
exec "$MFB_TEST_TIMESTAMP_TEST_EXE" "$MFB_TEST_TIMESTAMP_TEST_NAME" --exact --nocapture --test-threads=1
"#,
                )?;
                fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700))?;
                let _executable = foundation::common_utils::EnvGuard::set(
                    "MFB_TEST_TIMESTAMP_TEST_EXE",
                    executable
                        .to_str()
                        .context("timestamp test executable path")?,
                );
                let _name =
                    foundation::common_utils::EnvGuard::set("MFB_TEST_TIMESTAMP_TEST_NAME", test);
                let _lease = foundation::production_lock::acquire_production_lease()?;
                let result = run_timestamp_restore(&adapter, &source, &output)?;
                anyhow::ensure!(
                    result.status.success()
                        && String::from_utf8_lossy(&result.stdout).contains("running 1 test"),
                    "timestamp worker did not inherit its launcher's lease:\n{}\n{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
            _ => {
                let result = Command::new(executable)
                    .args([test, "--exact", "--nocapture", "--test-threads=1"])
                    .env(STATE, "launcher")
                    .output()?;
                anyhow::ensure!(
                    result.status.success()
                        && String::from_utf8_lossy(&result.stdout).contains("running 1 test"),
                    "isolated timestamp lease regression failed:\n{}\n{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn verification_lease_is_inherited_only_by_authorized_verifier() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        const DRIVER: &str = "MFB_TEST_VERIFY_LEASE_DRIVER";
        if std::env::var_os(DRIVER).is_none() {
            // Environment and the process-local active-lease registry belong to
            // this exact child, never other parallel unit tests.
            let thread = std::thread::current();
            let test = thread
                .name()
                .context("libtest must provide the exact test identity")?;
            let output = Command::new(std::env::current_exe()?)
                .args([test, "--exact", "--nocapture", "--test-threads=1"])
                .env(DRIVER, "1")
                .output()?;
            anyhow::ensure!(
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
                "isolated verification lease regression failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return Ok(());
        }
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        let source = root.join("source");
        let optimized = root.join("output");
        let state = root.join("state");
        fs::create_dir(&source)?;
        fs::create_dir(&optimized)?;
        let _state = foundation::common_utils::EnvGuard::set(
            "MFB_HOME_ROOT",
            state.to_str().context("test state path")?,
        );
        let _source = foundation::common_utils::EnvGuard::set(
            "MFB_TEST_VERIFY_SOURCE",
            source.to_str().context("test source path")?,
        );
        let _optimized = foundation::common_utils::EnvGuard::set(
            "MFB_TEST_VERIFY_OUTPUT",
            optimized.to_str().context("test output path")?,
        );
        let executable = std::env::current_exe()?;
        let _executable = foundation::common_utils::EnvGuard::set(
            "MFB_TEST_VERIFY_EXE",
            executable.to_str().context("test executable path")?,
        );
        let _reply = foundation::common_utils::EnvGuard::set(
            "MFB_TEST_VERIFY_REPLY",
            root.join("reply").to_str().context("test reply path")?,
        );
        let _child_log = foundation::common_utils::EnvGuard::set(
            "MFB_TEST_VERIFY_CHILD_LOG",
            root.join("child.log")
                .to_str()
                .context("test child log path")?,
        );
        let adapter = root.join("verify-adapter");
        fs::write(
            &adapter,
            r#"#!/bin/sh
case "$1" in
    --fast-img-marker-json) MFB_TEST_VERIFY_OPERATION=marker ;;
    --verify) MFB_TEST_VERIFY_OPERATION=verify ;;
    *) exit 2 ;;
esac
export MFB_TEST_VERIFY_OPERATION
"$MFB_TEST_VERIFY_EXE" --exact infra::drag_drop::tests::verification_lease_subprocess_helper --nocapture --test-threads=1 > "$MFB_TEST_VERIFY_CHILD_LOG" 2>&1
status=$?
if [ "$status" -ne 0 ]; then
    cat "$MFB_TEST_VERIFY_CHILD_LOG" >&2
    exit "$status"
fi
cat "$MFB_TEST_VERIFY_REPLY"
"#,
        )?;
        fs::set_permissions(&adapter, fs::Permissions::from_mode(0o700))?;
        let _lease = foundation::production_lock::acquire_production_lease()?;
        let blocked = Command::new(&adapter)
            .arg("--verify")
            .env_remove("MFB_PRODUCTION_TASK_LEASE_FD")
            .output()?;
        anyhow::ensure!(
            !blocked.status.success()
                && String::from_utf8_lossy(&blocked.stderr).contains("MFB_PRODUCTION_TASK_BUSY"),
            "an unrelated verifier must remain blocked by the production parent"
        );
        let (_, _, marker_error) = load_fast_img_marker_json(&adapter, &optimized)?;
        anyhow::ensure!(
            marker_error.is_none() && !state.exists(),
            "a read-only marker query must not require a lease or create state"
        );
        let outcome = run_unified_verification(
            &adapter,
            &source,
            Some(&optimized),
            ProcessingFilter::ImagesOnly,
            None,
            &root,
            "lease-regression",
            VerificationFlags {
                include_logs: false,
                auto_mode: true,
                fast_img: VerifyFastImgMode::None,
                strategy: None,
            },
        )?;
        assert_eq!(outcome.warnings, Some(false));
        assert_eq!(outcome.issue_count, 0);
        let summary = outcome
            .integrity_summary
            .context("delegated verifier omitted its actual result")?;
        assert_eq!((summary.source_count, summary.optimized_count), (0, 0));
        assert_eq!(summary.count_status.as_deref(), Some("MATCH"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn verifier_progress_arrives_before_the_integrity_report_and_exit() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let acknowledgment = temporary.path().join("parent-saw-progress");
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let summary = serde_json::json!({
            "has_warnings": false, "issue_count": 0, "source_count": 1,
            "optimized_count": 1, "skipped_count": 0, "failed_count": 0,
            "source_remaining_count": 0, "verified_deleted_count": 1, "count_status": "MATCH"
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", r#"
printf 'MFB_GATE_ACTIVITY={"schema_version":1,"stage_id":"%s-0","gate":"photos_custody","check":"original_bytes","completed":0,"total":1,"state":"running","file_progress":{"processed":0,"total":1}}\n' "$$"
printf 'ordinary diagnostic\n' >&2
count=0
until [ -f "$MFB_TEST_PROGRESS_ACK" ]; do
    count=$((count + 1))
    [ "$count" -lt 200 ] || exit 91
    sleep 0.025
done
printf '%s\n' "$MFB_TEST_VERIFY_SUMMARY"
"#])
        .env("MFB_TEST_PROGRESS_ACK", &acknowledgment)
        .env("MFB_TEST_VERIFY_SUMMARY", format!("{INTEGRITY_SUMMARY_JSON_PREFIX}{summary}"));
        let process = foundation::process_runner::ManagedProcess::spawn_captured_relaying(
            &mut command,
            move |pid, line| {
                let observed = line.split_once('=').is_some_and(|(marker, payload)| {
                    marker == "MFB_GATE_ACTIVITY"
                        && valid_independent_verify_progress(pid, marker, payload)
                });
                if observed {
                    let _ = sender.try_send(());
                }
                observed
            },
        )?;
        receiver.recv_timeout(std::time::Duration::from_secs(3))?;
        fs::write(acknowledgment, b"live")?;
        let result = process.wait()?;
        assert!(result.status.success());
        assert_eq!(result.stderr, "ordinary diagnostic\n");
        let (display, receipt) = parse_machine_integrity_summary(&result.stdout)?;
        assert!(display.is_empty());
        assert_eq!(receipt.count_status.as_deref(), Some("MATCH"));
        Ok(())
    }

    #[test]
    fn independent_verifier_progress_rejects_unbound_stage_and_private_fields() {
        let gate = "MFB_GATE_ACTIVITY";
        let running = r#"{"schema_version":1,"stage_id":"101-0","gate":"photos_custody","check":"original_bytes","completed":0,"total":1,"state":"running","file_progress":{"processed":0,"total":2}}"#;
        assert!(valid_independent_verify_progress(101, gate, running));
        assert!(!valid_independent_verify_progress(102, gate, running));
        let private_path = running.replacen(
            r#""schema_version":1"#,
            r#""schema_version":1,"path":"/synthetic/private.jpg""#,
            1,
        );
        assert!(!valid_independent_verify_progress(101, gate, &private_path));
        let premature = running
            .replace(r#""state":"running""#, r#""state":"passed""#)
            .replace(r#""completed":0"#, r#""completed":1"#);
        assert!(!valid_independent_verify_progress(101, gate, &premature));
        assert!(!valid_independent_verify_progress(
            101,
            "MFB_LOG_PHASE",
            r#"{"schema_version":1,"phase":"processing"}"#
        ));
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[serial_test::serial]
    fn historical_output_inventory_never_removes_a_recreated_directory() {
        use foundation::pipeline::verification::{
            FastImgStageName, WorkingCopyMarker, write_marker_atomic,
        };
        let root = tempfile::tempdir().unwrap();
        let canonical_root = root.path().canonicalize().unwrap();
        let source = canonical_root.join("source");
        let output = canonical_root.join("source_optimized");
        let current = canonical_root.join("source_optimized_2");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        let _state = foundation::common_utils::EnvGuard::set(
            "MFB_HOME_ROOT",
            root.path().join("state").to_str().unwrap(),
        );
        let mut marker = WorkingCopyMarker::new(source.clone(), output.clone(), 0);
        marker.stage = FastImgStageName::CleanupComplete;
        write_marker_atomic(&marker).unwrap();
        let report = observe_historical_fast_img_outputs(&source, "jxl", &current);
        assert_eq!(report.entries[0].outcome, "retained_empty");
        assert!(output.exists());
        for check in [
            &mut marker.gate1_checks.count,
            &mut marker.gate1_checks.blake3,
            &mut marker.gate1_checks.metadata,
            &mut marker.gate1_checks.size,
            &mut marker.gate1_checks.orient,
            &mut marker.gate1_checks.decode,
        ] {
            *check = true.into();
        }
        write_marker_atomic(&marker).unwrap();
        let report = observe_historical_fast_img_outputs(&source, "jxl", &current);
        assert_eq!(report.entries[0].outcome, "retained_local_delivery");
        assert!(output.exists());
        for check in [
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
        let unmanaged = canonical_root.join("source_optimized_99");
        fs::create_dir(&unmanaged).unwrap();
        let recovery = canonical_root.join("source_optimized_3");
        fs::create_dir(&recovery).unwrap();
        write_marker_atomic(&WorkingCopyMarker::new(source.clone(), recovery.clone(), 0)).unwrap();
        fs::create_dir(&current).unwrap();
        let mut current_marker = marker.clone();
        current_marker.working_copy.clone_from(&current);
        write_marker_atomic(&current_marker).unwrap();
        let report = observe_historical_fast_img_outputs(&source, "jxl", &current);
        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.entries[0].outcome, "retained_nonempty");
        assert_eq!(report.entries[0].regular_files, Some(1));
        assert_eq!(report.entries[0].regular_bytes, Some(9));
        assert_eq!(report.entries[1].outcome, "retained_recovery");
        assert_eq!(fs::read(&unknown).unwrap(), b"user data");
        fs::remove_file(unknown).unwrap();
        fs::remove_dir(&output).unwrap();
        let report = observe_historical_fast_img_outputs(&source, "jxl", &current);
        assert_eq!(report.entries[0].outcome, "absent");
        fs::create_dir(&output).unwrap();
        let report = observe_historical_fast_img_outputs(&source, "jxl", &current);
        assert_eq!(report.entries[0].outcome, "retained_empty");
        assert_eq!(report.entries[0].regular_files, Some(0));
        assert_eq!(report.entries[0].regular_bytes, Some(0));
        assert!(output.is_dir() && current.is_dir() && unmanaged.is_dir() && recovery.is_dir());
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn fast_img_cleanup_inventory_distinguishes_exact_copy_recovery_and_unknown() -> Result<()> {
        let test = std::thread::current();
        let name = test.name().context("cleanup inventory test name")?;
        if std::env::var("MFB_CLEANUP_INVENTORY_TEST").as_deref() != Ok(name) {
            let child = Command::new(std::env::current_exe()?)
                .args([name, "--exact", "--nocapture", "--test-threads=1"])
                .env("MFB_CLEANUP_INVENTORY_TEST", name)
                .output()?;
            anyhow::ensure!(
                child.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&child.stdout),
                String::from_utf8_lossy(&child.stderr)
            );
            return Ok(());
        }
        use foundation::pipeline::verification::{
            Blake3Entry, FastImgEncodingParameters, FastImgProcessingContract, FastImgStageName,
            WorkingCopyMarker, write_marker_atomic,
        };
        let temp = tempfile::tempdir()?;
        let root = temp.path().canonicalize()?;
        let state = root.join("state");
        let _state = foundation::common_utils::EnvGuard::set(
            "MFB_HOME_ROOT",
            state.to_str().context("synthetic state path")?,
        );
        let src = root.join("source");
        let output = root.join("source_optimized");
        let untracked = root.join("other_optimized");
        fs::create_dir(&src)?;
        fs::create_dir(&output)?;
        fs::create_dir(&untracked)?;
        fs::write(untracked.join("user.jpg"), b"unrelated")?;
        fs::create_dir(output.join("subdirectory"))?;
        std::os::unix::fs::symlink(&src, output.join("symbolic"))?;
        fs::write(output.join(".mfb-delivery-fake.JXL"), b"unknown")?;
        let mut marker = WorkingCopyMarker::new(src.clone(), output.clone(), 3);
        marker.stage = FastImgStageName::Gate1Passed;
        marker.processing_contract = Some(FastImgProcessingContract {
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
        for (relative, source, observed, recorded) in [
            (
                "copy",
                b"exact".as_slice(),
                b"exact".as_slice(),
                b"exact".as_slice(),
            ),
            ("protect", b"original", b"generated", b"generated"),
            ("drift", b"original", b"replaced", b"recorded"),
        ] {
            let source_rel = format!("{relative}.jxl");
            let output_rel = format!("{relative}.JXL");
            fs::write(src.join(&source_rel), source)?;
            fs::write(output.join(&output_rel), observed)?;
            marker.blake3_log.insert(
                source_rel,
                Blake3Entry {
                    src: blake3::hash(source).to_hex().to_string(),
                    out: blake3::hash(recorded).to_hex().to_string(),
                    out_rel: Some(output_rel),
                    library_asset: None,
                },
            );
        }
        marker.encoded_count = 3;
        write_marker_atomic(&marker)?;

        let source_times = fs::metadata(&src)?.modified()?;
        let output_times = fs::metadata(&output)?.modified()?;
        let report = inspect_fast_img_cleanup_inventory(&output);
        assert_eq!(report.state, "observed", "{:?}", report.reason);
        assert_eq!(
            (report.directories, report.symlinks, report.other_entries),
            (Some(1), Some(1), Some(0))
        );
        let groups: Vec<_> = report
            .categories
            .iter()
            .map(|group| (group.category, group.files, group.bytes))
            .collect();
        assert_eq!(
            groups,
            [
                (CleanupInventoryCategory::RegenerableOwned, 1, 5),
                (CleanupInventoryCategory::RecoveryProtected, 1, 9),
                (CleanupInventoryCategory::Unknown, 2, 15),
            ]
        );
        assert_eq!(report.categories[0].sample, ["copy.JXL"]);
        assert_eq!(
            report.categories[2].sample,
            [".mfb-delivery-fake.JXL", "drift.JXL"]
        );
        assert_eq!(
            inspect_fast_img_cleanup_inventory(&untracked).reason,
            Some("checkpoint_unavailable")
        );
        assert_eq!(
            inspect_fast_img_cleanup_inventory(&root.join("absent")).state,
            "absent"
        );
        assert_eq!(
            inspect_fast_img_cleanup_inventory(&output.join("symbolic")).reason,
            Some("unsafe_path")
        );
        assert_eq!(
            inspect_fast_img_cleanup_inventory(&output.join("symbolic/missing")).reason,
            Some("unsafe_path")
        );
        assert_eq!(fs::metadata(&src)?.modified()?, source_times);
        assert_eq!(fs::metadata(&output)?.modified()?, output_times);
        assert_eq!(fs::read(output.join("drift.JXL"))?, b"replaced");
        assert_eq!(foundation::fast_img_store::read_marker(&output)?, marker);

        marker.stage = FastImgStageName::Importing;
        write_marker_atomic(&marker)?;
        let imported = inspect_fast_img_cleanup_inventory(&output);
        assert_eq!(imported.categories[0].files, 0);
        assert_eq!(imported.categories[1].files, 2);
        Ok(())
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn output_residual_inventory_bounds_names_and_refuses_replacement_or_excess_depth() -> Result<()>
    {
        use std::os::unix::ffi::OsStrExt;

        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?.join("output");
        fs::create_dir(&root)?;
        for index in 0..=RESIDUAL_SAMPLE_LIMIT {
            fs::write(root.join(format!("synthetic-{index:02}")), b"a")?;
        }
        #[cfg(target_os = "linux")]
        fs::write(root.join(std::ffi::OsStr::from_bytes(b"name-\xff")), b"abc")?;
        #[cfg(target_os = "macos")]
        fs::write(
            root.join(std::ffi::OsStr::from_bytes(b"synthetic-extra")),
            b"abc",
        )?;
        let observed = output_residual_inventory(&root, &fs::symlink_metadata(&root)?);
        assert_eq!(observed.state, "observed");
        assert_eq!(observed.regular_files, Some(14));
        assert_eq!(observed.regular_bytes, Some(16));
        assert_eq!(observed.sample.len(), RESIDUAL_SAMPLE_LIMIT);
        assert!(observed.sample_truncated);

        let original = fs::symlink_metadata(&root)?;
        let archived = root.with_file_name("preserved-output");
        fs::rename(&root, &archived)?;
        fs::create_dir(&root)?;
        let replaced = output_residual_inventory(&root, &original);
        assert_eq!(replaced.state, "unknown");
        assert_eq!(replaced.error, Some("directory_changed"));
        assert!(replaced.regular_bytes.is_none() && replaced.sample.is_empty());
        assert_eq!(fs::read(archived.join("synthetic-00"))?, b"a");

        let nested = (0..=RESIDUAL_DEPTH_LIMIT).fold(root.clone(), |path, _| path.join("nested"));
        fs::create_dir_all(nested)?;
        let deep = output_residual_inventory(&root, &fs::symlink_metadata(&root)?);
        assert_eq!(deep.state, "unknown");
        assert_eq!(deep.error, Some("depth_limit"));
        assert!(deep.directories.is_none() && deep.sample.is_empty() && !deep.sample_truncated);
        Ok(())
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn output_residual_inventory_does_not_descend_photos_packages() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let root = temporary.path().canonicalize()?;
        for name in ["Archive.PhotosLibrary", "Older.PHOTOLIBRARY"] {
            let output = root.join(name.replace('.', "-"));
            let library = output.join("nested").join(name);
            fs::create_dir_all(&library)?;
            let protected = library.join("synthetic-private.txt");
            fs::write(&protected, b"unread protected synthetic payload")?;
            let observed = output_residual_inventory(&output, &fs::symlink_metadata(&output)?);
            assert_eq!(observed.state, "unknown");
            assert_eq!(observed.error, Some("inspection_failed"));
            assert!(observed.regular_files.is_none() && observed.sample.is_empty());
            assert_eq!(fs::read(&protected)?, b"unread protected synthetic payload");
        }
        Ok(())
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
            assert!(receipt.residual_inventory.is_none());
        }

        let source = root.path().join("source");
        let output = root.path().join("output");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&output).unwrap();
        let unknown = output.join("user.txt");
        let unknown_bytes = b"preserve user data";
        fs::write(&unknown, unknown_bytes).unwrap();
        let finder_metadata = output.join(".DS_Store");
        let finder_bytes = b"\x00\x00\x00\x01Bud1";
        fs::write(&finder_metadata, finder_bytes).unwrap();
        let nested = output.join("nested");
        fs::create_dir(&nested).unwrap();
        let nested_file = nested.join("留存.txt");
        let nested_bytes = b"nested synthetic file";
        fs::write(&nested_file, nested_bytes).unwrap();
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

        #[cfg(unix)]
        let (outside, link) = {
            let outside = root.path().join("outside/empty");
            fs::create_dir_all(&outside).unwrap();
            let link = output.join("linked-directory");
            std::os::unix::fs::symlink(outside.parent().unwrap(), &link).unwrap();
            (outside, link)
        };
        let residual = delete_fast_img_outputs_with(&output, |_| Ok(|_: &Path| Ok(())));
        assert_eq!(
            residual.outcome,
            OutputCleanupOutcome::Residual,
            "cleanup failure: {:?}",
            residual.error
        );
        assert_eq!(residual.removed_files, Some(0));
        assert_eq!(residual.already_absent_files, Some(0));
        assert_eq!(residual.pruned_directories, Some(0));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let inventory = residual.residual_inventory.unwrap();
            assert_eq!(inventory.state, "observed");
            assert_eq!(inventory.regular_files, Some(3));
            assert_eq!(
                inventory.regular_bytes,
                Some((unknown_bytes.len() + finder_bytes.len() + nested_bytes.len()) as u64)
            );
            assert_eq!(inventory.directories, Some(1));
            assert_eq!(inventory.symlinks, Some(1));
            assert_eq!(inventory.other_entries, Some(0));
            assert!(inventory.error.is_none());
            assert!(!inventory.sample_truncated);
            assert!(
                inventory
                    .sample
                    .iter()
                    .any(|entry| entry.relative_path == "nested/留存.txt"
                        && entry.kind == ResidualEntryKind::RegularFile)
            );
            assert!(
                inventory
                    .sample
                    .iter()
                    .any(|entry| entry.relative_path == "linked-directory"
                        && entry.kind == ResidualEntryKind::Symlink)
            );
        }
        assert_eq!(fs::read(&unknown).unwrap(), b"preserve user data");
        assert_eq!(fs::read(&finder_metadata).unwrap(), finder_bytes);
        assert_eq!(fs::read(&nested_file).unwrap(), b"nested synthetic file");
        fs::remove_file(nested_file).unwrap();
        fs::remove_dir(nested).unwrap();
        #[cfg(unix)]
        {
            assert!(outside.is_dir());
            assert!(link.is_symlink());
            fs::remove_file(link).unwrap();
        }

        fs::remove_file(unknown).unwrap();
        fs::write(&finder_metadata, b"unrecognized contents").unwrap();
        let residual = delete_fast_img_outputs_with(&output, |_| Ok(|_: &Path| Ok(())));
        assert_eq!(residual.outcome, OutputCleanupOutcome::Residual);
        assert_eq!(
            fs::read(&finder_metadata).unwrap(),
            b"unrecognized contents"
        );
        fs::write(&finder_metadata, finder_bytes).unwrap();
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

        let no_live_custody = delete_fast_img_shortest_path_output_dir(&output);
        assert_eq!(no_live_custody.outcome, OutputCleanupOutcome::Failed);
        assert!(first_output.exists() && second_output.exists());
        assert_eq!(fs::read(&finder_metadata).unwrap(), finder_bytes);
        let rejected_before_unlink = delete_fast_img_outputs_with(&output, |_| {
            Ok(|_: &Path| anyhow::bail!("fresh Photos UUID has no original resource"))
        });
        assert_eq!(rejected_before_unlink.outcome, OutputCleanupOutcome::Failed);
        assert_eq!(rejected_before_unlink.removed_files, Some(0));
        assert!(first_output.exists() && second_output.exists());

        #[cfg(unix)]
        {
            let moved = output.with_extension("moved");
            let replaced = delete_fast_img_outputs_with(&output, |_| {
                Ok(|_: &Path| -> Result<()> {
                    fs::rename(&output, &moved)?;
                    fs::create_dir(&output)?;
                    fs::write(&first_output, first_bytes)?;
                    Ok(())
                })
            });
            assert_eq!(replaced.outcome, OutputCleanupOutcome::Failed);
            assert_eq!(replaced.removed_files, Some(0));
            assert!(replaced.error.unwrap().contains("output root changed"));
            assert_eq!(fs::read(&first_output).unwrap(), first_bytes);
            assert_eq!(fs::read(moved.join("a.JXL")).unwrap(), first_bytes);
            fs::remove_file(&first_output).unwrap();
            fs::remove_dir(&output).unwrap();
            fs::rename(&moved, &output).unwrap();

            let displaced = output.join("displaced.JXL");
            let replaced_file = delete_fast_img_outputs_with(&output, |_| {
                Ok(|_: &Path| -> Result<()> {
                    fs::rename(&first_output, &displaced)?;
                    fs::write(&first_output, first_bytes)?;
                    Ok(())
                })
            });
            assert_eq!(replaced_file.outcome, OutputCleanupOutcome::Failed);
            assert_eq!(replaced_file.removed_files, Some(0));
            assert!(replaced_file.error.unwrap().contains("target replaced"));
            assert_eq!(fs::read(&first_output).unwrap(), first_bytes);
            assert_eq!(fs::read(&displaced).unwrap(), first_bytes);
            fs::remove_file(&first_output).unwrap();
            fs::rename(displaced, &first_output).unwrap();
        }

        let failed = delete_fast_img_outputs_with(&output, |_| Ok(|_: &Path| Ok(())));
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

        let mut control_dir = tempfile::Builder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            control_dir.permissions(fs::Permissions::from_mode(0o700));
        }
        let control = control_dir.tempdir().unwrap();
        let request = control.path().join("request");
        fs::write(&request, b"cancelled\ncleanup-test").unwrap();
        {
            let _control = foundation::common_utils::EnvGuard::set(
                "MFB_BATCH_CONTROL_FILE",
                request.to_str().unwrap(),
            );
            let cancelled = delete_fast_img_outputs_with(&output, |_| {
                Ok(|_: &Path| -> Result<()> {
                    panic!("cancelled cleanup must not probe another file")
                })
            });
            assert_eq!(cancelled.outcome, OutputCleanupOutcome::Failed);
            assert_eq!(cancelled.removed_files, Some(0));
            assert!(
                cancelled
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("batch cancelled at a safe boundary")),
                "{:?}",
                cancelled.error
            );
            assert_eq!(fs::read(&second_output).unwrap(), second_actual_bytes);
        }

        let second_actual_hash = blake3::hash(&second_actual_bytes).to_hex().to_string();
        marker.blake3_log.insert(
            "b.jpg".to_owned(),
            entry("b.jpg", "b.JXL", second_actual_hash),
        );
        write_marker_atomic(&marker).unwrap();
        fs::create_dir_all(output.join("metadata-mirror/ignored-format")).unwrap();
        let retained = delete_fast_img_outputs_with(&output, |_| Ok(|_: &Path| Ok(())));
        assert_eq!(retained.outcome, OutputCleanupOutcome::Residual);
        assert_eq!(retained.removed_files, Some(1));
        assert_eq!(retained.already_absent_files, Some(1));
        assert_eq!(retained.pruned_directories, Some(0));
        assert!(output.join("metadata-mirror/ignored-format").is_dir());
        assert!(finder_metadata.is_file());
        fs::remove_dir(output.join("metadata-mirror/ignored-format")).unwrap();
        fs::remove_dir(output.join("metadata-mirror")).unwrap();
        let removed = delete_fast_img_outputs_with(&output, |_| Ok(|_: &Path| Ok(())));
        assert_eq!(removed.outcome, OutputCleanupOutcome::Removed);
        assert_eq!(removed.removed_files, Some(0));
        assert_eq!(removed.already_absent_files, Some(2));
        assert_eq!(removed.pruned_directories, Some(1));
        assert!(!output.exists());
    }

    #[test]
    fn photos_quarantine_receipt_binds_paths_and_verified_pending_outputs() {
        let stats = crate::infra::process_stream::ProcessorStats {
            skipped: 4,
            unprocessed: Some(4),
            exit_code: 2,
            reported: [true; 4],
            ..Default::default()
        };
        let report = serde_json::json!({
            "photos_quarantine_blocked": 4, "encoded": 4, "photos_verified": 4,
            "photos_quarantine_locations": {"working_copies": 0, "photos_originals": 4, "affected": 4},
            "source_retained": 8, "failed_sources_retained": 0, "failed_sources_verified": 0,
            "failed_source_verification_error": null, "retention_unknown_reason": null,
            "finalization_error": "quarantine blocked Gate 2", "photos_delivery_deferred": false,
            "primary_verification_complete": true, "primary_cleanup_complete": false,
            "retained_only_complete": false, "source_directory": "/input", "output_directory": "/output"
        });
        let verify = |value: &Value| {
            PhotosQuarantineBlocked::from_receipt(
                value,
                &stats,
                Path::new("/input"),
                Path::new("/output"),
            )
            .map(|error| error.count)
        };
        assert_eq!(verify(&report), Some(4));
        for (key, value) in [
            (
                "photos_quarantine_locations",
                serde_json::json!({"working_copies": 0, "photos_originals": 3, "affected": 4}),
            ),
            ("photos_quarantine_blocked", 5.into()),
            ("source_retained", 7.into()),
            ("photos_verified", 3.into()),
            ("output_directory", "/other".into()),
            ("primary_cleanup_complete", true.into()),
            ("photos_delivery_deferred", true.into()),
        ] {
            let mut conflicting = report.clone();
            conflicting[key] = value;
            assert_eq!(verify(&conflicting), None, "{key}");
        }
    }

    #[test]
    fn retained_only_receipt_binds_paths_and_all_dispositions() {
        let stats = crate::infra::process_stream::ProcessorStats {
            failed: 1,
            skipped: 1,
            reported: [true; 4],
            unprocessed: Some(0),
            exit_code: 1,
            ..Default::default()
        };
        let receipt = serde_json::json!({
            "schema_version": 1, "succeeded": 0, "skipped": 1, "ignored": 0, "failed": 1,
            "unprocessed": 0, "photos_verified": 0, "encoded": 0, "failed_sources_retained": 1,
            "failed_sources_verified": 1, "source_retained": 2, "retained_only_complete": true,
            "primary_cleanup_complete": false, "photos_delivery_deferred": false,
            "finalization_error": null, "retention_unknown_reason": null,
            "failed_source_verification_error": null, "source_directory": "/input",
            "output_directory": "/output"
        });
        let verified = |value: &serde_json::Value| {
            fast_img_retained_only_verified(
                &format!("MFB_FAST_IMG_RESULT={value}"),
                &stats,
                Path::new("/input"),
                Path::new("/output"),
            )
        };
        assert!(verified(&receipt));
        for (key, value) in [
            ("source_directory", "/other".into()),
            ("output_directory", "/other".into()),
            ("retained_only_complete", false.into()),
            ("source_retained", 1.into()),
            ("encoded", 1.into()),
            ("photos_verified", 1.into()),
            ("retention_unknown_reason", "source changed".into()),
        ] {
            let mut invalid = receipt.clone();
            invalid[key] = value;
            assert!(!verified(&invalid), "{key}");
        }
        assert_eq!(
            fast_img_partial_cleanup_verified(&format!("MFB_FAST_IMG_RESULT={receipt}"), &stats),
            None
        );
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
        let error = remove_verified_fast_img_output(root.path(), &target, &expected).unwrap_err();
        assert_eq!(
            error.downcast_ref::<io::Error>().map(io::Error::kind),
            Some(io::ErrorKind::NotFound)
        );
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
        let unavailable = "MFB_INTEGRITY_UNAVAILABLE_JSON {\"status\":\"unavailable\"}\n";
        let error = parse_machine_integrity_summary(unavailable)
            .unwrap_err()
            .to_string();
        assert!(error.contains("could not complete"));
        assert!(parse_machine_integrity_summary(&format!("{stdout}{unavailable}")).is_err());
        assert!(
            parse_machine_integrity_summary(
                "MFB_INTEGRITY_UNAVAILABLE_JSON {\"status\":\"clean\"}\n"
            )
            .is_err()
        );
    }

    #[test]
    fn unavailable_integrity_result_reports_only_a_controlled_reason() {
        let record = |body| format!("{INTEGRITY_UNAVAILABLE_JSON_PREFIX}{body}\n");
        let valid = record(r#"{"status":"unavailable","reason":"photos_custody"}"#);
        assert_eq!(
            parse_machine_integrity_summary(&valid)
                .unwrap_err()
                .to_string(),
            "could not verify current Photos original-resource custody"
        );
        let invalid = [
            record(r#"{"status":"unavailable","reason":"private fixture path"}"#),
            record(
                r#"{"status":"unavailable","reason":"photos_custody","detail":"private fixture path"}"#,
            ),
            record(r#"{"status":"unavailable","reason":"checkpoint","reason":"photos_custody"}"#),
            record(&" ".repeat(257)),
        ];
        for output in invalid {
            assert_eq!(
                parse_machine_integrity_summary(&output)
                    .unwrap_err()
                    .to_string(),
                "invalid verify unavailable integrity result"
            );
        }
        assert_eq!(
            parse_machine_integrity_summary(&format!("{valid}{valid}"))
                .unwrap_err()
                .to_string(),
            "verify emitted more than one machine integrity result"
        );
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

//! Shared media-scope rules for Python orchestration (drag-and-drop, verify).
//!
//! Single source of truth for animation detection and pipeline routing.

#![allow(clippy::implicit_hasher)]

use anyhow::{Context, Result, anyhow};
use foundation::image::format_detect::{FormatKind, detect_true_format as rust_detect_true_format};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const PIPELINE_IMAGE: &str = "image";
pub const PIPELINE_VIDEO: &str = "video";
pub const HANDOFF_PRESERVE_PHASE_POST_IMG_VID: &str = "post_img_vid";

pub const MFB_AUDIT_PREFIX: &str = "MFB_AUDIT";

pub const SKIP_EXTS: &[&str] = &[
    "json", "jsonl", "txt", "log", "md", "sqlite", "db", "sh", "py", "rs", "toml",
];

pub const PURE_VIDEO_FORMATS: &[&str] = &["mp4", "mov", "mkv", "webm"];

pub const VID_STATIC_IGNORE_CLASSES: &[&str] =
    &["vid_static_single_frame", "vid_static_unknown_frames"];

pub const VID_HANDOFF_IGNORE_CLASSES: &[&str] = &[
    "vid_static_single_frame",
    "vid_static_unknown_frames",
    "vid_out_of_domain",
];

pub const IMG_ANIMATED_HANDOFF_CLASSES: &[&str] = &["img_animated_handoff"];

pub const IMG_IGNORE_CLASSES: &[&str] = &[
    "img_animated_handoff",
    "img_analysis_uncertainty",
    "img_strict_entropy_missing",
    "img_animation_ambiguity",
];

#[derive(Debug, Clone)]
pub struct HandoffPreserveCandidate {
    pub rel_path: String,
    pub size_bytes: u64,
}

fn read_prefix(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; limit];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

pub fn detect_true_format(path: &Path) -> Result<String> {
    let kind = rust_detect_true_format(path)?;
    Ok(match kind {
        FormatKind::Jpeg => "jpeg".to_string(),
        FormatKind::Png => "png".to_string(),
        FormatKind::Heic => "heic".to_string(),
        FormatKind::Heif => "heif".to_string(),
        FormatKind::Avif => "avif".to_string(),
        FormatKind::WebP => "webp".to_string(),
        FormatKind::Gif => "gif".to_string(),
        FormatKind::Bmp => "bmp".to_string(),
        FormatKind::Jxl => "jxl".to_string(),
        FormatKind::Tiff => "tiff".to_string(),
        FormatKind::Qoi => "qoi".to_string(),
        FormatKind::Jp2 => "jp2".to_string(),
        FormatKind::Ico => "ico".to_string(),
        FormatKind::Exr => "exr".to_string(),
        FormatKind::Flif => "flif".to_string(),
        FormatKind::Psd => "psd".to_string(),
        FormatKind::Pnm => "pnm".to_string(),
        FormatKind::Dds => "dds".to_string(),
        FormatKind::Mp4 => "mp4".to_string(),
        FormatKind::Mov => "mov".to_string(),
        FormatKind::Mkv => "mkv".to_string(),
        FormatKind::Webm => "webm".to_string(),
        FormatKind::Unknown => "unknown".to_string(),
    })
}

pub fn is_animated_webp(path: &Path) -> Result<bool> {
    let data = read_prefix(path, 1024 * 1024)?;
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return Err(anyhow!(
            "WebP animation probe failed for {}: missing RIFF/WEBP header",
            path.display()
        ));
    }
    // Check for ANIM or ANMF chunks in the prefix data
    let has_anim = data.windows(4).any(|w| w == b"ANIM" || w == b"ANMF");
    Ok(has_anim)
}

pub fn is_animated_png(path: &Path) -> Result<bool> {
    let data = read_prefix(path, 1024 * 1024)?;
    if !data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err(anyhow!(
            "PNG animation probe failed for {}: missing PNG signature",
            path.display()
        ));
    }
    if data.len() < 24 || &data[12..16] != b"IHDR" {
        return Err(anyhow!(
            "PNG animation probe failed for {}: malformed or truncated IHDR",
            path.display()
        ));
    }
    let has_actl = data.windows(4).any(|w| w == b"acTL");
    Ok(has_actl)
}

pub fn is_probably_animated_isobmff(path: &Path) -> Result<bool> {
    let data = read_prefix(path, 4096)?;
    if data.len() < 16 || &data[4..8] != b"ftyp" {
        return Err(anyhow!(
            "ISOBMFF animation probe failed for {}: malformed ftyp header",
            path.display()
        ));
    }
    let mut size_bytes = [0u8; 4];
    size_bytes.copy_from_slice(&data[0..4]);
    let box_size = u32::from_be_bytes(size_bytes) as usize;
    if box_size < 16 || box_size > data.len() {
        return Err(anyhow!(
            "ISOBMFF animation probe failed for {}: invalid ftyp box size {}",
            path.display(),
            box_size
        ));
    }
    let brand_bytes = &data[8..box_size];
    for chunk in brand_bytes.as_chunks::<4>().0 {
        if chunk == b"avis" || chunk == b"msf1" {
            return Ok(true);
        }
    }
    Ok(false)
}

#[must_use]
pub fn parse_jxlinfo_animation_hint(output: &str) -> Option<bool> {
    let normalized = output.to_lowercase();
    for line in normalized.lines() {
        if line.contains("have_animation:")
            && let Some(token) = line
                .split("have_animation:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
        {
            if token == "1" {
                return Some(true);
            }
            if token == "0" {
                return Some(false);
            }
        }
        if line.contains("animation length:")
            && let Some(token) = line
                .split("animation length:")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
        {
            match token.parse::<f64>() {
                Ok(len) => return Some(len > 0.0),
                Err(_err) => {}
            }
        }
    }
    if normalized.lines().any(|l| l.starts_with("jpeg xl image")) {
        return Some(false);
    }
    None
}

fn which_binary(name: &str) -> Option<PathBuf> {
    if let Some(path_var) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&path_var) {
            let candidate = path.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

pub fn is_animated_jxl(path: &Path) -> Result<bool> {
    if detect_true_format(path)? != "jxl" {
        return Err(anyhow!(
            "JXL animation probe failed for {}: true format is not JXL",
            path.display()
        ));
    }
    let jxlinfo = which_binary("jxlinfo").ok_or_else(|| {
        anyhow!(
            "JXL animation probe failed for {}: jxlinfo not found",
            path.display()
        )
    })?;

    let output = Command::new(jxlinfo)
        .arg(path)
        .output()
        .context(format!("JXL animation probe failed for {}", path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(anyhow!(
            "JXL animation probe failed for {}: jxlinfo exited {} ({})",
            path.display(),
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_jxlinfo_animation_hint(&combined).ok_or_else(|| {
        anyhow!(
            "JXL animation probe failed for {}: jxlinfo output had no animation verdict",
            path.display()
        )
    })?;
    Ok(parsed)
}

pub fn is_animated_gif(path: &Path) -> Result<bool> {
    let data = fs::read(path)?;
    if data.len() < 13 || (&data[0..6] != b"GIF87a" && &data[0..6] != b"GIF89a") {
        return Err(anyhow!(
            "GIF animation probe failed for {}: malformed or truncated header",
            path.display()
        ));
    }

    let mut pos = 6;
    if pos + 7 > data.len() {
        return Err(anyhow!(
            "GIF animation probe failed for {}: truncated logical screen",
            path.display()
        ));
    }
    let packed = data[pos + 4];
    let has_gct = (packed & 0x80) != 0;
    let gct_size = if has_gct {
        3 * (1 << ((packed & 0x07) + 1))
    } else {
        0
    };
    pos += 7 + gct_size;
    if pos > data.len() {
        return Err(anyhow!(
            "GIF animation probe failed for {}: truncated global color table",
            path.display()
        ));
    }

    let mut image_descriptors = 0;
    let mut gce_count = 0;
    let mut saw_trailer = false;

    while pos < data.len() {
        let byte = data[pos];
        if byte == 0x2C {
            image_descriptors += 1;
            if pos + 10 > data.len() {
                return Err(anyhow!(
                    "GIF animation probe failed for {}: truncated image descriptor",
                    path.display()
                ));
            }
            let img_packed = data[pos + 9];
            let has_local_color_table = (img_packed & 0x80) != 0;
            let lct_size = if has_local_color_table {
                3 * (1 << ((img_packed & 0x07) + 1))
            } else {
                0
            };
            pos += 10 + lct_size;
            if pos >= data.len() {
                return Err(anyhow!(
                    "GIF animation probe failed for {}: missing image data",
                    path.display()
                ));
            }
            pos += 1;
            loop {
                if pos >= data.len() {
                    return Err(anyhow!(
                        "GIF animation probe failed for {}: unterminated image data",
                        path.display()
                    ));
                }
                let block_size = data[pos] as usize;
                pos += 1;
                if block_size == 0 {
                    break;
                }
                if pos + block_size > data.len() {
                    return Err(anyhow!(
                        "GIF animation probe failed for {}: truncated image data block",
                        path.display()
                    ));
                }
                pos += block_size;
            }
        } else if byte == 0x21 {
            if pos + 2 > data.len() {
                return Err(anyhow!(
                    "GIF animation probe failed for {}: truncated extension block",
                    path.display()
                ));
            }
            let label = data[pos + 1];
            if label == 0xF9 {
                gce_count += 1;
            }
            pos += 2;
            loop {
                if pos >= data.len() {
                    return Err(anyhow!(
                        "GIF animation probe failed for {}: unterminated extension block",
                        path.display()
                    ));
                }
                let block_size = data[pos] as usize;
                pos += 1;
                if block_size == 0 {
                    break;
                }
                if pos + block_size > data.len() {
                    return Err(anyhow!(
                        "GIF animation probe failed for {}: truncated extension data",
                        path.display()
                    ));
                }
                pos += block_size;
            }
        } else if byte == 0x3B {
            saw_trailer = true;
            break;
        } else {
            return Err(anyhow!(
                "GIF animation probe failed for {}: unexpected block byte 0x{:02x}",
                path.display(),
                byte
            ));
        }
    }

    if !saw_trailer {
        return Err(anyhow!(
            "GIF animation probe failed for {}: missing trailer",
            path.display()
        ));
    }

    let frame_count = if gce_count > 1 {
        gce_count
    } else {
        image_descriptors
    };
    Ok(frame_count > 1)
}

pub fn true_format_owner(path: &Path, true_format: &str) -> Result<Option<String>> {
    if true_format == "unknown" {
        return Ok(None);
    }
    if PURE_VIDEO_FORMATS.contains(&true_format) {
        return Ok(Some(PIPELINE_VIDEO.to_string()));
    }
    if true_format == "gif" {
        return Ok(Some(
            if is_animated_gif(path)? {
                PIPELINE_VIDEO
            } else {
                PIPELINE_IMAGE
            }
            .to_string(),
        ));
    }
    if true_format == "webp" {
        return Ok(Some(
            if is_animated_webp(path)? {
                PIPELINE_VIDEO
            } else {
                PIPELINE_IMAGE
            }
            .to_string(),
        ));
    }
    if true_format == "png" {
        return Ok(Some(
            if is_animated_png(path)? {
                PIPELINE_VIDEO
            } else {
                PIPELINE_IMAGE
            }
            .to_string(),
        ));
    }
    if ["avif", "heic", "heif"].contains(&true_format) {
        return Ok(Some(
            if is_probably_animated_isobmff(path)? {
                PIPELINE_VIDEO
            } else {
                PIPELINE_IMAGE
            }
            .to_string(),
        ));
    }
    if true_format == "jxl" {
        return Ok(Some(
            if is_animated_jxl(path)? {
                PIPELINE_VIDEO
            } else {
                PIPELINE_IMAGE
            }
            .to_string(),
        ));
    }
    Ok(Some(PIPELINE_IMAGE.to_string()))
}

pub fn classify_media_owner(path: &Path) -> Result<Option<String>> {
    let filename = match path.file_name().and_then(|f| f.to_str()) {
        Some(f) => f,
        None => return Ok(None),
    };
    if filename.starts_with('.') {
        return Ok(None);
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && SKIP_EXTS.contains(&ext.to_lowercase().as_str())
    {
        return Ok(None);
    }
    let true_format = detect_true_format(path)?;
    true_format_owner(path, &true_format)
}

pub fn true_format_matches_processing_mode(
    path: &Path,
    true_format: &str,
    processing_mode: &str,
) -> Result<bool> {
    let owner = true_format_owner(path, true_format)?;
    Ok(match owner {
        None => false,
        Some(o) => {
            if processing_mode == "images_only" {
                o == PIPELINE_IMAGE
            } else if processing_mode == "videos_only" {
                o == PIPELINE_VIDEO
            } else {
                true
            }
        }
    })
}

pub fn matches_processing_mode(path: &Path, processing_mode: &str) -> Result<bool> {
    let owner = classify_media_owner(path)?;
    Ok(match owner {
        None => false,
        Some(o) => {
            if processing_mode == "images_only" {
                o == PIPELINE_IMAGE
            } else if processing_mode == "videos_only" {
                o == PIPELINE_VIDEO
            } else {
                true
            }
        }
    })
}

pub fn animation_label(path: &Path) -> Result<Option<String>> {
    let true_format = detect_true_format(path)?;
    if true_format == "webp" && is_animated_webp(path)? {
        return Ok(Some("animated WebP (ANIM/ANMF)".to_string()));
    }
    if true_format == "gif" && is_animated_gif(path)? {
        return Ok(Some("animated GIF".to_string()));
    }
    if true_format == "png" && is_animated_png(path)? {
        return Ok(Some("animated PNG (APNG/acTL)".to_string()));
    }
    if ["avif", "heic", "heif"].contains(&true_format.as_str())
        && is_probably_animated_isobmff(path)?
    {
        return Ok(Some(format!(
            "animated {} sequence (ISOBMFF)",
            true_format.to_uppercase()
        )));
    }
    if true_format == "jxl" && is_animated_jxl(path)? {
        return Ok(Some("animated JXL".to_string()));
    }
    Ok(None)
}

#[must_use]
pub fn format_session_audit_routed(pipeline: &str, rel_path: &str) -> String {
    format!("ROUTED pipeline={pipeline} path={rel_path}")
}

#[must_use]
pub fn format_session_audit_preserve_handoff(rel_path: &str) -> String {
    format!("PRESERVE_HANDOFF path={rel_path}")
}

#[must_use]
pub fn format_audit_event(category: &str, fields: &HashMap<String, String>) -> String {
    let mut parts: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
    parts.sort();
    let tail = parts.join(" ");
    format!("{category} {tail}").trim().to_string()
}

#[must_use]
pub fn audit_handoff_blocked(reason: &str, extra: &HashMap<String, String>) -> String {
    let mut fields = extra.clone();
    fields.insert("reason".to_string(), reason.to_string());
    format_audit_event("HANDOFF_PRESERVE_BLOCKED", &fields)
}

#[must_use]
pub fn integrity_stem_key(rel: &Path) -> String {
    let stem = rel.with_extension("");
    let stem_str = stem.to_string_lossy();
    stem_str.to_lowercase().trim().to_string()
}

pub fn collect_optimized_stem_keys(optimized_root: &Path) -> Result<HashSet<String>> {
    let mut keys = HashSet::new();
    let opt_resolved = optimized_root
        .canonicalize()
        .unwrap_or_else(|_| optimized_root.to_path_buf());
    if !opt_resolved.is_dir() {
        return Ok(keys);
    }
    for entry in walkdir::WalkDir::new(&opt_resolved) {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && let Some(fname) = path.file_name().and_then(|f| f.to_str())
        {
            if fname.starts_with('.') {
                continue;
            }
            if let Some(ext) = path.extension().and_then(|e| e.to_str())
                && SKIP_EXTS.contains(&ext.to_lowercase().as_str())
            {
                continue;
            }
            let true_format = detect_true_format(path)?;
            if true_format == "unknown" {
                continue;
            }
            match path.strip_prefix(&opt_resolved) {
                Ok(rel) => {
                    keys.insert(integrity_stem_key(rel));
                }
                Err(_err) => {}
            }
        }
    }
    Ok(keys)
}

#[must_use]
pub fn optimized_has_stem_match(optimized_root: &Path, rel_path: &Path) -> bool {
    let keys = collect_optimized_stem_keys(optimized_root).unwrap_or_default();
    keys.contains(&integrity_stem_key(rel_path))
}

#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn format_bytes(size_bytes: u64) -> String {
    if size_bytes < 1024 {
        format!("{size_bytes} B")
    } else if size_bytes < 1024 * 1024 {
        format!("{:.1} KB", size_bytes as f64 / 1024.0)
    } else if size_bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", size_bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", size_bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

pub fn list_handoff_preserve_candidates(
    source_root: &Path,
    optimized_root: &Path,
    video_rel_paths: &[String],
) -> Result<Vec<HandoffPreserveCandidate>> {
    let src_resolved = source_root
        .canonicalize()
        .unwrap_or_else(|_| source_root.to_path_buf());
    let opt_resolved = optimized_root
        .canonicalize()
        .unwrap_or_else(|_| optimized_root.to_path_buf());
    if !src_resolved.is_dir() || !opt_resolved.is_dir() {
        return Ok(Vec::new());
    }

    let stem_keys = collect_optimized_stem_keys(&opt_resolved)?;
    let mut candidates = Vec::new();

    let mut sorted_vids = video_rel_paths.to_vec();
    sorted_vids.sort();

    for rel_s in sorted_vids {
        let rel = Path::new(&rel_s);
        let src = src_resolved.join(rel);
        if !src.is_file() {
            continue;
        }
        if stem_keys.contains(&integrity_stem_key(rel)) {
            continue;
        }
        let size = src.metadata()?.len();
        candidates.push(HandoffPreserveCandidate {
            rel_path: rel.to_string_lossy().to_string(),
            size_bytes: size,
        });
    }

    Ok(candidates)
}

#[must_use]
pub fn is_img_classified_ignore(rust: &HashMap<String, String>) -> bool {
    if rust.get("pipeline").map(std::string::String::as_str) != Some("img")
        || rust.get("outcome").map(std::string::String::as_str) != Some("ignored")
    {
        return false;
    }
    let ignore_class = rust.get("ignore_class").map_or("", |s| s.trim());
    IMG_IGNORE_CLASSES.contains(&ignore_class)
}

#[must_use]
pub fn lookup_rust_outcomes_for_rel(
    rust_outcomes: &HashMap<String, Vec<HashMap<String, String>>>,
    rel_s: &str,
    source_dir: Option<&Path>,
) -> Vec<HashMap<String, String>> {
    let mut hits = Vec::new();
    for (audit_path, records) in rust_outcomes {
        if audit_path_matches_rel(audit_path, rel_s, source_dir) {
            hits.extend(records.clone());
        }
    }
    hits
}

fn audit_path_matches_rel(audit_path: &str, rel_s: &str, source_dir: Option<&Path>) -> bool {
    let rel_norm = rel_s.replace('\\', "/");
    if let Some(s_dir) = source_dir {
        match s_dir.canonicalize() {
            Ok(s_abs) => match Path::new(audit_path).canonicalize() {
                Ok(a_abs) => match a_abs.strip_prefix(&s_abs) {
                    Ok(key) => {
                        return key.to_string_lossy().replace('\\', "/") == rel_norm;
                    }
                    Err(_err) => {}
                },
                Err(_err) => {}
            },
            Err(_err) => {}
        }
    }
    let norm = audit_path.replace('\\', "/");
    norm.ends_with(&format!("/{rel_norm}")) || norm.ends_with(&rel_norm) || norm.contains(&rel_norm)
}

#[must_use]
pub fn lookup_rust_outcome(
    rust_outcomes: &HashMap<String, Vec<HashMap<String, String>>>,
    rel_s: &str,
    source_dir: Option<&Path>,
) -> Option<HashMap<String, String>> {
    let hits = lookup_rust_outcomes_for_rel(rust_outcomes, rel_s, source_dir);
    if hits.is_empty() {
        return None;
    }
    for record in hits.iter().rev() {
        if record.get("pipeline").map(std::string::String::as_str) == Some("img") {
            return Some(record.clone());
        }
    }
    hits.last().cloned()
}

pub fn classify_missing_entry(
    path: &Path,
    processing_mode: &str,
    routing: Option<&HashMap<String, String>>,
    rust_outcomes: Option<&HashMap<String, Vec<HashMap<String, String>>>>,
    source_dir: Option<&Path>,
) -> Result<(String, String)> {
    let owner = classify_media_owner(path)?;
    let anim = animation_label(path)?;
    let true_format = detect_true_format(path)?;

    let rel_s = match source_dir {
        Some(s_dir) => path
            .strip_prefix(s_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string(),
        None => path.to_string_lossy().to_string(),
    };

    let empty_routing = HashMap::new();
    let r_dict = routing.unwrap_or(&empty_routing);
    let routed = r_dict.get(&rel_s).map(std::string::String::as_str);

    let empty_outcomes = HashMap::new();
    let outcomes_ref = rust_outcomes.unwrap_or(&empty_outcomes);
    let audit_hits = lookup_rust_outcomes_for_rel(outcomes_ref, &rel_s, source_dir);

    let img_rust = audit_hits
        .iter()
        .rev()
        .find(|r| r.get("pipeline").map(std::string::String::as_str) == Some("img"));
    let vid_rust = audit_hits
        .iter()
        .rev()
        .find(|r| r.get("pipeline").map(std::string::String::as_str) == Some("vid"));
    let rust = img_rust.or(vid_rust);

    let reason_full = rust
        .and_then(|r| r.get("reason").map(std::string::String::as_str))
        .unwrap_or("");
    let reason = if reason_full.len() > 200 {
        &reason_full[..200]
    } else {
        reason_full
    };

    if let Some(vr) = vid_rust {
        let outcome = vr.get("outcome").map_or("", std::string::String::as_str);
        if outcome == "failed" || outcome == "skipped" {
            let vr_reason_full = vr.get("reason").map_or("", std::string::String::as_str);
            let vr_reason = if vr_reason_full.len() > 200 {
                &vr_reason_full[..200]
            } else {
                vr_reason_full
            };
            return Ok((
                "vid_pipeline_failed".to_string(),
                format!(
                    "vid pipeline {}{}",
                    outcome,
                    if vr_reason.is_empty() {
                        String::new()
                    } else {
                        format!(": {vr_reason}")
                    }
                ),
            ));
        }
    }

    if let Some(r) = rust
        && r.get("pipeline").map(std::string::String::as_str) == Some("vid")
    {
        let outcome = r.get("outcome").map_or("", std::string::String::as_str);
        if outcome == "failed" || outcome == "skipped" {
            return Ok((
                "vid_pipeline_failed".to_string(),
                format!(
                    "vid pipeline {}{}",
                    outcome,
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(": {reason}")
                    }
                ),
            ));
        }
        if outcome == "ignored" {
            let r_map = r.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            if !is_vid_expected_handoff(&r_map) {
                return Ok((
                    "vid_pipeline_failed".to_string(),
                    format!(
                        "vid ignored without expected handoff classification{}",
                        if reason.is_empty() {
                            String::new()
                        } else {
                            format!(": {reason}")
                        }
                    ),
                ));
            }
            if is_vid_expected_handoff(&r_map) {
                if processing_mode == "videos_only" {
                    return Ok((
                        "pipeline_handoff".to_string(),
                        "vid ignored static/single-frame asset — videos_only does not require \
                         optimized output for this file"
                            .to_string(),
                    ));
                }
                if processing_mode == "both" && owner.as_deref() == Some(PIPELINE_IMAGE) {
                    return Ok((
                        "pipeline_handoff".to_string(),
                        "vid ignored static asset — both mode expects img pipeline output (check \
                         img batch / rsync for this path)"
                            .to_string(),
                    ));
                }
                if processing_mode == "both" {
                    return Ok((
                        "pipeline_handoff".to_string(),
                        format!(
                            "vid ignored as static ({}); not an animated encode failure",
                            if reason.is_empty() {
                                "single-frame"
                            } else {
                                reason
                            }
                        ),
                    ));
                }
            }
        }
    }

    if let Some(r) = rust
        && r.get("pipeline").map(std::string::String::as_str) == Some("img")
    {
        let outcome = r.get("outcome").map_or("", std::string::String::as_str);
        if outcome == "failed" || outcome == "skipped" {
            return Ok((
                "true_missing".to_string(),
                format!(
                    "img pipeline {}{}",
                    outcome,
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(": {reason}")
                    }
                ),
            ));
        }
        if outcome == "ignored" {
            let r_map: HashMap<String, String> =
                r.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let img_r_map = img_rust.map_or_else(HashMap::new, |ir| {
                ir.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
            });
            if is_img_animation_ambiguity(&img_r_map) || is_img_animation_ambiguity(&r_map) {
                return Ok((
                    "pipeline_handoff".to_string(),
                    format!(
                        "img could not confirm static-only (AVIF/HEIC/JXL ambiguity) — no \
                         optimized output until re-probed or manual review{}",
                        if reason.is_empty() {
                            String::new()
                        } else {
                            format!(": {reason}")
                        }
                    ),
                ));
            }
            if is_img_animated_handoff(&img_r_map) || is_img_animated_handoff(&r_map) {
                if processing_mode == "both"
                    && (anim.is_some()
                        || routed == Some(PIPELINE_VIDEO)
                        || owner.as_deref() == Some(PIPELINE_VIDEO))
                {
                    return Ok((
                        "pipeline_handoff".to_string(),
                        "img ignored animated asset — both mode expects vid encode output"
                            .to_string(),
                    ));
                }
                if processing_mode == "videos_only"
                    && (anim.is_some()
                        || routed == Some(PIPELINE_VIDEO)
                        || owner.as_deref() == Some(PIPELINE_VIDEO))
                {
                    return Ok((
                        "pipeline_handoff".to_string(),
                        "img ignored animated asset — videos_only expects vid encode".to_string(),
                    ));
                }
            }
            return Ok((
                "true_missing".to_string(),
                format!(
                    "img ignored without classified handoff — investigate img batch logs{}",
                    if reason.is_empty() {
                        String::new()
                    } else {
                        format!(": {reason}")
                    }
                ),
            ));
        }
    }

    if processing_mode == "images_only"
        && (owner.as_deref() == Some(PIPELINE_VIDEO) || anim.is_some())
    {
        let detail = format!(
            "{}: not in images_only verify scope (owner={}); if present in tree, run vid or use \
             both mode",
            anim.as_deref().unwrap_or("video-scoped asset"),
            owner.as_deref().unwrap_or("video")
        );
        return Ok(("pipeline_handoff".to_string(), detail));
    }

    if processing_mode == "videos_only" && owner.as_deref() == Some(PIPELINE_IMAGE) {
        return Ok((
            "pipeline_handoff".to_string(),
            format!(
                "{true_format} is static image-owned — excluded from videos_only scope \
                 (matches_processing_mode); no optimized counterpart required"
            ),
        ));
    }

    if processing_mode == "both"
        && (owner.as_deref() == Some(PIPELINE_VIDEO)
            || routed == Some(PIPELINE_VIDEO)
            || anim.is_some())
    {
        if let Some(r) = rust {
            let outcome = r.get("outcome").map_or("", std::string::String::as_str);
            if outcome == "failed" || outcome == "skipped" {
                return Ok((
                    "vid_pipeline_failed".to_string(),
                    format!(
                        "vid pipeline {}{}",
                        outcome,
                        if reason.is_empty() {
                            String::new()
                        } else {
                            format!(": {reason}")
                        }
                    ),
                ));
            }
        }
        if rust.is_none() {
            let routed_note = routed.unwrap_or_else(|| owner.as_deref().unwrap_or("video"));
            return Ok((
                "vid_pipeline_unverified".to_string(),
                format!(
                    "{}: missing mfb::audit for pipeline={}; re-run verify with --session-audit \
                     and bundle run logs",
                    anim.as_deref().unwrap_or("video-route asset"),
                    routed_note
                ),
            ));
        }
        let routed_note = routed.unwrap_or_else(|| owner.as_deref().unwrap_or("video"));
        return Ok((
            "vid_pipeline_failed".to_string(),
            format!(
                "{}: session routed pipeline={}; both mode expects vid encode output — no \
                 optimized counterpart",
                anim.as_deref().unwrap_or("video-route asset"),
                routed_note
            ),
        ));
    }

    if processing_mode == "videos_only"
        && (owner.as_deref() == Some(PIPELINE_VIDEO)
            || routed == Some(PIPELINE_VIDEO)
            || anim.is_some())
    {
        if let Some(r) = rust {
            let outcome = r.get("outcome").map_or("", std::string::String::as_str);
            if outcome == "failed" || outcome == "skipped" {
                return Ok((
                    "vid_pipeline_failed".to_string(),
                    format!(
                        "vid pipeline {}{}",
                        outcome,
                        if reason.is_empty() {
                            String::new()
                        } else {
                            format!(": {reason}")
                        }
                    ),
                ));
            }
        }
        if rust.is_none() {
            let routed_note = routed.unwrap_or_else(|| owner.as_deref().unwrap_or("video"));
            return Ok((
                "vid_pipeline_unverified".to_string(),
                format!(
                    "{}: missing mfb::audit (routed={}); re-run verify with session/bundle logs",
                    anim.as_deref().unwrap_or("video-route asset"),
                    routed_note
                ),
            ));
        }
        let routed_note = routed.unwrap_or_else(|| owner.as_deref().unwrap_or("video"));
        return Ok((
            "vid_pipeline_failed".to_string(),
            format!(
                "{}: videos_only expects vid encode (routed={}) — no optimized counterpart",
                anim.as_deref().unwrap_or("video-route asset"),
                routed_note
            ),
        ));
    }

    Ok((
        "true_missing".to_string(),
        "no optimized counterpart (static image pipeline expected output)".to_string(),
    ))
}

pub fn load_session_pipeline_exits(
    audit_paths: &[PathBuf],
) -> Result<Vec<HashMap<String, String>>> {
    let mut exits = Vec::new();
    for path in audit_paths {
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(path)?;
        for line in contents.lines() {
            let line_lower = line.to_lowercase();
            if line_lower.contains("img_pipeline_exit") || line_lower.contains("vid_pipeline_exit")
            {
                let pipeline = if line_lower.contains("img_pipeline_exit") {
                    "img"
                } else {
                    "vid"
                };
                let mut map = HashMap::new();
                map.insert("pipeline".to_string(), pipeline.to_string());
                for part in line.split_whitespace() {
                    if let Some(idx) = part.find('=') {
                        let key = &part[..idx];
                        let val = &part[idx + 1..];
                        map.insert(key.to_string(), val.to_string());
                    }
                }
                exits.push(map);
            }
        }
    }
    Ok(exits)
}

pub fn session_handoff_preserve_was_declined(audit_paths: &[PathBuf]) -> Result<bool> {
    for path in audit_paths {
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(path)?;
        for line in contents.lines() {
            if line.to_lowercase().contains("handoff_preserve_declined") {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn load_session_preserve_handoff(audit_paths: &[PathBuf]) -> Result<HashSet<String>> {
    let mut preserved = HashSet::new();
    for path in audit_paths {
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(path)?;
        for line in contents.lines() {
            if line.to_lowercase().contains("preserve_handoff") {
                for part in line.split_whitespace() {
                    if part.starts_with("path=") {
                        let path_val = part.split('=').nth(1).unwrap_or("").trim();
                        preserved.insert(path_val.to_string());
                    }
                }
            }
        }
    }
    Ok(preserved)
}

pub fn scan_directory_routing(root: &Path) -> Result<(HashSet<String>, HashSet<String>)> {
    let mut image_paths = HashSet::new();
    let mut video_paths = HashSet::new();
    let root_resolved = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for entry in walkdir::WalkDir::new(&root_resolved) {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && let Some(fname) = path.file_name().and_then(|f| f.to_str())
        {
            if fname.starts_with('.') {
                continue;
            }
            if let Some(owner) = classify_media_owner(path)? {
                match path.strip_prefix(&root_resolved) {
                    Ok(rel) => {
                        let rel_str = rel.to_string_lossy().to_string();
                        if owner == PIPELINE_IMAGE {
                            image_paths.insert(rel_str);
                        } else {
                            video_paths.insert(rel_str);
                        }
                    }
                    Err(_err) => {}
                }
            }
        }
    }
    Ok((image_paths, video_paths))
}

pub fn is_video_like(path: &Path) -> Result<bool> {
    Ok(classify_media_owner(path)?.as_deref() == Some(PIPELINE_VIDEO))
}

pub fn load_session_routing(audit_paths: &[PathBuf]) -> Result<HashMap<String, String>> {
    let mut routing = HashMap::new();
    for path in audit_paths {
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(path)?;
        for line in contents.lines() {
            if line.contains("ROUTED") {
                let mut pipeline = String::new();
                let mut path_val = String::new();
                for part in line.split_whitespace() {
                    if part.starts_with("pipeline=") {
                        pipeline = part.split('=').nth(1).unwrap_or("").trim().to_lowercase();
                    } else if part.starts_with("path=") {
                        path_val = part.split('=').nth(1).unwrap_or("").trim().to_string();
                    }
                }
                if !path_val.is_empty() && !pipeline.is_empty() {
                    routing.insert(path_val, pipeline);
                }
            }
        }
    }
    Ok(routing)
}

pub fn load_rust_outcomes_from_logs(
    log_paths: &[PathBuf],
    source_root: Option<&Path>,
) -> Result<HashMap<String, Vec<HashMap<String, String>>>> {
    let mut by_path = HashMap::new();
    let source_abs = match source_root {
        Some(r) => match r.canonicalize() {
            Ok(p) => p.to_str().map(String::from),
            Err(_err) => None,
        },
        None => None,
    };

    let mut add =
        |path_str: &str, outcome: &str, pipeline: &str, reason: &str, ignore_class: &str| {
            let p_str = path_str.trim();
            if let Some(ref s_abs) = source_abs {
                match Path::new(p_str).canonicalize() {
                    Ok(res_path) => {
                        if !res_path.to_string_lossy().starts_with(s_abs) {
                            return;
                        }
                    }
                    Err(_err) => {}
                }
            }
            let mut record = HashMap::new();
            record.insert("outcome".to_string(), outcome.to_string());
            record.insert("pipeline".to_string(), pipeline.to_string());
            record.insert("reason".to_string(), reason.to_string());
            if !ignore_class.is_empty() {
                record.insert("ignore_class".to_string(), ignore_class.to_string());
            }
            by_path
                .entry(p_str.to_string())
                .or_insert_with(Vec::new)
                .push(record);
        };

    for log_path in log_paths {
        if !log_path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(log_path)?;
        for line in contents.lines() {
            let stripped = line.trim();
            if stripped.contains(MFB_AUDIT_PREFIX) || stripped.contains("mfb::audit") {
                if stripped.contains("outcome=batch_complete") {
                    continue;
                }
                // Parse key values from structured log
                let mut kvs = HashMap::new();
                for part in stripped.split_whitespace() {
                    if let Some(idx) = part.find('=') {
                        let k = &part[..idx];
                        let v = part[idx + 1..].trim_matches('"');
                        kvs.insert(k, v);
                    }
                }
                if let Some(outcome) = kvs.get("outcome") {
                    if *outcome == "batch_complete" {
                        continue;
                    }
                    if let Some(path_raw) = kvs.get("path") {
                        let pipeline = kvs.get("pipeline").copied().unwrap_or_else(|| {
                            if stripped.contains("image_processing") {
                                "img"
                            } else {
                                "vid"
                            }
                        });
                        let reason = kvs.get("reason").copied().unwrap_or("");
                        let ignore_class = kvs.get("ignore_class").copied().unwrap_or("");
                        add(path_raw, outcome, pipeline, reason, ignore_class);
                        continue;
                    }
                }
            }
            // Fallback to legacy ignore/skip logs
            if stripped.contains("[IGNORE]")
                && let Some(idx) = stripped.find("[IGNORE]")
            {
                let rest = &stripped[idx + 8..];
                if let Some(dash_idx) = rest.find("—") {
                    let path_val = &rest[..dash_idx].trim();
                    let reason = &rest[dash_idx + 1..].trim();
                    add(path_val, "ignored", "img", reason, "");
                }
            }
            if stripped.contains("[SKIP]")
                && let Some(idx) = stripped.find("[SKIP]")
            {
                let rest = &stripped[idx + 6..];
                if let Some(dash_idx) = rest.find("—") {
                    let path_val = &rest[..dash_idx].trim();
                    let reason = &rest[dash_idx + 1..].trim();
                    add(path_val, "skipped", "img", reason, "");
                }
            }
        }
    }
    Ok(by_path)
}

pub fn reconcile_handoff(
    handoff_entries: &[(String, PathBuf, String)],
    routing: &HashMap<String, String>,
    rust_outcomes: &HashMap<String, Vec<HashMap<String, String>>>,
    source_dir: &Path,
    optimized_dir: Option<&Path>,
    preserved_rel_paths: Option<&HashSet<String>>,
    preserve_declined: bool,
    pipeline_exits: Option<&[HashMap<String, String>]>,
) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    if let Some(exits) = pipeline_exits {
        lines.push("  Session pipeline exits:".to_string());
        for ex in exits {
            lines.push(format!(
                "    {}: code={} ok={} skip={} ignore={} fail={}",
                ex.get("pipeline").map_or("?", String::as_str),
                ex.get("code").map_or("0", String::as_str),
                ex.get("succeeded").map_or("0", String::as_str),
                ex.get("skipped").map_or("0", String::as_str),
                ex.get("ignored").map_or("0", String::as_str),
                ex.get("failed").map_or("0", String::as_str)
            ));
        }
        lines.push(String::new());
    }

    for (_key, src_path, note) in handoff_entries {
        let rel = src_path.strip_prefix(source_dir).unwrap_or(src_path);
        let rel_s = rel.to_string_lossy().to_string();
        let owner = match classify_media_owner(src_path) {
            Ok(Some(o)) => o,
            Ok(None) => "none".to_string(),
            Err(e) => return Err(e),
        };
        let routed = routing.get(&rel_s).map(std::string::String::as_str);

        lines.push(format!("  ▶ {rel_s}"));
        lines.push(format!("      scope note: {note}"));
        lines.push(format!(
            "      Python classify_media_owner: {} | session ROUTED: {}",
            owner,
            routed.unwrap_or("(not in session audit)")
        ));

        let mut rust_hits = Vec::new();
        for (log_path, records) in rust_outcomes {
            if log_path.ends_with(&rel_s) || log_path.contains(&rel_s) {
                rust_hits.extend(records.clone());
            }
        }

        if rust_hits.is_empty() {
            lines.push(
                "      Rust log: (no img/vid outcome line for this path — check bundle img_run / \
                 vid_run or run was before structured audit)"
                    .to_string(),
            );
        } else {
            for rec in rust_hits.iter().take(3) {
                let reason = rec.get("reason").map_or("", std::string::String::as_str);
                let reason_trunc = if reason.len() > 120 {
                    &reason[..120]
                } else {
                    reason
                };
                lines.push(format!(
                    "      Rust log: outcome={} pipeline={} — {}",
                    rec.get("outcome").map_or("", String::as_str),
                    rec.get("pipeline").map_or("?", String::as_str),
                    reason_trunc
                ));
            }
        }

        if routed == Some(PIPELINE_VIDEO) && owner == PIPELINE_VIDEO {
            let last_outcome = rust_hits.last();
            if let Some(rec) = last_outcome {
                let outcome = rec.get("outcome").map(std::string::String::as_str);
                if outcome == Some("ignored") && is_vid_static_ignore(rec) {
                    lines.push(
                        "      ✓ vid ignored static/single-frame — expected gap in videos_only; \
                         not a encode failure"
                            .to_string(),
                    );
                } else if outcome == Some("failed") || outcome == Some("skipped") {
                    lines.push(
                        "      ✗ vid pipeline failed/skipped — both mode expects encode; this \
                         is a real integrity gap (not img handoff ignore)"
                            .to_string(),
                    );
                } else {
                    lines.push(
                        "      ⚠ video-route asset missing optimized output; check vid batch logs \
                         (both mode expects vid encode, not img ignore)"
                            .to_string(),
                    );
                }
            } else {
                lines.push(
                    "      ⚠ video-route asset missing optimized output; check vid batch logs \
                     (both mode expects vid encode, not img ignore)"
                        .to_string(),
                );
            }
        } else if routed == Some(PIPELINE_IMAGE) && owner == PIPELINE_VIDEO {
            lines.push(
                "      ⚠ Layer mismatch: file is animated but session routed as image".to_string(),
            );
        } else if routed.is_none() {
            lines.push(
                "      ⚠ Session routing log missing this path (re-run with current drag-and-drop)"
                    .to_string(),
            );
        }

        if preserve_declined {
            lines.push(
                "      Session audit: user declined HANDOFF_PRESERVE (no copies performed)"
                    .to_string(),
            );
        } else if let Some(preserved) = preserved_rel_paths
            && preserved.contains(&rel_s)
        {
            lines.push("      Session audit: PRESERVE_HANDOFF ran for this path".to_string());
        }

        if let Some(opt_dir) = optimized_dir {
            let exact = opt_dir.join(rel);
            if exact.is_file() {
                lines.push(format!(
                    "      Optimized tree: {} present ({} bytes)",
                    rel_s,
                    exact.metadata()?.len()
                ));
            } else {
                let was_preserved = preserved_rel_paths.is_some_and(|p| p.contains(&rel_s));
                if !was_preserved {
                    lines.push(
                        "      Optimized tree: still missing — re-run batch or call \
                         finalize_handoff_preservation()"
                            .to_string(),
                    );
                }
            }
        }
        lines.push(String::new());
    }

    Ok(lines)
}

#[must_use]
pub fn is_vid_static_ignore(rust: &HashMap<String, String>) -> bool {
    if let Some(ignore_class) = rust.get("ignore_class")
        && VID_STATIC_IGNORE_CLASSES.contains(&ignore_class.as_str())
    {
        return true;
    }
    let reason = rust
        .get("reason")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    reason.contains("static image detected (1 frame)")
        || reason.contains("vid ignores static media")
        || reason.contains("vid ignores potentially non-animated")
        || reason.contains("single-frame")
        || reason.contains("non-animated")
}

#[must_use]
pub fn is_vid_expected_handoff(rust: &HashMap<String, String>) -> bool {
    if rust.get("pipeline").map(std::string::String::as_str) != Some("vid")
        || rust.get("outcome").map(std::string::String::as_str) != Some("ignored")
    {
        return false;
    }
    if let Some(ignore_class) = rust.get("ignore_class") {
        let trimmed = ignore_class.trim();
        if VID_HANDOFF_IGNORE_CLASSES.contains(&trimmed)
            || VID_STATIC_IGNORE_CLASSES.contains(&trimmed)
        {
            return true;
        }
    }
    if is_vid_static_ignore(rust) {
        return true;
    }
    let reason = rust
        .get("reason")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    reason.contains("outside video domain") || reason.contains("outside this tool domain")
}

#[must_use]
pub fn is_img_animation_ambiguity(rust: &HashMap<String, String>) -> bool {
    if rust.get("pipeline").map(std::string::String::as_str) != Some("img")
        || rust.get("outcome").map(std::string::String::as_str) != Some("ignored")
    {
        return false;
    }
    if let Some(ignore_class) = rust.get("ignore_class") {
        return ignore_class.trim() == "img_animation_ambiguity";
    }
    false
}

#[must_use]
pub fn is_img_animated_handoff(rust: &HashMap<String, String>) -> bool {
    if rust.get("pipeline").map(std::string::String::as_str) != Some("img")
        || rust.get("outcome").map(std::string::String::as_str) != Some("ignored")
    {
        return false;
    }
    if let Some(ignore_class) = rust.get("ignore_class")
        && IMG_ANIMATED_HANDOFF_CLASSES.contains(&ignore_class.trim())
    {
        return true;
    }
    let reason = rust
        .get("reason")
        .map(|s| s.to_lowercase())
        .unwrap_or_default();
    reason.contains("img strictly processes static images only")
        || (reason.contains("animated media detected")
            && reason.contains("refusing static conversion"))
}

// ── Handoff preserve integration for drag_and_drop_processor ────────────────

pub fn preserve_handoff_gaps(
    source_root: &Path,
    optimized_root: &Path,
    video_rel_paths: &[String],
    only_candidates: Option<&[HandoffPreserveCandidate]>,
    phase: &str,
    mut audit_log: Option<&mut dyn FnMut(&str)>,
) -> Result<Vec<String>> {
    if phase != HANDOFF_PRESERVE_PHASE_POST_IMG_VID {
        if let Some(log) = audit_log.as_mut() {
            let mut fields = HashMap::new();
            fields.insert(
                "phase".to_string(),
                if phase.is_empty() {
                    "(empty)".to_string()
                } else {
                    phase.to_string()
                },
            );
            fields.insert(
                "required".to_string(),
                HANDOFF_PRESERVE_PHASE_POST_IMG_VID.to_string(),
            );
            log(&audit_handoff_blocked("invalid_phase", &fields));
        }
        return Ok(Vec::new());
    }

    let source_root = source_root
        .canonicalize()
        .unwrap_or_else(|_| source_root.to_path_buf());
    let optimized_root = optimized_root
        .canonicalize()
        .unwrap_or_else(|_| optimized_root.to_path_buf());
    if !source_root.is_dir() || !optimized_root.is_dir() {
        if let Some(log) = audit_log.as_mut() {
            let fields = HashMap::from([(
                "reason".to_string(),
                "missing_source_or_optimized_dir".to_string(),
            )]);
            log(&format_audit_event("HANDOFF_PRESERVE_ABORT", &fields));
        }
        return Ok(Vec::new());
    }

    let rels_to_copy: Vec<String> = if let Some(candidates) = only_candidates {
        candidates.iter().map(|c| c.rel_path.clone()).collect()
    } else {
        list_handoff_preserve_candidates(&source_root, &optimized_root, video_rel_paths)?
            .into_iter()
            .map(|c| c.rel_path)
            .collect()
    };

    let mut preserved = Vec::new();
    for rel_s in rels_to_copy {
        let rel = Path::new(&rel_s);
        let src = source_root.join(rel);
        let dst = optimized_root.join(rel);
        if !src.is_file() {
            continue;
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create handoff preserve parent {}", parent.display()))?;
        }
        fs::copy(&src, &dst).with_context(|| {
            format!(
                "copy handoff preserve {} -> {}",
                src.display(),
                dst.display()
            )
        })?;
        if let Some(log) = audit_log.as_mut() {
            log(&format_session_audit_preserve_handoff(&rel_s));
        }
        preserved.push(rel.to_string_lossy().replace('\\', "/"));
    }

    Ok(preserved)
}

/// Scan session audit logs for routed video paths after img/vid processing.
pub fn extract_routed_video_paths_from_audit(session_audit: &Path) -> Result<Vec<String>> {
    if !session_audit.is_file() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(session_audit)?;
    let mut routed = Vec::new();
    for line in content.lines() {
        if line.contains("ROUTED")
            && line.contains("pipeline=vid")
            && let Some(idx) = line.find("path=")
        {
            let rest = &line[idx + 5..];
            if let Some(space_idx) = rest.find(|c: char| c.is_whitespace() || c == '"') {
                routed.push(rest[..space_idx].to_string());
            } else {
                routed.push(rest.to_string());
            }
        }
    }
    routed.sort();
    routed.dedup();
    Ok(routed)
}

/// Report handoff preserve candidates from routed video paths.
pub fn report_handoff_preserve_gaps_from_paths(
    source_root: &Path,
    optimized_root: &Path,
    routed_video_paths: &[String],
) -> Result<Vec<HandoffPreserveCandidate>> {
    list_handoff_preserve_candidates(source_root, optimized_root, routed_video_paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    fn test_parse_jxlinfo_animation_hint() {
        assert_eq!(
            parse_jxlinfo_animation_hint("have_animation: 1"),
            Some(true)
        );
        assert_eq!(
            parse_jxlinfo_animation_hint("have_animation: 0"),
            Some(false)
        );
        assert_eq!(
            parse_jxlinfo_animation_hint("animation length: 10"),
            Some(true)
        );
        assert_eq!(
            parse_jxlinfo_animation_hint("animation length: 0"),
            Some(false)
        );
        assert_eq!(parse_jxlinfo_animation_hint("jpeg xl image"), Some(false));
        assert_eq!(parse_jxlinfo_animation_hint("random text"), None);
    }

    #[test]
    fn test_detect_true_format_unknown() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("unknown.xyz");
        fs::write(&path, b"some random data of unknown format").unwrap();
        assert_eq!(detect_true_format(&path).unwrap(), "unknown");
    }

    #[test]
    fn test_is_animated_webp_malformed() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test.webp");
        fs::write(&path, b"not_a_webp_at_all_header_data_etc").unwrap();
        assert!(is_animated_webp(&path).is_err());
    }

    #[test]
    fn test_is_animated_png_malformed() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test.png");
        fs::write(&path, b"not_a_png_header").unwrap();
        assert!(is_animated_png(&path).is_err());
    }

    #[test]
    fn test_is_img_classification_helpers() {
        let mut rust = HashMap::new();
        rust.insert("pipeline".to_string(), "img".to_string());
        rust.insert("outcome".to_string(), "ignored".to_string());
        rust.insert(
            "ignore_class".to_string(),
            "img_animation_ambiguity".to_string(),
        );

        assert!(is_img_animation_ambiguity(&rust));
        assert!(!is_img_animated_handoff(&rust));

        rust.insert(
            "ignore_class".to_string(),
            "img_animated_handoff".to_string(),
        );
        assert!(!is_img_animation_ambiguity(&rust));
        assert!(is_img_animated_handoff(&rust));
    }

    #[test]
    fn test_media_scope_routes_disguised_animated_formats_by_content() {
        let tempdir = tempfile::tempdir().unwrap();
        let webp = tempdir.path().join("animated-webp.bin");
        let apng = tempdir.path().join("animated-png.bin");
        let still_jpeg = tempdir.path().join("still-jpeg.bin");
        let fake_jpg = tempdir.path().join("fake.jpg");

        fs::write(&webp, b"RIFF\x18\x00\x00\x00WEBPVP8XANIM").unwrap();
        fs::write(
            &apng,
            b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x08acTL",
        )
        .unwrap();
        fs::write(&still_jpeg, b"\xff\xd8\xff\xe0jpeg").unwrap();
        fs::write(&fake_jpg, b"not media").unwrap();

        assert_eq!(
            classify_media_owner(&webp).unwrap(),
            Some(PIPELINE_VIDEO.to_string())
        );
        assert_eq!(
            classify_media_owner(&apng).unwrap(),
            Some(PIPELINE_VIDEO.to_string())
        );
        assert_eq!(
            classify_media_owner(&still_jpeg).unwrap(),
            Some(PIPELINE_IMAGE.to_string())
        );
        assert_eq!(classify_media_owner(&fake_jpg).unwrap(), None);
    }

    #[test]
    fn test_media_scope_classification_surfaces_missing_file_probe_errors() {
        let tempdir = tempfile::tempdir().unwrap();
        let missing = tempdir.path().join("missing.gif");
        let res = classify_media_owner(&missing);
        assert!(res.is_err());
        let err_str = res.err().unwrap().to_string();
        assert!(
            err_str.contains(&missing.to_string_lossy().to_string())
                || err_str.contains("No such file or directory")
        );
    }

    #[test]
    fn test_media_scope_rejects_malformed_animated_containers() {
        let tempdir = tempfile::tempdir().unwrap();
        let malformed_gif = tempdir.path().join("truncated-gif.bin");
        let malformed_apng = tempdir.path().join("truncated-apng.bin");

        fs::write(&malformed_gif, b"GIF89a\x01\x00").unwrap();
        fs::write(&malformed_apng, b"\x89PNG\r\n\x1a\n").unwrap();

        for path in &[malformed_gif, malformed_apng] {
            let res = classify_media_owner(path);
            assert!(res.is_err());
            let err_str = res.err().unwrap().to_string();
            assert!(
                err_str.contains(&path.to_string_lossy().to_string())
                    || err_str.contains("GIF animation probe failed")
                    || err_str.contains("PNG animation probe failed")
            );
        }
    }
}

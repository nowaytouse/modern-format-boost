use crate::infra::hardening::file_name_display;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const JUNK_EXTS: &[&str] = &[
    "ds_store", "xmp", "txt", "md", "json", "ini", "db", "lnk", "bak", "tmp",
];

const JUNK_PATH_TOKENS: &[&str] = &["backup", "tmp", "old", "redundant"];

fn path_extension_lower(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext.to_lowercase(),
        None => String::new(),
    }
}

pub fn is_junk_path(path: &Path) -> bool {
    let name = file_name_display(path).to_lowercase();
    if name.starts_with('.') || name == "thumbs.db" {
        return true;
    }
    if JUNK_EXTS.contains(&path_extension_lower(path).as_str()) {
        return true;
    }
    let path_lower = path.to_string_lossy().to_lowercase();
    JUNK_PATH_TOKENS
        .iter()
        .any(|token| path_lower.contains(token))
}

pub fn iter_media_files(root: &Path) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(e) => Some(e),
            Err(err) => {
                eprintln!(
                    "[SCANNER] walkdir entry failed under {}: {err}",
                    root.display()
                );
                None
            }
        })
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| !is_junk_path(p))
}

/// Evaluate file_quality_filter rules from training_rules.json.
/// Mirrors py `passes_file_quality_filter` + `evaluate_file_quality_rule`.
pub fn passes_file_quality_filter(
    path: &Path,
    filter: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    let filter = match filter {
        Some(f) => f,
        None => return true,
    };
    let logic = filter
        .get("logic")
        .and_then(|v| v.as_str())
        .unwrap_or("ALL")
        .to_uppercase();
    let rules = match filter.get("rules").and_then(|v| v.as_array()) {
        Some(r) => r,
        None => return true,
    };
    if rules.is_empty() {
        return true;
    }

    let mut verdicts = Vec::with_capacity(rules.len());
    for raw_rule in rules {
        let obj = match raw_rule.as_object() {
            Some(o) => o,
            None => continue,
        };
        let rule_name = obj
            .get("rule")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let verdict = evaluate_file_quality_rule(path, &rule_name, obj);
        verdicts.push(verdict);
    }

    if verdicts.is_empty() {
        return true;
    }
    if logic == "ANY" {
        verdicts.iter().any(|&v| v)
    } else {
        verdicts.iter().all(|&v| v)
    }
}

fn metadata_size_kb(path: &Path) -> Option<f64> {
    match std::fs::metadata(path) {
        Ok(m) => Some(m.len() as f64 / 1024.0),
        Err(err) => {
            eprintln!("[SCANNER] metadata failed ({}): {err}", path.display());
            None
        }
    }
}

fn evaluate_file_quality_rule(
    path: &Path,
    rule_name: &str,
    obj: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    match rule_name {
        "is_supported_image_file" => {
            const IMG_EXTS: &[&str] = &[
                "jpg", "jpeg", "png", "tiff", "tif", "bmp", "webp", "avif", "heic", "heif", "hif",
                "jxl", "gif", "apng",
            ];
            IMG_EXTS.contains(&path_extension_lower(path).as_str())
        }
        "is_supported_animated_image_file" => {
            const ANIM_EXTS: &[&str] = &["gif", "webp", "apng", "avif", "heic", "heif", "jxl"];
            ANIM_EXTS.contains(&path_extension_lower(path).as_str())
        }
        "is_supported_non_loop_media_file" | "is_supported_loop_intent_media_file" => {
            const MEDIA_EXTS: &[&str] = &[
                "gif", "webp", "apng", "avif", "heic", "heif", "jxl", "mp4", "mov", "webm", "mkv",
                "avi",
            ];
            MEDIA_EXTS.contains(&path_extension_lower(path).as_str())
        }
        "file_size_kb_ge" => {
            let min_kb = match obj.get("value").and_then(|v| v.as_f64()) {
                Some(v) if v.is_finite() => v,
                _ => {
                    eprintln!("[SCANNER] file_size_kb_ge missing numeric value");
                    0.0
                }
            };
            match metadata_size_kb(path) {
                Some(size_kb) => size_kb >= min_kb,
                None => false,
            }
        }
        "file_size_kb_le" => {
            let max_kb = match obj.get("value").and_then(|v| v.as_f64()) {
                Some(v) if v.is_finite() => v,
                _ => f64::MAX,
            };
            match metadata_size_kb(path) {
                Some(size_kb) => size_kb <= max_kb,
                None => false,
            }
        }
        "extension_not_in" => {
            let blocked: Vec<String> = match obj.get("value").and_then(|v| v.as_array()) {
                Some(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.trim_start_matches('.').to_lowercase())
                    .collect(),
                None => Vec::new(),
            };
            if blocked.is_empty() {
                return true;
            }
            !blocked.contains(&path_extension_lower(path))
        }
        "path_not_contains_any" => {
            let tokens: Vec<String> = match obj.get("value").and_then(|v| v.as_array()) {
                Some(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_lowercase())
                    .collect(),
                None => Vec::new(),
            };
            if tokens.is_empty() {
                return true;
            }
            let path_str = path.to_string_lossy().to_lowercase();
            !tokens.iter().any(|t| path_str.contains(t.as_str()))
        }
        "filename_not_matches_regex" => {
            let pattern = obj.get("value").and_then(|v| v.as_str()).unwrap_or("");
            if pattern.is_empty() {
                return true;
            }
            let filename = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => {
                    eprintln!("[SCANNER] filename_not_matches_regex missing file_name");
                    return true;
                }
            };
            !filename.to_lowercase().contains(&pattern.to_lowercase())
        }
        _ => true,
    }
}

use crate::infra::hardening::positive_usize_env;
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

const ENV_SEGMENT_FILE_THRESHOLD: &str = "MFB_TRAINING_SEGMENT_FILE_THRESHOLD";
const ENV_SEGMENT_SUBDIR_BATCH: &str = "MFB_TRAINING_SEGMENT_SUBDIR_BATCH";

const DEFAULT_SEGMENT_FILE_THRESHOLD: usize = 20_000;
const DEFAULT_SEGMENT_SUBDIR_BATCH: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    Oneshot,
    Segmented,
}

impl ScanMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ScanMode::Oneshot => "oneshot",
            ScanMode::Segmented => "segmented",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanSegment {
    pub mode: ScanMode,
    pub roots: Vec<PathBuf>,
    pub label: String,
    pub index: usize,
    pub total: usize,
}

impl ScanSegment {
    pub fn display_root(&self) -> &Path {
        &self.roots[0]
    }
}

fn positive_int_env(name: &str, default: usize) -> usize {
    positive_usize_env(name, default)
}

pub fn segment_file_threshold() -> usize {
    positive_int_env(ENV_SEGMENT_FILE_THRESHOLD, DEFAULT_SEGMENT_FILE_THRESHOLD)
}

pub fn segment_subdir_batch() -> usize {
    positive_int_env(ENV_SEGMENT_SUBDIR_BATCH, DEFAULT_SEGMENT_SUBDIR_BATCH)
}

pub fn estimate_media_file_count<F>(
    root: &Path,
    is_media_file: F,
    cap: usize,
) -> Result<Option<usize>>
where
    F: Fn(&Path) -> bool,
{
    let mut count = 0;
    let mut stack = vec![root.to_path_buf()];

    while let Some(current) = stack.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(e) => e,
            Err(err) => bail!("media count scan failed for {}: {}", current.display(), err),
        };

        let mut subdirs = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(err) => bail!(
                    "media count entry probe failed under {}: {}",
                    current.display(),
                    err
                ),
            };

            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(err) => {
                    eprintln!("[SCAN] metadata probe failed ({}): {err}", path.display());
                    continue;
                }
            };

            if metadata.is_dir() {
                subdirs.push(path);
                continue;
            }

            if !metadata.is_file() {
                continue;
            }

            if is_media_file(&path) {
                count += 1;
                if count > cap {
                    return Ok(None);
                }
            }
        }

        // Push in reverse order so we visit in alphabetical (or standard) order if sorted
        // though `read_dir` order is arbitrary. The python script just appended them.
        for sd in subdirs.into_iter().rev() {
            stack.push(sd);
        }
    }

    Ok(Some(count))
}

fn top_level_subdirs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut children = Vec::new();
    let entries = fs::read_dir(root)
        .with_context(|| format!("top-level subdir scan failed for {}", root.display()))?;

    for entry in entries {
        let entry = entry.with_context(|| {
            format!(
                "top-level subdir entry probe failed under {}",
                root.display()
            )
        })?;
        let metadata = match entry.metadata() {
            Ok(m) => Some(m),
            Err(err) => {
                eprintln!(
                    "[SCAN] subdir metadata probe failed ({}): {err}",
                    entry.path().display()
                );
                None
            }
        };
        if let Some(m) = metadata
            && m.is_dir()
        {
            children.push(entry.path());
        }
    }

    children.sort_by(|a, b| {
        let a_name = a
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        let b_name = b
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        a_name.cmp(&b_name)
    });

    Ok(children)
}

fn segment_label_for_paths(paths: &[PathBuf]) -> String {
    if paths.len() == 1 {
        return paths[0]
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
    }

    let names: Vec<String> = paths
        .iter()
        .take(3)
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let mut combined = names.join("+");

    if paths.len() > 3 {
        combined = format!("{}+{}_more", combined, paths.len() - 3);
    }
    combined
}

pub fn plan_scan_segments<F>(root: &Path, is_media_file: F) -> Result<Vec<ScanSegment>>
where
    F: Fn(&Path) -> bool,
{
    let threshold = segment_file_threshold();
    if let Some(_count) = estimate_media_file_count(root, is_media_file, threshold)? {
        return Ok(vec![ScanSegment {
            mode: ScanMode::Oneshot,
            roots: vec![root.to_path_buf()],
            label: "full".to_string(),
            index: 1,
            total: 1,
        }]);
    }

    let subdirs = top_level_subdirs(root)?;
    if subdirs.is_empty() {
        return Ok(vec![ScanSegment {
            mode: ScanMode::Oneshot,
            roots: vec![root.to_path_buf()],
            label: "full".to_string(),
            index: 1,
            total: 1,
        }]);
    }

    let batch = segment_subdir_batch();
    let mut raw_segments = Vec::new();

    for chunk in subdirs.chunks(batch) {
        let chunk_vec = chunk.to_vec();
        raw_segments.push((segment_label_for_paths(&chunk_vec), chunk_vec));
    }

    let total = raw_segments.len();
    let segments = raw_segments
        .into_iter()
        .enumerate()
        .map(|(i, (label, roots))| ScanSegment {
            mode: ScanMode::Segmented,
            roots,
            label,
            index: i + 1,
            total,
        })
        .collect();

    Ok(segments)
}

pub fn format_scan_plan_summary(root: &Path, segments: &[ScanSegment]) -> String {
    if segments.is_empty() {
        return format!("path={} segments=0", root.display());
    }

    let mode = segments[0].mode.as_str();
    if segments.len() == 1 && segments[0].mode == ScanMode::Oneshot {
        return format!(
            "path={} mode={} files≤{}",
            root.display(),
            mode,
            segment_file_threshold()
        );
    }

    format!(
        "path={} mode={} segments={} threshold>{} files",
        root.display(),
        mode,
        segments.len(),
        segment_file_threshold()
    )
}

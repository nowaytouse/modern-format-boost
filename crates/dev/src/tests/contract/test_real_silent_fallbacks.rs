#![allow(clippy::too_many_lines)]

use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|err| panic!("failed to resolve workspace root: {err:?}")) // audited: contract test assertion path; panic/expect is test-only failure signal
}

fn read_hardening_doc(root: impl AsRef<Path>, name: &str) -> String {
    let root = root.as_ref();
    let direct = root.join("docs/hardening").join(name);
    if direct.is_file() {
        return fs::read_to_string(&direct)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", direct.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    }

    let ssot_path = root.join(".agents/harding/SSOT.md");
    let ssot = fs::read_to_string(&ssot_path)
        .unwrap_or_else(|err| panic!("read {}: {err:?}", ssot_path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let header = format!("# SOURCE: {name}");
    let start = ssot
        .find(&header)
        .unwrap_or_else(|| panic!("SSOT missing embedded section {header}")); // audited: contract test assertion path; panic/expect is test-only failure signal
    let after_header = start + header.len();
    let end = ssot[after_header..]
        .find("\n# SOURCE: ")
        .map_or(ssot.len(), |offset| after_header + offset);
    ssot[start..end].to_string()
}

fn map_legacy_rel(rel: &str) -> String {
    for (old, new) in [
        (
            "crates/dev/src/tests/comprehensive_weakness_audit.rs",
            "crates/dev/src/tests/contract/comprehensive_weakness_audit.rs",
        ),
        (
            "crates/dev/src/tests/numeric_cast_safety.rs",
            "crates/dev/src/tests/contract/numeric_cast_safety.rs",
        ),
        (
            "crates/dev/src/tests/test_real_silent_fallbacks.rs",
            "crates/dev/src/tests/contract/test_real_silent_fallbacks.rs",
        ),
        (
            "crates/dev/src/tests/test_silent_numeric_fallbacks.rs",
            "crates/dev/src/tests/contract/test_silent_numeric_fallbacks.rs",
        ),
        (
            "crates/dev/src/tests/classification_snapshots.rs",
            "crates/dev/src/tests/matrix/classification_snapshots.rs",
        ),
        (
            "crates/dev/src/tests/mode_matrix_tests.rs",
            "crates/dev/src/tests/matrix/mode_matrix_tests.rs",
        ),
        (
            "crates/dev/src/tests/parity_tests.rs",
            "crates/dev/src/tests/matrix/parity_tests.rs",
        ),
        (
            "crates/dev/src/tests/property_tests.rs",
            "crates/dev/src/tests/matrix/property_tests.rs",
        ),
        (
            "crates/dev/src/tests/snapshot_tests.rs",
            "crates/dev/src/tests/matrix/snapshot_tests.rs",
        ),
        (
            "crates/dev/src/tests/deny_animated_jxl.rs",
            "crates/dev/src/tests/media/deny_animated_jxl.rs",
        ),
        (
            "crates/dev/src/tests/headless_gif_regression.rs",
            "crates/dev/src/tests/media/headless_gif_regression.rs",
        ),
        (
            "crates/dev/src/tests/test_animated_frame_consistency.rs",
            "crates/dev/src/tests/media/test_animated_frame_consistency.rs",
        ),
        (
            "crates/dev/src/tests/test_cjxl_errors.rs",
            "crates/dev/src/tests/media/test_cjxl_errors.rs",
        ),
        (
            "crates/dev/src/tests/test_ultrahdr_hardening.rs",
            "crates/dev/src/tests/media/test_ultrahdr_hardening.rs",
        ),
        (
            "crates/dev/src/tests/test_webp_animated_classification.rs",
            "crates/dev/src/tests/media/test_webp_animated_classification.rs",
        ),
        (
            "crates/dev/src/tests/test_webp_duration_parser.rs",
            "crates/dev/src/tests/media/test_webp_duration_parser.rs",
        ),
        (
            "crates/dev/src/tests/vmaf_baseline_missing.rs",
            "crates/dev/src/tests/media/vmaf_baseline_missing.rs",
        ),
        (
            "crates/dev/src/tests/ctrlc_behavior.rs",
            "crates/dev/src/tests/runtime/ctrlc_behavior.rs",
        ),
        (
            "crates/dev/src/tests/ignored_semantics.rs",
            "crates/dev/src/tests/runtime/ignored_semantics.rs",
        ),
        (
            "crates/dev/src/tests/ignored_static.rs",
            "crates/dev/src/tests/runtime/ignored_static.rs",
        ),
        (
            "crates/dev/src/tests/path_safety.rs",
            "crates/dev/src/tests/runtime/path_safety.rs",
        ),
        (
            "crates/dev/src/tests/runtime_probe_regression.rs",
            "crates/dev/src/tests/runtime/runtime_probe_regression.rs",
        ),
    ] {
        if rel == old {
            return new.to_string();
        }
    }
    if rel == "docs/dev/decision_tree.md" {
        return "docs/dev/decision_tree_dev.md".to_string();
    }
    if rel == "docs/dev/JPEG XL& HEVC .md" {
        return "docs/dev/jpeg_xl_hevc_notes.md".to_string();
    }
    let prefix = "crates/foundation/src/";
    if !rel.starts_with(prefix) {
        return rel.to_string();
    }
    let tail = &rel[prefix.len()..];
    for (dir, names) in [
        (
            "infra",
            &[
                "app_error.rs",
                "common_utils.rs",
                "constants.rs",
                "ctrlc_guard.rs",
                "entry_guard.rs",
                "error_handler.rs",
                "flag_validator.rs",
                "float_compare.rs",
                "io_utils.rs",
                "logging.rs",
                "numeric_cast.rs",
                "path_safety.rs",
                "path_validator.rs",
                "performance_schedule.rs",
                "process_lock.rs",
                "safety.rs",
                "static_logs.rs",
                "system_memory.rs",
                "test_ci_contract.rs",
                "thread_manager.rs",
                "unified_error.rs",
                "version.rs",
            ][..],
        ),
        (
            "convert",
            &[
                "analysis_cache.rs",
                "batch.rs",
                "checkpoint.rs",
                "cli_runner.rs",
                "conversion.rs",
                "conversion_types.rs",
                "delivery_codec_strategy.rs",
                "explore_strategy.rs",
                "file_copier.rs",
                "file_sorter.rs",
                "lru_cache.rs",
                "media_conversion_gate.rs",
                "media_passthrough.rs",
                "media_penetration.rs",
                "media_precision.rs",
                "process_runner.rs",
                "pure_media_verifier.rs",
                "smart_file_copier.rs",
            ][..],
        ),
        (
            "image",
            &[
                "animated_image_quality_features.rs",
                "candidate_comparator.rs",
                "depth_channel.rs",
                "image_analyzer.rs",
                "image_builders.rs",
                "image_detection.rs",
                "image_formats.rs",
                "image_heic_analysis.rs",
                "image_jpeg_analysis.rs",
                "image_metrics.rs",
                "image_quality_db.rs",
                "image_quality_detector.rs",
                "jxl_builder.rs",
                "jxl_explorer.rs",
                "jxl_utils.rs",
                "live_photo.rs",
                "loop_intent.rs",
            ][..],
        ),
        (
            "video",
            &[
                "codecs.rs",
                "ffmpeg_builder.rs",
                "ffmpeg_process.rs",
                "ffprobe.rs",
                "ffprobe_json.rs",
                "gpu_accel.rs",
                "msssim_parallel.rs",
                "msssim_progress.rs",
                "msssim_sampling.rs",
                "stream_size.rs",
                "video.rs",
                "video_detection.rs",
                "video_explorer.rs",
                "video_quality_detector.rs",
                "video_quality_features.rs",
                "vmaf_standalone.rs",
                "x265_encoder.rs",
                "x265_params.rs",
            ][..],
        ),
        (
            "quality",
            &[
                "crf_constants.rs",
                "quality_matcher.rs",
                "quality_regression_model.rs",
                "quality_verifier_enhanced.rs",
                "real_physics.rs",
                "ssim_mapping.rs",
            ][..],
        ),
        (
            "media",
            &[
                "date_analysis.rs",
                "hdr.rs",
                "media_index_types.rs",
                "media_meta_utils.rs",
                "xmp_merger.rs",
            ][..],
        ),
        (
            "db",
            &[
                "database.rs",
                "database_vector.rs",
                "mfb_sqlite_store.rs",
                "multi_scenario_db.rs",
                "path_tree_cache.rs",
                "scenario.rs",
                "scenario_quality_lookup.rs",
            ][..],
        ),
        (
            "train",
            &[
                "c_api.rs",
                "training_entry_guard.rs",
                "training_progress.rs",
                "training_tier_audit.rs",
            ][..],
        ),
        (
            "ui",
            &[
                "modern_ui.rs",
                "progress.rs",
                "progress_mode.rs",
                "report.rs",
                "ui_stderr.rs",
                "unified_progress.rs",
            ][..],
        ),
        (
            "algo",
            &[
                "algorithm_audit.rs",
                "algorithm_runtime.rs",
                "algorithm_seal.rs",
            ][..],
        ),
        (
            "tooling",
            &["builder_base.rs", "tool_builders.rs", "tools.rs"][..],
        ),
    ] {
        if names.contains(&tail) {
            return format!("{prefix}{dir}/{tail}");
        }
    }
    if tail.starts_with("video_explorer/") {
        return format!("{prefix}video/{tail}");
    }
    rel.to_string()
}

fn join_legacy_aware(root: &impl AsRef<Path>, rel: &str) -> PathBuf {
    let base = root.as_ref();
    let direct = base.join(rel);
    if direct.exists() {
        return direct;
    }
    base.join(map_legacy_rel(rel))
}

/// True when the contract milestone table defines row M{n} (column padding
/// tolerant).
fn contract_documents_milestone(contract: &str, n: u32) -> bool {
    contract_table_documents_row(contract, 'M', n)
}

/// True when a markdown contract table defines row `{prefix}{n}` (column
/// padding tolerant).
fn contract_table_documents_row(contract: &str, prefix: char, n: u32) -> bool {
    let needle = format!("{prefix}{n}");
    let row_prefix = format!("| {prefix}");
    contract.lines().any(|line| {
        line.starts_with(&row_prefix)
            && line
                .split('|')
                .nth(1)
                .is_some_and(|col| col.trim() == needle)
    })
}

/// True when `LOGGING_LAYER_CONTRACT.md` defines row L{n}.
fn logging_contract_documents_row(contract: &str, n: u32) -> bool {
    contract_table_documents_row(contract, 'L', n)
}

fn baseline_usize_field(baseline: &serde_json::Value, key: &str) -> usize {
    let raw = baseline
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| panic!("baseline missing {key}")); // audited: contract test assertion path; panic/expect is test-only failure signal
    usize::try_from(raw).unwrap_or_else(|_| panic!("baseline {key} does not fit usize")) // audited: contract test assertion path; panic/expect is test-only failure signal
}

const MC_FORBIDDEN_MAP_OR_NA: &[&str] = &["map_or_else(|| \"N/A\""];
const MC_FORBIDDEN_NA_STRING: &[&str] = &["|| \"N/A\".to_string()", "map_or_else(|| \"N/A\""];
const MC_FORBIDDEN_MUTEX_OK: &[&str] = &[concat!("TERMINAL_LOCK.lock().", "ok", "()")];
const MC_FORBIDDEN_NONE_NA: &[&str] = &["None => \"N/A\".to_string()"];
const MC_FORBIDDEN_M50_EMOJI: &[&str] = &["log_detail!(&format!(\"📊", "emit_stderr(&format!(\"⚠️"];
const MC_FORBIDDEN_M52_BAIL: &[&str] = &["bail!(\"❌", "\"❌ PATH", "\"❌ Only"];
const MC_FORBIDDEN_M53_SYMBOLS: &[&str] = &["symbols::pick(\"❌\", \"[ERROR]\")"];
const MC_FORBIDDEN_PRECISION_UNWRAP: &[&str] =
    &["unwrap_or(", "unwrap_or_default(", ".map_or_else("];
const MC_FORBIDDEN_PRECISION_PIX_FMT_LITERAL: &[&str] =
    &["\"yuv420p10le\"", "\"yuv420p\"", "\"rgb48le\"", "\"rgb24\""];

/// Production scope: strip `mod tests { … }` when present (files may use
/// `#[cfg(test)]` imports at top).
fn production_scope(content: &str) -> &str {
    if let Some((idx, _)) = content.match_indices("\nmod tests {").next() {
        &content[..idx]
    } else if let Some((idx, _)) = content.match_indices("\nmod tests\n").next() {
        &content[..idx]
    } else if let Some((idx, _)) = content.match_indices("\n#[cfg(test)]\nmod tests").next() {
        &content[..idx]
    } else {
        content
    }
}

fn production_rust_files(root: &Path) -> Vec<PathBuf> {
    workspace_crate_production_rust_files(root, &["foundation", "img", "vid"])
}

/// All `crates/<name>/src/**/*.rs` production units (M222 repo-wide
/// silent-fabrication scan).
fn workspace_crate_production_rust_files(root: &Path, crate_names: &[&str]) -> Vec<PathBuf> {
    crate_names
        .iter()
        .flat_map(|name| {
            WalkDir::new(root.join("crates").join(name).join("src"))
                .into_iter()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_file())
                .filter(|entry| {
                    entry
                        .path()
                        .extension()
                        .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
                })
                .filter(|entry| {
                    entry
                        .path()
                        .file_name()
                        .is_none_or(|name| name != std::ffi::OsStr::new("algorithm_audit.rs"))
                })
                .filter(|entry| {
                    !entry
                        .path()
                        .components()
                        .any(|c| c.as_os_str() == std::ffi::OsStr::new("tests"))
                })
                .map(|entry| entry.path().to_path_buf())
        })
        .collect()
}

fn workspace_all_crate_production_rust_files(root: &Path) -> Vec<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("crates"))
        .unwrap_or_else(|e| panic!("read crates/: {e}")) // audited: contract test assertion path; panic/expect is test-only failure signal
        .filter_map(Result::ok)
        .filter(|e| matches!(e.file_type(), Ok(t) if t.is_dir()))
        .filter_map(|e| {
            let file_name = e.file_name();
            let name = file_name.to_str()?;
            Some(name.to_owned())
        })
        .collect();
    names.sort();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    workspace_crate_production_rust_files(root, &refs)
}

fn workspace_bin_rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let crates_dir = root.join("crates");
    let crate_entries = std::fs::read_dir(&crates_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", crates_dir.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    for crate_entry in crate_entries.filter_map(Result::ok) {
        if !matches!(crate_entry.file_type(), Ok(t) if t.is_dir()) {
            continue;
        }
        let bin_dir = crate_entry.path().join("src").join("bin");
        if !bin_dir.is_dir() {
            continue;
        }
        for bin_entry in std::fs::read_dir(&bin_dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", bin_dir.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
            .filter_map(Result::ok)
        {
            let path = bin_entry.path();
            if path
                .extension()
                .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn workspace_dev_scripts_py_files(root: &Path) -> Vec<PathBuf> {
    let scripts_dir = root.join("crates/dev/scripts");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&scripts_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", scripts_dir.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext == std::ffi::OsStr::new("py"))
        {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn workspace_fuzz_rust_files(root: &Path) -> Vec<PathBuf> {
    let fuzz_dir = root.join("crates/dev/src/fuzz/fuzz_targets");
    let mut files = Vec::new();
    if !fuzz_dir.is_dir() {
        return files;
    }
    for entry in std::fs::read_dir(&fuzz_dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", fuzz_dir.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
        {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn workspace_whole_repo_rust_production_files(root: &Path) -> Vec<PathBuf> {
    let mut files = workspace_all_crate_production_rust_files(root);
    let migrations = root.join("migrations");
    if migrations.is_dir() {
        for entry in std::fs::read_dir(&migrations)
            .unwrap_or_else(|e| panic!("read {}: {e}", migrations.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn whole_repo_silent_measurement_forgery_offenders(root: &Path) -> Vec<String> {
    silent_fabrication_offenders_in_files(
        root,
        &workspace_whole_repo_rust_production_files(root),
        MC_FORBIDDEN_SILENT_MEASUREMENT_FORGERY,
    )
}

fn is_allowlisted_numeric_fallback(relative_file: &str, line: &str) -> bool {
    // All production numeric-default paths must use media_conversion_gate helpers
    // (M43).
    const ALLOWLIST: &[(&str, &str)] = &[];

    ALLOWLIST
        .iter()
        .any(|(file, snippet)| relative_file == *file && line.contains(snippet))
}

fn offending_lines(root: &Path, files: &[PathBuf], patterns: &[&str]) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in files {
        let content = fs::read_to_string(file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if patterns.iter().any(|pattern| line.contains(pattern)) {
                // Skip matches that are likely inside unit tests or test modules by
                // scanning a small window of previous lines for test annotations.
                let start = idx.saturating_sub(20);
                let in_test_context = lines[start..idx].iter().any(|l| {
                    l.contains("#[test]")
                        || l.contains("#[cfg(test)]")
                        || l.contains("mod tests")
                        || l.contains("proptest!")
                        || l.trim_start().starts_with("fn test_")
                });
                if in_test_context {
                    continue;
                }
                let rel = file.strip_prefix(root).unwrap_or(file);
                let rel_str = rel.to_string_lossy();
                // Gate helpers are the audited home for delivery defaults (M39).
                if rel_str == "crates/foundation/src/media_conversion_gate.rs" {
                    continue;
                }
                if is_allowlisted_numeric_fallback(&rel_str, line) {
                    continue;
                }
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

const NUMERIC_FORGERY_PATTERNS: &[&str] = &[
    "unwrap_or(0)",
    "unwrap_or(0.0)",
    "unwrap_or(&0.0",
    "unwrap_or(&0.0_f64",
    "unwrap_or(0usize)",
    "unwrap_or(0u32)",
    "unwrap_or(0u64)",
    "unwrap_or(1)",
    "unwrap_or(1.0)",
    "unwrap_or(2)",
    "unwrap_or(0.5)",
    "unwrap_or(85)",
    "unwrap_or(35)",
    "unwrap_or(0x",
    "unwrap_or(u16::MAX",
    "unwrap_or(usize::MAX",
    "map_or(0,",
    "map_or(0.0,",
    "map_or(0.0_f64,",
    "map_or(1,",
    "map_or(1.0,",
    "map_or(100.0,",
    "map_or(100,",
    "map_or(4,",
];

const MC_FORBIDDEN_M68_EXTENDED: &[&str] = &[
    ".unwrap_or_default()",
    "map_or(true,",
    "available_parallelism().map_or(4",
];

const MC_FORBIDDEN_M167_POISON: &[&str] = &[".unwrap_or_else(std::sync::PoisonError::into_inner)"];

const MC_FORBIDDEN_M168_CWD: &[&str] = &["std::env::current_dir()"];

const MC_FORBIDDEN_M169_TEMP: &[&str] = &["std::env::temp_dir()"];

const MC_FORBIDDEN_M171_TMPDIR: &[&str] = &["get_mfb_tmp_dir()"];

const MC_FORBIDDEN_M171_TEMPDIR_IN: &[&str] = &[".tempfile_in("];

const MC_FORBIDDEN_M172_PARENT_UNWRAP: &[&str] =
    &["parent().unwrap_or_else", ".parent().unwrap_or"];

const MC_FORBIDDEN_M172_PATH_DOT: &[&str] = &["Path::new(\".\")"];

const MC_FORBIDDEN_M172_SILENT_MKDIR: &[&str] = &["let _ = std::fs::create_dir_all"];

const MC_FORBIDDEN_M173_SILENT_FS: &[&str] = &[
    "let _ = std::fs::remove_file",
    "let _ = std::fs::rename",
    "let _ = std::fs::copy",
];

const MC_FORBIDDEN_M174_FILE_STEM_RAW: &[&str] =
    &["file_stem().and_then(|s| s.to_str()).unwrap_or"];

const MC_FORBIDDEN_M176_STDERR_LINE: &[&str] =
    &["lines().last().unwrap_or", "lines().next().unwrap_or"];

const MC_FORBIDDEN_M177_FILE_NAME_MAPOR: &[&str] = &["file_name().map_or"];

const MC_FORBIDDEN_M178_FILE_STEM_OK_OR: &[&str] = &["file_stem().ok_or"];

const MC_FORBIDDEN_M179_BEST_SIZE_UNWRAP: &[&str] = &["best_size.unwrap_or_else"];

const MC_FORBIDDEN_M179_PRECHECK_NB_DIRECT: &[&str] = &["explore_precheck_nb_frames_or_zero("];

const MC_FORBIDDEN_M180_READ_ERROR_UNWRAP: &[&str] = &["read_error.unwrap_or_else"];

const MC_FORBIDDEN_M181_CHECKPOINT_START: &[&str] = &["get_process_start_time().unwrap_or_else"];

const MC_FORBIDDEN_M181_DATE_FIELD: &[&str] = &[
    "file_name.clone().unwrap_or_else",
    "source_file.clone().unwrap_or_else",
];

const MC_FORBIDDEN_M182_CONTENT_TYPE: &[&str] = &["content_type.unwrap_or_else"];

const MC_FORBIDDEN_M182_JPEG_QT: &[&str] = &["\"delivery_jpeg_qt\""];

const MC_FORBIDDEN_M183_TOOL_INLINE: &[&str] = &["resolve_tool_path(name).unwrap_or_else"];

const MC_FORBIDDEN_M183_VQD_CRF: &[&str] = &["extract_crf_from_params(params).unwrap_or_else"];

const MC_FORBIDDEN_M184_PATH_ENV: &[&str] = &["var_os(\"PATH\").unwrap_or_else"];

const MC_FORBIDDEN_M184_MEMORY_MB: &[&str] = &["get_memory_mb().unwrap_or_else"];

const MC_FORBIDDEN_M184_RSYNC: &[&str] = &["\"rsync\".to_string()"];

const MC_FORBIDDEN_M186_BATCH_PERCEIVED_SPEED: &[&str] = &[
    "collect_image_files_for_perceived_speed",
    "collect_video_files_for_perceived_speed",
];

const MC_FORBIDDEN_M186_ANALYZER_UNWRAP: &[&str] = &[
    "parse_jxlinfo_output(&stdout).unwrap_or_else",
    "pixel_fallback_lossless(path).unwrap_or_else",
    "parse_frame_rate(r_frame_rate).unwrap_or_else",
    "get(\"r_frame_rate\").and_then(|v| v.as_str()).unwrap_or_else",
];

const MC_FORBIDDEN_M187_DB_INLINE: &[&str] = &[
    "f64_to_usize_strict(scaled_index.floor(), \"lower_index\")\n        .unwrap_or_else",
    "f64_to_usize_strict(scaled_index.ceil(), \"upper_index\")\n        .unwrap_or_else",
    "from_value::<LoopIntentStoredMetadata>(metadata_value)\n        .unwrap_or_else",
];

const MC_FORBIDDEN_M188_RUNTIME_UI_STREAM_UOE: &[&str] = &[
    "CTRLC_PROMPT_TIMEOUT_MS).unwrap_or_else",
    "GLOBAL_LOGGER.get().unwrap_or_else",
    ".unwrap_or_else(|| \"python3\".to_string())",
    ".unwrap_or(\"python3\".to_string())",
    "u64::try_from(now_ms).unwrap_or_else",
    "f64_to_rational_strict(overhead_percent, \"overhead_percent\")\n                \
     .unwrap_or_else",
    "f64_to_u64_strict(overhead.to_f64(), \"overhead\")\n            .unwrap_or_else",
    "width.unwrap_or_else(||",
    "height.unwrap_or_else(||",
];

const MC_FORBIDDEN_M189_EXPLORE_JXL_UOE: &[&str] = &[
    "get_frame_count(path).unwrap_or_else",
    "count_frames_from_bytes(&data).unwrap_or_else",
    "f64_to_u64_strict(\n        crate::numeric_cast::u64_to_f64(input_size) * \
     JXL_NEAR_BEST_MARGIN_RATIO,\n        \"jxl_margin\",\n    )\n    .unwrap_or_else",
    "f64_to_u64_strict(margin.to_f64(), \"margin\")\n            .unwrap_or_else",
];

const MC_FORBIDDEN_M190_METRICS_MARGIN_UOE: &[&str] = &[
    "f64_to_u64_strict(mse_sum.round(), \"mse_sum\").unwrap_or_else",
    "Rational::from_f64(count).unwrap_or_else",
    "usize_to_u32_strict(px, \"px\").unwrap_or_else",
    "usize_to_u32_strict(py, \"py\").unwrap_or_else",
    "Rational::from_f64(C1).unwrap_or_else",
    "Rational::from_f64(C2).unwrap_or_else",
    "usize_to_u32_strict(WINDOW_SIZE, \"window_size\")\n            .unwrap_or_else",
    "METADATA_MARGIN_PERCENT\",\n            )\n            .unwrap_or_else",
    "f64_to_u64_strict(m.to_f64(), \"margin\")\n                .unwrap_or_else",
    "\"metadata_margin\",\n            )\n            .unwrap_or_else",
];

const MC_FORBIDDEN_M191_RUNTIME_EXPLORE_UOE: &[&str] = &[
    "detect_animation(path, &format)\n            .unwrap_or_else",
    "usize::try_from(\n                client.execute(\n                    &format!(\"DELETE FROM {table} WHERE created_at < $1\"),\n                    &[&threshold],\n                )?,\n            )\n            .unwrap_or_else",
    ".template(template)\n        .unwrap_or_else",
    "check_lossless_integrity(\n            self.input,\n            self.output,\n            result.output_size,\n            true,\n        )\n        .unwrap_or_else",
    "check_lossless_integrity(\n                input,\n                output,\n                final_full_size,\n                true,\n            )\n            .unwrap_or_else",
    "serde_json::from_str(json).unwrap_or_else",
    "serde_json::from_value(rules_array.clone()).unwrap_or_else",
    "f64_to_i32_strict(\n        (normalized * JXL_REGION_BUCKET_COUNT).floor(),\n        \"region_bucket\",\n    )\n    .unwrap_or_else",
    "u8::try_from(t.clamp(0, crate::constants::QUALITY_TWEAK_STANDARD_MAX_TICK))\n                    .unwrap_or_else",
    "u8::try_from(t.clamp(0, crate::constants::QUALITY_TWEAK_HIGH_MAX_TICK))\n                    .unwrap_or_else",
    "f64_to_u8_strict(\n        f64::from(crate::constants::FALLBACK_CRF_VIDEO),\n        \"fallback\",\n    )\n    .unwrap_or_else",
    "result.output_path.clone().unwrap_or_else",
    ".get((i - 1) as usize)\n            .copied()\n            .unwrap_or_else",
    "pkt_sizes\n        .get(1..pkt_sizes.len().saturating_sub(1))\n        .unwrap_or_else",
    ".duration_since(UNIX_EPOCH)\n                .unwrap_or_else",
    ".or_else(|| payload.downcast_ref::<String>().cloned())\n            .unwrap_or_else",
    "diff.unwrap_or_else(|| (i - o).abs())",
    ".filter(|value| !value.trim().is_empty())\n        .unwrap_or_else(|| foundation::database::PG_DEFAULT_CONNSTR.to_string())",
    ".unwrap_or(foundation::database::PG_DEFAULT_CONNSTR.to_string())",
    ".finalists\n                        .get(*best_idx)\n                        .unwrap_or_else",
];

const MC_FORBIDDEN_M206_VIDEO_DETECTION_OR_ELSE: &[&str] = &[
    ".or_else(|| read_container_header_prefix(path, \"apng\"",
    ".or_else(|| crate::image_formats::webp::canvas_dimensions_from_path(path))",
    ".or_else(|| {\n            try_probe_from_animated_gif_header(path)",
    ".or_else(|| {\n            try_probe_from_animated_apng_header(path)",
    "media_info_without_ffprobe(path).or_else(||",
    "probe.bit_rate.or_else(||",
];

const MC_FORBIDDEN_M207_VIDEO_EXPLORER_OR_ELSE: &[&str] = &[
    ".or_else(|| try_ffprobe_count(\"-count_packets\"",
    ".or_else(|| run_ssim_all_filter(input, output, GIF_RGB24))",
    ".or_else(|| run_ssim_all_filter(input, output, GIF_NORM))",
    ".or_else(|| run_ssim_all_filter(input, output, FORMAT_NORM))",
    ".or_else(|| run_ssim_all_filter(input, output, ALPHA_FLATTEN))",
    ".or_else(|| stream[\"nb_frames\"].as_u64())",
];

const MC_FORBIDDEN_M208_IMAGE_HEIC_DETECTION_OR_ELSE: &[&str] = &[
    ".or_else(|| find_box_data_recursive(data, *b\"colr\"))",
    ".or_else(|| find_box_data_recursive(data, *b\"pixi\"))",
    "jpeg_precision_from_header(path).or_else(|| {",
    ".or_else(|| measured_bit_depth_for_format(path, &format));",
];

const MC_FORBIDDEN_M205_QUALITY_TIMING_OR_ELSE: &[&str] = &[
    ".filter(|count| *count > 1)\n            .or_else(|| detected_frame_count",
    ".and_then(sanitize_positive_f64)\n            .or_else(|| {\n                analysis",
    ".or_else(|| probe.duration.and_then(sanitize_positive_f64))",
    ".or_else(|| preferred_fps(&probe, detected_fps))",
    ".or_else(|| fps_from_duration(frame_count, duration_secs))",
    ".or_else(|| average_pts_delay_ms(&probe.pts_deltas))",
    ".or_else(|| average_frame_delay_ms(frame_count, duration_secs))",
    ".or_else(|| normalized_delay_variation(&probe.pts_deltas))",
    ".or_else(|| probe.frame_rate.and_then(sanitize_positive_f64))",
    ".and_then(sanitize_positive_f64)\n            .or_else(|| {\n                \
     probe_frame_count",
    "probe_fps\n            .or_else(|| probe_frame_count",
    ".or_else(|| derive_frame_count(duration_secs, Some(fps)))",
    ".map(|bit_rate| crate::numeric_cast::u64_to_f64(bit_rate) / 1_000_000.0)\n            \
     .or_else(|| {",
    ".or_else(|| derive_bitrate_mbps(file_size, duration_secs))",
];

const MC_FORBIDDEN_M204_FFPROBE_HDR_OR_ELSE: &[&str] = &[
    "tags.get(\"loop_count\").or_else(|| tags.get(\"loop\"))",
    "f64_to_u64_strict(v, \"hdr_luma_raw\").or_else(||",
    ".and_then(parse_rational_to_50k)\n            .or_else(||",
    ".and_then(parse_luminance_to_10k)\n            .or_else(||",
    ".or_else(|| sd[\"MaxCLL\"].as_u64())",
    ".or_else(|| sd[\"MaxFALL\"].as_u64())",
    ".or_else(|| {\n        crate::numeric_cast::parse_option_strict(\n            \
     stream.bits_per_sample",
];

const MC_FORBIDDEN_M203_FFPROBE_LOOP_OR_ELSE: &[&str] = &[
    ".or_else(|| {\n            video_stream\n                .get(\"bits_per_sample\")",
    ".or_else(|| r_frame_rate.filter",
    ".or_else(|| {\n            if field_name == \"width\"",
    ".or_else(|| crate::conversion::dimensions_without_ffprobe(path))",
    ".or_else(|| tags.get(\"x264-params\")",
    ".or_else(|| {\n                    tags.get(\"encoder_settings\")",
    "f64_to_u64_strict(v, \"hdr_coord_raw\").or_else(||",
    "reference.duration.p50.or_else(||",
    ".or_else(|| detection.tags.get(crate::constants::TAG_ENCODER)",
    ".or_else(|| probe.tags.get(crate::constants::TAG_ENCODER)",
    ".or_else(|| self.duration_secs.map(DurationTier::from_secs)",
    ".or_else(|| {\n                    fallback_probability.and_then",
    ".or_else(|| tree.resolution_path.clone())",
];

const MC_FORBIDDEN_M202_CONVERSION_CLI_OR_ELSE: &[&str] = &[
    ".or_else(|| Some(input.display().to_string()))",
    ".or_else(|_| {\n            crate::image_builders::IdentifyBuilder::new()\n                \
     .use_magick(false)",
    "base_dir.or_else(|| {",
    "checked_mul(u64::from(height)).or_else(|| {",
    "checked_mul(u64::from(probe.height))\n            .or_else(|| {",
];

const MC_FORBIDDEN_M201_DATABASE_OR_ELSE: &[&str] = &[
    "duration_p90_empirical.or_else(||",
    ".or_else(|| (height > 0).then",
    ").or_else(|| {\n        let source_path",
    "usize_to_i32_strict(s, \"db_knn_count\").or_else(||",
    ".as_deref().map_or(\"?\", |v| v)",
];

const MC_FORBIDDEN_M200_DATABASE_TRAINING_UNWRAP_OR: &[&str] = &[
    ".unwrap_or(crate::constants::DEFAULT_LOOP_BASELINE",
    ".unwrap_or(foundation::database::PG_DEFAULT_CONNSTR",
    ".unwrap_or(\"<empty>\")",
    ".unwrap_or(\"<unknown>\")",
    ".unwrap_or(arg0.as_str())",
    ".unwrap_or(self.points.len())",
    ".unwrap_or(first.len())",
    "u64::try_from(avail_u128).unwrap_or(u64::MAX)",
    ".unwrap_or(\"\")",
    ".as_deref().unwrap_or(\"?\")",
    "u64_to_usize_strict(\n                    frames,\n                    \
     \"gif_frame_count_types\"\n                )\n                .unwrap_or_else",
    "u64_to_usize_strict(frames, \"gif_frame_count_pts\")\n                    .unwrap_or_else",
];

const MC_FORBIDDEN_M199_RUNTIME_UNWRAP_OR: &[&str] = &[
    ".unwrap_or(\"python3\".to_string())",
    ".unwrap_or_else(|| Duration::from_secs(default_secs)",
    ".unwrap_or(\"Unknown panic payload\".to_string())",
    "diff.unwrap_or(",
    "analysis.analysis_error.as_deref().unwrap_or(\"none\")",
];

const MC_FORBIDDEN_M198_API_EXPLORE_FFI_MAPOR: &[&str] = &[
    "input_to_lock.map_or_else",
    ".map_or_else(\n                                    || {\n                                        err_str.contains",
    "output_dir.map_or_else",
    "LoopMeta::from_gif_path(input).map_or_else",
    "options.base_dir.as_ref().map_or_else",
    "param.split_once('=').map_or(param",
    ".map_or(String::new(), |e| e.to_string_lossy",
    ".map_or_else(|| Duration::from_secs(default_secs), Duration::from_secs)",
    ".map_or_else(\n            || \"Unknown FFmpeg error\"",
    "v.map_or_else(|| \"—\"",
    "hint_ext.map_or_else",
    "prefix.rfind('.').map_or_else",
    ".map_or_else(\n                || {\n                    if duration > 0.0_f64",
    ".map_or_else(|| \"unknown\"",
    ".map_or_else(|| EMPTY.as_ptr",
    ".map_or_else(|| \"   N/A\"",
];

const MC_FORBIDDEN_M197_BUILDERS_PROGRESS_COPIER_MAPOR: &[&str] = &[
    "resolve_imagemagick_cli().map_or_else",
    "cmd.output().map_or_else",
    "String::from_utf8(output.stdout).map_or_else",
    ".to_str()\n            .map_or_else",
    "ue_from_chain.map_or_else",
    "ssim.map_or_else",
    "active_progress_line().map_or_else",
    "remaining_secs.map_or_else",
    "probe.color_space.as_ref().map_or_else",
    "chroma_estimate.map_or_else",
    "aspect_diff.map_or_else",
    "output_dir.map_or_else",
    ".path()\n        .map_or_else",
    "panic_info.location().map_or_else",
    "std::fs::read_to_string(path).map_or_else",
    "Self::from_json(&json).map_or_else",
    "color_info.map_or_else",
    "encoder_params.map_or_else",
    "f64_to_u8_strict(val.round(), \"crf_from_params\")\n                    .map_or_else",
];

const MC_FORBIDDEN_M196_IO_GPU_VECTOR_DB_MAPOR: &[&str] = &[
    ".get(pos..pos + 4).map_or_else",
    ".get(pos..pos + 2).map_or_else",
    ".get(pos..pos + 8).map_or_else",
    "diagnostics\n                    .first()\n                    .map_or(\"no supported \
     encoder found\"",
    ".find(' ')\n                .map_or(after_all",
    "self.prev_size.map_or(f64::MAX",
    "duration_secs.filter(|d| *d > 0.0_f64).map_or_else",
    "sample.fps.map_or_else",
    "sample.palette_size.map_or_else",
    "sample.directory_loop_intent_score.map_or_else",
    "analysis.color_depth.map_or(Value::Null",
    ".map_or(Value::Null, |h| Value::String",
];

const MC_FORBIDDEN_M195_ANALYZER_LOOP_HDR_MAPOR: &[&str] = &[
    "color_info.map_or_else(Self::default, Self::classify)",
    "jpeg_analysis.as_ref().map_or((None, None)",
    "self.jpeg_analysis.as_ref().map_or_else",
    "self.heic_analysis.as_ref().map_or_else",
    "fallback_detection.as_ref().map_or_else",
    ".map_or_else(\n        |_| {\n            match pixel_fallback_lossless",
    ".file_name()\n        .map_or_else(|| \"unknown_heic\"",
    "path.extension().and_then(|ext| ext.to_str()).map_or_else",
    "base.map_or_else(String::new, str::to_string)",
    "m.map_or(Some(x), |v: f64| Some(v.max(x))",
    "m.map_or(Some(x), |v: f64| Some(v.min(x))",
    ".map_or(name, |(s, _)| s)",
];

const MC_FORBIDDEN_M194_BATCH_DB_CONVERSION_MAPOR: &[&str] = &[
    ".map_or_else(\n            || {\n                \
     crate::media_conversion_gate::delivery_pipeline_path_audit",
    concat!(
        "path.strip_prefix(root)\n        .",
        "ok",
        "()\n        .and_then(Path::parent)\n        .map_or_else"
    ),
    "probe.frame_count.map_or_else",
    "pixels.checked_mul(frames.max(1)).map_or_else",
    "row_opt.map_or_else",
    "row_opt.map_or(Ok(None)",
    ".map_or(LabelStatus::Uncertain",
    ".map_or(name, |(s, _)| s)",
    "meta.frame_count.map_or_else",
    "mean(&all_values).map_or_else",
    "self.quality_label.filter(|q| !q.is_empty()).map_or_else",
    "i128_to_i64_strict(diff_bytes, \"size_diff\")\n            .map_or_else",
    "extra_info.map_or_else",
    "quality_label.filter(|q| !q.is_empty()).map_or_else",
];

const MC_FORBIDDEN_M193_PROBE_GPU_MAPOR: &[&str] = &[
    "max_palette.map_or(lct_colors",
    "palette_size.map_or_else",
    "path.file_name()\n        .and_then(|n| n.to_str())\n        .map_or_else",
    "entropy_norm.map_or(inverse_factor",
    "entropy_norm.map_or(palette_signal",
    "confidence.map_or_else",
    ".map_or_else(Vec::new, |rules_array|",
    "precision.palette_size.map_or_else",
    "best_rule.map_or_else",
    "baseline.explore_ssim.map_or_else",
    "ultimate_quality_passed.failure_reason().map_or_else",
    "quality_passed.failure_reason().map_or_else",
    "ms_ssim_passed.failure_reason().map_or_else",
    "enhanced_verify_fail_reason.as_ref().map_or_else",
    ".map_or_else(|| Ok(self.cpu_only_search_plan",
    "tracking.best_vmaf.map_or_else",
    "tracking.best_psnr_uv.map_or_else",
    ".map_or_else(|| \"none\".to_string(), |v| format!",
    "warm_start_crf.map_or_else",
    ".map_or(req.baseline_crf, |hint|",
];

const MC_FORBIDDEN_M192_GPU_EXPLORE_MAPOR: &[&str] = &[
    "detect_format_from_bytes(path).map_or_else",
    "payload.downcast::<String>().map_or_else",
    "state.best_crf.map_or_else",
    "lines.last().map_or_else",
    "cached.map_or_else(|| encode_cached",
    "handle.map_or_else",
    "ultimate_quality_passed.failure_reason().map_or_else",
    "ms_ssim_passed.failure_reason().map_or_else",
    "quality.0.map_or_else",
    "duration.map_or_else",
    "input.extension().and_then(|e| e.to_str()).map_or_else",
    "result.output_size().map_or_else",
    "detection.estimated_quality.map_or_else",
];

const MC_FORBIDDEN_M169_TERMINAL_MUTEX: &[&str] = &[
    concat!(
        "if let ",
        "Ok",
        "(_guard) = crate::ctrlc_guard::TERMINAL_LOCK.lock()"
    ),
    concat!(
        "&& let ",
        "Ok",
        "(_guard) = crate::ctrlc_guard::TERMINAL_LOCK.lock()"
    ),
    concat!(
        "if let ",
        "Ok",
        "(_terminal_guard) = crate::ctrlc_guard::TERMINAL_LOCK.lock()"
    ),
    concat!(
        "if let ",
        "Ok",
        "(mut held_locks) = held_dir_locks().lock()"
    ),
];

const MC_FORBIDDEN_M69_SUBSTRATE: &[&str] = &[
    ".map_or(std::ptr::null_mut(), CString::into_raw)",
    "map_or((0, 0),",
    ".map_or(fallback, |candidate| candidate.output_size)",
    "file_name.as_deref().unwrap_or(\"\")",
    ".get(key).map_or(fallback, DistributionStats::from)",
    ".map_or(luma_estimate.quality, |chroma|",
];

const MC_FORBIDDEN_M72_EXPLORE_METRICS: &[&str] = &[
    "is_valid_ssim(y)",
    "is_valid_ssim(all)",
    "return Some(100.0)",
    "self.probe.as_ref().map_or(",
    "crate::constants::PIX_FMT_YUV420P,\n            crate::hevc_yuv420_output_pix_fmt",
];

const MC_FORBIDDEN_M73_CENTRAL_PARSE: &[&str] = &[
    "precision::is_valid_ssim(",
    "precision::is_valid_ms_ssim(",
    "search_baseline.map_or(",
    "map_or(crate::constants::GPU_SAMPLE_DURATION",
    concat!("stdout.trim().parse::<f64>().", "ok", "()"),
    concat!(
        ".duration\n        .as_ref()\n        .and_then(|d| d.parse::<f64>().",
        "ok",
        "())"
    ),
];

const MC_FORBIDDEN_M74_EXPLORE_PARSE: &[&str] = &[
    "precision::is_valid_psnr(",
    "(0.0_f64..=1.0_f64).contains(&ssim)",
    "Ok(Some(f64::INFINITY))",
];

const MC_FORBIDDEN_M79_SSIM_CALC: &[&str] = &[
    "explore_precheck_batch_audit",
    "video too long (",
    "Image too small (",
    "explore_metric_numeric_end(\n                        after_colon",
    "seal_cambi(f)",
];

const MC_FORBIDDEN_M81_DYNAMIC_MAPPING: &[&str] = &[
    "explore_precheck_batch_audit",
    ".unwrap_or_else(|| f64::from(sample_duration))",
];

const MC_FORBIDDEN_M83_GPU_COARSE: &[&str] = &[
    "explore_gpu_coarse_batch_audit",
    "GPU_COARSE_SEARCH_DEFAULT_AUDIO_BITRATE / 1000",
];

const MC_FORBIDDEN_M84_PRECHECK_STREAM: &[&str] = &[
    "explore_precheck_batch_audit",
    "DURATION: stream.duration unavailable",
    "DURATION: format.duration failed",
    "SSIM method ",
    "failed, trying next method",
    "delivery_batch_audit(\n            \"explore_highly_compressed\"",
];

const MC_FORBIDDEN_M85_QUALITY: &[&str] =
    &["delivery_fallback_audit(\n                \"quality_content_type"];

const MC_FORBIDDEN_M87_CONVERSION: &[&str] = &[
    "delivery_fallback_audit(\n                \"collision_stem\"",
    "delivery_fallback_audit(\n                    \"strip_base_dir\"",
    "file_stem().and_then(|s| s.to_str())",
];

const MC_FORBIDDEN_M89_VIDEO_EXPLORER: &[&str] = &["explore_gpu_coarse_audit("];

const MC_FORBIDDEN_M90_DELIVERY_API: &[&str] = &["delivery_fallback_audit("];

const MC_FORBIDDEN_M91_ANIMATED: &[&str] = &["delivery_path_audit("];

const MC_FORBIDDEN_M92_JXL_BATCH: &[&str] = &["delivery_jxl_batch_audit("];

const MC_FORBIDDEN_M95_CONVERSION: &[&str] = &["delivery_path_audit(", "delivery_batch_audit("];

/// Production must not call always-on emitters; use strict/domain gate helpers
/// (M100).
const MC_FORBIDDEN_M100_DELIVERY_EMITTERS: &[&str] = &[
    "delivery_fallback_audit(",
    "delivery_path_audit(",
    "delivery_batch_audit(",
];

fn delivery_numeric_forgery_offenders(root: &Path) -> Vec<String> {
    offending_lines(root, &production_rust_files(root), NUMERIC_FORGERY_PATTERNS)
}

fn delivery_raw_current_dir_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M168_CWD)
}

fn delivery_raw_temp_dir_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M169_TEMP)
}

fn delivery_raw_get_mfb_tmp_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate_and_process_lock(root, MC_FORBIDDEN_M171_TMPDIR)
}

fn delivery_raw_tempfile_in_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M171_TEMPDIR_IN)
}

fn delivery_raw_parent_unwrap_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M172_PARENT_UNWRAP)
}

fn delivery_raw_path_dot_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M172_PATH_DOT)
}

fn delivery_silent_create_dir_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M172_SILENT_MKDIR)
}

fn delivery_silent_fs_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M173_SILENT_FS)
}

fn delivery_remove_file_unwrap_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if (line.contains("std::fs::remove_file(") || line.contains("remove_file("))
                && line.contains("unwrap_or_else")
                && !line.contains("delivery_remove_file_or_audit")
            {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_raw_file_stem_fallback_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M174_FILE_STEM_RAW)
}

fn delivery_extension_map_or_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if line.contains(".extension().map_or") || line.contains("extension().map_or_else") {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_stderr_line_unwrap_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M176_STDERR_LINE)
}

fn delivery_file_name_map_or_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M177_FILE_NAME_MAPOR)
}

fn delivery_file_stem_ok_or_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M178_FILE_STEM_OK_OR)
}

fn delivery_gpu_best_size_unwrap_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M179_BEST_SIZE_UNWRAP)
}

fn delivery_precheck_nb_frames_direct_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M179_PRECHECK_NB_DIRECT)
}

fn delivery_read_error_unwrap_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M180_READ_ERROR_UNWRAP)
}

fn delivery_checkpoint_start_unwrap_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M181_CHECKPOINT_START)
}

fn delivery_date_field_unwrap_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M181_DATE_FIELD)
}

fn delivery_content_type_unwrap_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M182_CONTENT_TYPE)
}

fn delivery_tool_path_inline_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M183_TOOL_INLINE)
}

fn delivery_vqd_crf_inline_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M183_VQD_CRF)
}

fn delivery_path_env_inline_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M184_PATH_ENV)
}

fn delivery_memory_mb_inline_offenders(root: &Path) -> Vec<String> {
    delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M184_MEMORY_MB)
}

fn delivery_rsync_inline_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|n| n == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if line.contains(MC_FORBIDDEN_M184_RSYNC[0])
                && line.contains("unwrap_or_else")
                && !line.contains("delivery_rsync_executable_or_default")
            {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_batch_perceived_speed_unwrap_offenders(root: &Path) -> Vec<String> {
    let file = join_legacy_aware(&root, "crates/foundation/src/batch.rs");
    let content =
        fs::read_to_string(&file).unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&content);
    let mut offenders = Vec::new();
    for name in MC_FORBIDDEN_M186_BATCH_PERCEIVED_SPEED {
        let start = prod
            .find(&format!("pub fn {name}"))
            .unwrap_or_else(|| panic!("batch must define {name}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let tail = &prod[start..];
        let end = tail
            .find("\n\npub fn ")
            .or_else(|| tail.find("\n\n#[must_use]"))
            .unwrap_or(tail.len());
        let body = &tail[..end];
        if body.contains("unwrap_or_else") {
            offenders.push(format!(
                "crates/foundation/src/batch.rs: {name} contains unwrap_or_else"
            ));
        }
    }
    offenders
}

fn delivery_image_analyzer_probe_unwrap_offenders(root: &Path) -> Vec<String> {
    let file = join_legacy_aware(&root, "crates/foundation/src/image_analyzer.rs");
    let content =
        fs::read_to_string(&file).unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&content);
    let mut offenders = Vec::new();
    for needle in MC_FORBIDDEN_M186_ANALYZER_UNWRAP {
        if prod.contains(needle) {
            offenders.push(format!(
                "crates/foundation/src/image_analyzer.rs: contains forbidden '{needle}'"
            ));
        }
    }
    offenders
}

fn delivery_db_inline_unwrap_offenders(root: &Path) -> Vec<String> {
    let file = join_legacy_aware(&root, "crates/foundation/src/database.rs");
    let content =
        fs::read_to_string(&file).unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&content);
    let mut offenders = Vec::new();
    for needle in MC_FORBIDDEN_M187_DB_INLINE {
        if prod.contains(needle) {
            offenders.push(format!(
                "crates/foundation/src/database.rs: contains forbidden '{needle}'"
            ));
        }
    }
    offenders
}

fn delivery_runtime_ui_stream_uoe_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/ctrlc_guard.rs",
        "crates/foundation/src/modern_ui.rs",
        "crates/foundation/src/quality_regression_model.rs",
        "crates/foundation/src/lru_cache.rs",
        "crates/foundation/src/stream_size.rs",
        "crates/foundation/src/video_quality_detector.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M188_RUNTIME_UI_STREAM_UOE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_explore_jxl_uoe_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/video_explorer/stream_analysis.rs",
        "crates/foundation/src/jxl_explorer.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M189_EXPLORE_JXL_UOE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_metrics_margin_uoe_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/image_metrics.rs",
        "crates/foundation/src/video_explorer.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M190_METRICS_MARGIN_UOE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_runtime_explore_uoe_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/scenario_quality_lookup.rs",
        "crates/foundation/src/analysis_cache.rs",
        "crates/foundation/src/unified_progress.rs",
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
        "crates/foundation/src/image_quality_detector.rs",
        "crates/foundation/src/jxl_explorer.rs",
        "crates/foundation/src/video_quality_detector.rs",
        "crates/img/src/main.rs",
        "crates/vid/src/animated_image.rs",
        "crates/foundation/src/loop_intent.rs",
        "crates/foundation/src/checkpoint.rs",
        "crates/foundation/src/error_handler.rs",
        "crates/foundation/src/quality_verifier_enhanced.rs",
        "crates/foundation/src/bin/train_knn.rs",
        "crates/foundation/src/bin/train_quality.rs",
        "crates/img/src/lossless_converter.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M191_RUNTIME_EXPLORE_UOE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_video_detection_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    let file = join_legacy_aware(&root, "crates/foundation/src/video_detection.rs");
    let content =
        fs::read_to_string(&file).unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&content);
    for needle in MC_FORBIDDEN_M206_VIDEO_DETECTION_OR_ELSE {
        if prod.contains(needle) {
            offenders.push(format!("video_detection.rs: contains forbidden '{needle}'"));
        }
    }
    if prod.contains(".or_else(") {
        offenders.push("video_detection.rs: production scope still contains .or_else(".to_string());
    }
    offenders
}

fn delivery_stream_analysis_precheck_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/video_explorer/stream_analysis.rs",
        "crates/foundation/src/video_explorer/precheck.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M207_VIDEO_EXPLORER_OR_ELSE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_image_heic_detection_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/image_heic_analysis.rs",
        "crates/foundation/src/image_detection.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M208_IMAGE_HEIC_DETECTION_OR_ELSE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
        if prod.contains(".or_else(") {
            offenders.push(format!("{rel}: production scope still contains .or_else("));
        }
    }
    offenders
}

fn delivery_logging_system_memory_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/logging.rs",
        "crates/foundation/src/system_memory.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        if prod.contains(".or_else(") {
            offenders.push(format!("{rel}: production scope still contains .or_else("));
        }
    }
    offenders
}

fn delivery_remaining_or_else_closures_m210_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/msssim_progress.rs",
        "crates/foundation/src/image_formats.rs",
        "crates/foundation/src/video_explorer.rs",
        "crates/foundation/src/media_precision.rs",
        "crates/foundation/src/xmp_merger.rs",
        "crates/foundation/src/scenario_quality_lookup.rs",
        "crates/foundation/src/gpu_accel.rs",
        "crates/foundation/src/image_analyzer.rs",
        "crates/foundation/src/video_quality_detector.rs",
        "crates/foundation/src/image_jpeg_analysis.rs",
        "crates/foundation/src/io_utils.rs",
        "crates/foundation/src/common_utils.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        if prod.contains(".or_else(||") {
            offenders.push(format!(
                "{rel}: production scope still contains .or_else(||"
            ));
        }
    }
    offenders
}

fn delivery_quality_timing_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/animated_image_quality_features.rs",
        "crates/foundation/src/video_quality_features.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M205_QUALITY_TIMING_OR_ELSE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
        if prod.contains(".or_else(") {
            offenders.push(format!("{rel}: production scope still contains .or_else("));
        }
    }
    offenders
}

fn delivery_ffprobe_hdr_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/ffprobe.rs",
        "crates/foundation/src/ffprobe_json.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M204_FFPROBE_HDR_OR_ELSE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_ffprobe_loop_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/ffprobe.rs",
        "crates/foundation/src/loop_intent.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M203_FFPROBE_LOOP_OR_ELSE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_conversion_cli_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/conversion.rs",
        "crates/foundation/src/batch.rs",
        "crates/vid/src/main.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M202_CONVERSION_CLI_OR_ELSE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_database_or_else_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/database.rs",
        "crates/foundation/src/bin/db_diagnostics.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M201_DATABASE_OR_ELSE {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_database_training_unwrap_or_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/database.rs",
        "crates/foundation/src/process_runner.rs",
        "crates/foundation/src/training_progress.rs",
        "crates/foundation/src/entry_guard.rs",
        "crates/foundation/src/system_memory.rs",
        "crates/foundation/src/ssim_mapping.rs",
        "crates/foundation/src/progress_mode.rs",
        "crates/foundation/src/bin/train_knn.rs",
        "crates/foundation/src/bin/train_quality.rs",
        "crates/foundation/src/bin/db_diagnostics.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M200_DATABASE_TRAINING_UNWRAP_OR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_runtime_unwrap_or_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/quality_regression_model.rs",
        "crates/foundation/src/error_handler.rs",
        "crates/foundation/src/quality_verifier_enhanced.rs",
        "crates/foundation/src/analysis_cache.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M199_RUNTIME_UNWRAP_OR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_api_explore_ffi_mapor_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/img/src/main.rs",
        "crates/img/src/conversion_api.rs",
        "crates/vid/src/conversion_api.rs",
        "crates/vid/src/animated_image.rs",
        "crates/foundation/src/x265_params.rs",
        "crates/foundation/src/metadata/exif.rs",
        "crates/foundation/src/quality_regression_model.rs",
        "crates/foundation/src/ffmpeg_process.rs",
        "crates/foundation/src/quality_matcher.rs",
        "crates/foundation/src/xmp_merger.rs",
        "crates/foundation/src/progress_mode.rs",
        "crates/foundation/src/video_explorer/precheck.rs",
        "crates/foundation/src/video_explorer/error_handling.rs",
        "crates/foundation/src/c_api.rs",
        "crates/foundation/src/bin/db_diagnostics.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M198_API_EXPLORE_FFI_MAPOR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_builders_progress_copier_mapor_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/image_builders.rs",
        "crates/foundation/src/checkpoint.rs",
        "crates/foundation/src/cli_runner.rs",
        "crates/foundation/src/modern_ui.rs",
        "crates/foundation/src/progress.rs",
        "crates/foundation/src/video_detection.rs",
        "crates/foundation/src/image_jpeg_analysis.rs",
        "crates/foundation/src/smart_file_copier.rs",
        "crates/foundation/src/file_copier.rs",
        "crates/foundation/src/error_handler.rs",
        "crates/foundation/src/lru_cache.rs",
        "crates/foundation/src/video_quality_detector.rs",
        "crates/img/src/lossless_converter.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M197_BUILDERS_PROGRESS_COPIER_MAPOR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_io_gpu_vector_db_mapor_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/io_utils.rs",
        "crates/foundation/src/gpu_accel.rs",
        "crates/foundation/src/video_explorer.rs",
        "crates/foundation/src/database_vector.rs",
        "crates/foundation/src/image_quality_db.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M196_IO_GPU_VECTOR_DB_MAPOR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_analyzer_loop_hdr_mapor_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/image_analyzer.rs",
        "crates/foundation/src/loop_intent.rs",
        "crates/foundation/src/hdr.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M195_ANALYZER_LOOP_HDR_MAPOR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_batch_db_conversion_mapor_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/batch.rs",
        "crates/foundation/src/database.rs",
        "crates/foundation/src/conversion.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M194_BATCH_DB_CONVERSION_MAPOR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_probe_gpu_mapor_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/image_detection.rs",
        "crates/foundation/src/image_quality_detector.rs",
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M193_PROBE_GPU_MAPOR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_gpu_explore_mapor_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for rel in [
        "crates/foundation/src/analysis_cache.rs",
        "crates/foundation/src/gpu_accel.rs",
        "crates/foundation/src/video_explorer.rs",
        "crates/foundation/src/cli_runner.rs",
        "crates/img/src/conversion_api.rs",
    ] {
        let file = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in MC_FORBIDDEN_M192_GPU_EXPLORE_MAPOR {
            if prod.contains(needle) {
                offenders.push(format!("{rel}: contains forbidden '{needle}'"));
            }
        }
    }
    offenders
}

fn delivery_jpeg_qt_inline_audit_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    let file = join_legacy_aware(&root, "crates/foundation/src/image_jpeg_analysis.rs");
    let content =
        fs::read_to_string(&file).unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&content);
    for (idx, line) in prod.lines().enumerate() {
        if line.contains(MC_FORBIDDEN_M182_JPEG_QT[0])
            && line.contains("delivery_numeric_fallback_audit")
        {
            let rel = file.strip_prefix(root).unwrap_or(&file);
            offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
        }
    }
    offenders
}

fn delivery_jpeg_inline_slice_unwrap_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    let file = join_legacy_aware(&root, "crates/foundation/src/image_jpeg_analysis.rs");
    let content =
        fs::read_to_string(&file).unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&content);
    for (idx, line) in prod.lines().enumerate() {
        if !line.contains("unwrap_or_else") {
            continue;
        }
        let window: String = prod
            .lines()
            .skip(idx)
            .take(6)
            .collect::<Vec<_>>()
            .join("\n");
        if window.contains("probe_image_format_batch_audit") && window.contains(".get(") {
            let rel = file.strip_prefix(root).unwrap_or(&file);
            offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
        }
    }
    offenders
}

fn delivery_raw_remove_file_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if line.contains("std::fs::remove_file")
                && line.contains("if let Err(")
                && !line.contains("delivery_remove_file_or_audit")
            {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_path_strip_prefix_fallback_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if line.contains("strip_prefix_or_self") {
                continue;
            }
            let path_strip = line.contains(".strip_prefix(")
                && (line.contains("Path") || line.contains("path."));
            let str_strip = line.contains("line.strip_prefix(") || line.contains("s.strip_prefix(");
            if path_strip
                && !str_strip
                && (line.contains("unwrap_or") || line.contains("map_or_else"))
            {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_pattern_offenders_outside_gate_and_process_lock(
    root: &Path,
    patterns: &[&str],
) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file.file_name().is_some_and(|name| {
            name == std::ffi::OsStr::new("media_conversion_gate.rs")
                || name == std::ffi::OsStr::new("process_lock.rs")
        }) {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if patterns.iter().any(|pattern| line.contains(pattern)) {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_terminal_mutex_offenders(root: &Path) -> Vec<String> {
    let mut offenders =
        delivery_pattern_offenders_outside_gate(root, MC_FORBIDDEN_M169_TERMINAL_MUTEX);
    offenders.extend(delivery_pattern_offenders_outside_gate(
        root,
        MC_FORBIDDEN_MUTEX_OK,
    ));
    offenders.sort();
    offenders.dedup();
    offenders
}

fn delivery_unscoped_tempfile_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if line.contains("tempfile::TempDir::new()")
                || (line.contains(".tempdir()") && !line.contains("tempdir_in"))
                || (line.contains(".tempfile()") && !line.contains("tempfile_in"))
                || line.contains("NamedTempFile::new()")
            {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_pattern_offenders_outside_gate(root: &Path, patterns: &[&str]) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if patterns.iter().any(|pattern| line.contains(pattern)) {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn delivery_poison_recovery_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in production_rust_files(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if MC_FORBIDDEN_M167_POISON
                .iter()
                .any(|pattern| line.contains(pattern))
            {
                let rel = file.strip_prefix(root).unwrap_or(&file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

#[test]
fn production_code_has_no_numeric_forgery_fallbacks() {
    let root = workspace_root();
    let offenders = delivery_numeric_forgery_offenders(&root);
    assert!(
        offenders.is_empty(),
        "numeric metadata must not be forged with unwrap_or/map_or 0/1 defaults:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn release_workflow_does_not_publish_partial_artifacts() {
    let root = workspace_root();
    let release = fs::read_to_string(root.join(".github/workflows/cd-stable.yml"))
        .expect("stable release workflow must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    for forbidden in [
        "continue-on-error: ${{ matrix.optional == true }}",
        "if: always() && !cancelled()",
        "fail_on_unmatched_files: false",
    ] {
        assert!(
            !release.contains(forbidden),
            "release workflow still contains partial-success pattern: {forbidden}"
        );
    }
}

#[test]
fn dependency_installation_is_not_silenced_in_release_workflows() {
    let root = workspace_root();
    for workflow in [
        ".github/workflows/cd-stable.yml",
        ".github/workflows/cd-nightly.yml",
    ] {
        let content = fs::read_to_string(root.join(workflow))
            .unwrap_or_else(|err| panic!("read {workflow}: {err:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let offenders: Vec<_> = content
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("brew install") && line.contains("|| true"))
            .map(|(idx, line)| format!("{workflow}:{}: {}", idx + 1, line.trim()))
            .collect();
        assert!(
            offenders.is_empty(),
            "release dependency installation must fail loudly:\n{}",
            offenders.join("\n")
        );
    }
}

#[test]
fn multi_scenario_constraints_are_table_scoped_not_name_only_patches() {
    let root = workspace_root();
    let runtime_schema = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/multi_scenario_db.rs",
    ))
    .expect("runtime schema initializer must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let migration_schema =
        fs::read_to_string(root.join("crates/dev/src/migrations/001_multi_scenario_embedding.sql"))
            .expect("multi-scenario migration must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    for (table, constraint) in [
        (
            "image_quality_samples",
            "image_quality_samples_quality_score_check",
        ),
        (
            "animated_image_quality_samples",
            "animated_image_quality_samples_quality_score_check",
        ),
        (
            "video_quality_samples",
            "video_quality_samples_quality_score_check",
        ),
        ("loop_samples", "loop_samples_media_metadata_check"),
        (
            "image_quality_samples",
            "image_quality_samples_media_metadata_check",
        ),
        (
            "animated_image_quality_samples",
            "animated_image_quality_samples_media_metadata_check",
        ),
        (
            "video_quality_samples",
            "video_quality_samples_media_metadata_check",
        ),
    ] {
        assert_constraint_install_is_precise(&runtime_schema, table, constraint);
    }

    for (table, constraint) in [
        ("loop_samples", "loop_samples_media_metadata_check"),
        (
            "image_quality_samples",
            "image_quality_samples_media_metadata_check",
        ),
        (
            "animated_image_quality_samples",
            "animated_image_quality_samples_media_metadata_check",
        ),
        (
            "video_quality_samples",
            "video_quality_samples_media_metadata_check",
        ),
    ] {
        assert_constraint_install_is_precise(&migration_schema, table, constraint);
    }
}

const EXPLORE_DELIVERY_OR_GATE: &str = "quality_passed.is_passed() ||";

fn assert_no_explore_quality_size_or_gate(rel_path: &str) {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(&root, rel_path))
        .unwrap_or_else(|err| panic!("read {rel_path}: {err:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !content.contains(EXPLORE_DELIVERY_OR_GATE),
        "{rel_path} must not use quality_passed || size_target_met; use pipeline_acceptable"
    );
    assert!(
        !content.contains("quality_passed.is_passed() || explore_result.size_target_met"),
        "{rel_path} must not OR quality_passed with size_target_met"
    );
}

#[test]
fn vid_explore_delivery_paths_do_not_use_quality_or_size_or_gate() {
    assert_no_explore_quality_size_or_gate("crates/vid/src/conversion_api.rs");
    assert_no_explore_quality_size_or_gate("crates/vid/src/animated_image.rs");
}

#[test]
fn loop_intent_layer7_inference_log_does_not_borrow_tree_posterior() {
    let root = workspace_root();
    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in [
        "fn inference_used_layer7_policy",
        "fn resolve_legacy_mode",
        "self.log_inference(&tree, &verdict)",
        "layer7_policy_null_posteriors",
        "let tree_probability = if layer7_policy",
        "let fallback_probability = if layer7_policy",
    ] {
        assert!(
            loop_intent.contains(required),
            "loop_intent.rs must keep Layer 7 inference_log contract; missing `{required}`"
        );
    }
    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("runtime_final_probability"),
        "database.rs must persist runtime_final_probability in audit-only signal_snapshot"
    );
}

#[test]
fn media_conversion_session_fixes_no_silent_fabrication() {
    let root = workspace_root();
    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let quality_db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_quality_db.rs",
    ))
    .expect("image_quality_db.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let analyzer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    // ── Loop intent: supplementary KNN is telemetry-only ─────────────────────
    assert!(
        loop_intent.contains("run_supplementary_knn_telemetry"),
        "loop_intent must probe corpus health when tree exits before Layer 6"
    );
    assert!(
        loop_intent.contains("knn_telemetry_lookup_succeeded"),
        "loop_intent must use dedicated knn_telemetry_* fields"
    );
    let supplementary_scope = loop_intent
        .split("fn run_supplementary_knn_telemetry")
        .nth(1)
        .and_then(|tail| tail.split("\n    fn ").next())
        .expect("run_supplementary_knn_telemetry body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !supplementary_scope.contains("knn_lookup_succeeded"),
        "supplementary KNN must not overwrite decision-path knn_lookup_succeeded"
    );
    assert!(
        !supplementary_scope.contains("capture_knn_tracking"),
        "supplementary KNN must not populate decision-path KNN posteriors"
    );
    assert!(
        supplementary_scope.contains("lookup_meta"),
        "supplementary KNN must clone meta for lookup without mutating session meta"
    );
    assert!(
        !supplementary_scope.contains("deep_refine_meta(&mut self.mutable_meta"),
        "supplementary KNN must not refine session meta after tree verdict"
    );

    // ── Image quality embed: missing PSNR/SSIM → NaN (zero-tolerance; gate
    // 备案不豁免) ─
    let quality_db_prod = production_scope(&quality_db);
    assert!(
        gate.contains("quality_embedding_optional_f64_or_zero"),
        "gate may export legacy helper name (implementation must use NaN not 0.0)"
    );
    assert!(
        quality_db_prod.contains("quality_embed_measured_dimension_f32"),
        "image_quality_db must encode absent PSNR/SSIM as NaN via \
         quality_embed_measured_dimension_f32"
    );
    assert!(
        !quality_db_prod.contains("quality_embedding_optional_f64_or_zero"),
        "image_quality_db production must not call gate 0.0 sentinel helper"
    );
    assert!(
        !quality_db_prod.contains("infer_quality_embedding_psnr_ssim"),
        "image_quality_db must not fabricate PSNR/SSIM in production"
    );
    assert!(
        !quality_db_prod.contains("return (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)"),
        "jpeg_sidecar must not fabricate four 0.0 slots when JPEG analysis is absent"
    );
    assert!(
        quality_db_prod.contains("matches!(index, 12 | 17 | 18 | 19 | 20)"),
        "embed optional slots must include JPEG sidecar indices 19–20"
    );
    assert!(
        !quality_db_prod.contains("fn unit_interval_f32")
            || !quality_db_prod.contains("} else {\n        0.0\n    }"),
        "unit_interval_f32 must not collapse non-finite inputs to 0.0"
    );
    assert!(
        !quality_db.contains("probe_optional_f64_or_zero"),
        "image_quality_db must not audit-spam expected-missing embed dims"
    );
    let analyzer_prod = production_scope(&analyzer);
    assert!(
        !analyzer_prod.contains("infer_quality_embedding_psnr_ssim"),
        "image_analyzer must not fabricate PSNR/SSIM from entropy/defaults in production"
    );
    assert!(
        !analyzer_prod.contains("estimate_psnr_from_quality("),
        "image_analyzer must not map JPEG Q tables into analysis.psnr/ssim in production"
    );
    assert!(
        analyzer_prod.contains("let (psnr, ssim) = (None, None)"),
        "image_analyzer ingest must leave psnr/ssim unset without reference transcode"
    );

    // ── Inference JSON: sparse optional fields policy-silent (M117) ───────────
    assert!(
        database.contains("json_inference_optional_bool_or_null")
            && database.contains("knn_lookup_succeeded"),
        "build_signal_snapshot must use policy-silent bool JSON for knn_lookup_succeeded"
    );
    assert!(
        database.contains("knn_telemetry_lookup_succeeded"),
        "build_signal_snapshot must expose knn_telemetry_* separately from decision-path KNN"
    );
    assert!(
        !database.contains(
            "\"knn_lookup_succeeded\": crate::media_conversion_gate::json_optional_bool_or_null"
        ),
        "knn_lookup_succeeded must not use strict-audit json_optional_bool_or_null"
    );

    // ── M118: no synthesized loop percentiles on production reference path ───
    let prod_db = production_scope(&database);
    assert!(
        !prod_db.contains("fill_missing_percentiles_from_moments()"),
        "production database.rs must not synthesize loop profile percentiles"
    );
    assert!(
        !prod_db.contains("merge_duration_distribution_from_collection"),
        "production must not patch collection aggregates into duration percentiles"
    );
    assert!(
        database.contains("duration_has_empirical_percentiles"),
        "LoopReferenceProfile must expose duration histogram provenance"
    );
    assert!(
        loop_intent.contains("reference.duration_has_empirical_percentiles"),
        "LoopThresholds must gate on empirical duration percentiles only"
    );
    assert!(
        loop_intent.contains("duration_p90_from_samples"),
        "LoopThresholds must gate collection P90 on sample provenance (M219)"
    );
}

#[test]
fn media_conversion_phase2_embed_and_inference_json_hardening_m216() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let animated = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/animated_image_quality_features.rs",
    ))
    .expect("animated_image_quality_features.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let video_quality = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_quality_features.rs",
    ))
    .expect("video_quality_features.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let analyzer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    for sym in [
        "quality_embedding_optional_unit_interval_f32",
        "quality_embedding_optional_f64_or_zero",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M216)");
    }

    let animated_prod = production_scope(&animated);
    assert!(
        animated_prod.contains("quality_embedding_optional_unit_interval_f32"),
        "animated embed vector must use quality embedding sentinels (M216)"
    );
    assert!(
        !animated_prod.contains("probe_optional_f64_or_zero"),
        "animated production path must not audit-spam optional embed dims (M216)"
    );

    let video_prod = production_scope(&video_quality);
    assert!(
        video_prod.contains("quality_embedding_optional_unit_interval_f32"),
        "video_quality embed must use quality embedding sentinels (M216)"
    );
    assert!(
        !video_prod.contains("probe_optional_f64_or_zero"),
        "video_quality production path must not audit-spam optional bit_depth embed (M216)"
    );

    let db_prod = production_scope(&database);
    assert!(
        db_prod.contains("json_inference_optional_bool_or_null")
            && db_prod.contains("layer6b_resolved"),
        "inference snapshot layer6b_resolved must be policy-silent (M216)"
    );
    assert!(
        db_prod.contains("json_inference_optional_string_or_null")
            && db_prod.contains("resolution_path"),
        "inference snapshot resolution_path must be policy-silent (M216)"
    );
    assert!(
        gate.contains("loop_profile_feature_absent"),
        "missing loop feature_stats keys must be audited, not silently defaulted (M216)"
    );

    let analyzer_prod = production_scope(&analyzer);
    assert!(
        !analyzer_prod.contains("estimate_psnr_from_quality("),
        "production image_analyzer must not map JPEG Q tables into analysis.psnr/ssim"
    );
    assert!(
        analyzer_prod.contains("let (psnr, ssim) = (None, None)"),
        "image_analyzer ingest must leave psnr/ssim unset without reference transcode"
    );
    assert!(
        analyzer.contains("#[cfg(test)]\nmod tests")
            && analyzer.contains("fn estimate_psnr_from_quality"),
        "JPEG Q→PSNR mapping may exist only in unit tests"
    );

    let quality_model = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_regression_model.rs",
    ))
    .expect("quality_regression_model.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        quality_model.contains("psnr_measured") && quality_model.contains("ssim_measured"),
        "LightGBM payload must expose PSNR/SSIM measurement provenance"
    );
    assert!(
        quality_model.contains("embed_measurement_slot_json")
            && quality_model.contains("embedding_017")
            && quality_model.contains("embedding_018"),
        "LightGBM must gate embed 17/18 on measurement via embed_measurement_slot_json (M216/M235)"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let build_result_body = gpu
        .split("fn build_result")
        .nth(1)
        .and_then(|rest| rest.split("fn search_anchor_crf").next())
        .expect("gpu build_result body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        build_result_body.contains("lossless_integrity_ok"),
        "GPU coarse build_result must gate lossless GIF on integrity, not fabricated SSIM"
    );
    assert!(
        build_result_body.contains("(None, Some(integrity_ok))"),
        "GPU coarse build_result must return integrity outcome without fabricated SSIM"
    );
    assert!(
        !build_result_body
            .contains("Some(1.0_f64)\n        } else {\n            calculate_ssim_enhanced"),
        "GPU coarse build_result must not invent SSIM=1.0 on the lossless integrity branch"
    );
}

#[test]
fn media_conversion_loop_duration_percentile_policy_m217() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 217),
        "contract must document M217"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    let duration_policy = gate
        .split("pub fn loop_duration_or_fallback_policy")
        .nth(1)
        .and_then(|body| body.split("pub fn loop_scaled_duration_percentile").next())
        .expect("loop_duration_or_fallback_policy body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        duration_policy.contains("if profile_percentiles_available")
            && duration_policy.contains("if let Some(value) = primary"),
        "loop_duration_or_fallback_policy must gate primary on empirical percentiles (M217)"
    );
    let scaled_policy = gate
        .split("pub fn loop_scaled_duration_percentile_or_fallback_policy")
        .nth(1)
        .and_then(|body| {
            body.split("pub fn algorithm_env_flag_enabled_or_default")
                .next()
        })
        .expect("loop_scaled_duration_percentile policy body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        scaled_policy.contains("if !profile_percentiles_available"),
        "scaled duration policy must ignore synthetic percentiles without histogram (M217)"
    );

    let thresholds = loop_intent
        .split("fn from_reference_profile(reference: &LoopReferenceProfile)")
        .nth(1)
        .and_then(|body| body.split("fn get_feature_weight").next())
        .expect("LoopThresholds::from_profile body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        thresholds.contains("duration_has_empirical_percentiles"),
        "LoopThresholds must thread duration_has_empirical_percentiles into policy helpers (M217)"
    );
    for call in [
        "loop_duration_or_fallback_policy(",
        "loop_scaled_duration_percentile_or_fallback_policy(",
        "loop_collection_duration_p90_or_baseline(",
        "loop_duration_p50_or_capped_p75_policy(",
    ] {
        assert!(
            thresholds.contains(call),
            "LoopThresholds must use policy-gated duration helpers: {call}"
        );
    }
    let loop_prod = production_scope(&loop_intent);
    let without_policy = loop_prod
        .replace("loop_duration_or_fallback_policy", "")
        .replace("loop_scaled_duration_percentile_or_fallback_policy", "");
    assert!(
        !without_policy.contains("loop_duration_or_fallback("),
        "loop_intent production must not call non-policy loop_duration_or_fallback (M218)"
    );
    assert!(
        !without_policy.contains("loop_scaled_duration_percentile_or_fallback("),
        "loop_intent production must not call non-policy scaled duration helper (M218)"
    );
}

#[test]
fn media_conversion_loop_profile_strips_synthetic_duration_percentiles_m218() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 218),
        "contract must document M218"
    );

    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("fn strip_non_empirical_duration_percentiles"),
        "database must strip synthetic duration percentiles when histogram absent (M218)"
    );
    let prod_db = production_scope(&database);
    assert!(
        prod_db.contains("strip_non_empirical_duration_percentiles(&mut profile.duration)"),
        "profile build must strip non-empirical duration slots (M218)"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("collection_duration_p90_feature_stats"),
        "feature_stats duration_p90 fallback must be audited (M218)"
    );
}

#[test]
fn media_conversion_collection_duration_p90_provenance_m219() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 219),
        "contract must document M219"
    );

    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("duration_p90_from_samples"),
        "GlobalCollectionStats must track sample-derived duration_p90 (M219)"
    );
    let prod_db = production_scope(&database);
    assert!(
        prod_db.contains("duration_p90_from_samples: duration_p90_empirical.is_some()"),
        "collection stats build must set duration_p90_from_samples from empirical samples (M219)"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("collection_duration_p90_discarded"),
        "gate must discard feature-stats-only collection P90 without provenance (M219)"
    );
    assert!(
        gate.contains("loop_duration_p50_or_capped_p75_policy"),
        "gate must expose policy-gated p50/p75 median helper (M219)"
    );

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let thresholds = loop_intent
        .split("fn from_reference_profile(reference: &LoopReferenceProfile)")
        .nth(1)
        .and_then(|body| body.split("fn get_feature_weight").next())
        .expect("LoopThresholds::from_profile body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        thresholds.contains("duration_p90_from_samples"),
        "LoopThresholds must pass collection P90 provenance into gate helpers (M219)"
    );
    assert!(
        !thresholds.contains("loop_duration_p50_or_capped_p75("),
        "LoopThresholds must not call non-policy p50/p75 helper (M219)"
    );
}

const MC_FORBIDDEN_DECISION_CHAIN_FABRICATION: &[&str] = &[
    "infer_quality_embedding_psnr_ssim",
    "fill_missing_percentiles_from_moments()",
    "merge_duration_distribution_from_collection",
    "probe_optional_f64_or_zero(",
    "estimate_psnr_from_quality(",
    "estimate_ssim_from_quality(",
];

/// Explore confidence must come from measured evidence
/// (`measured_exploration_confidence*`).
const MC_FORBIDDEN_EXPLORE_CONFIDENCE_FABRICATION: &[&str] = &[
    "sampling_coverage = Some(1.0_f64)",
    "GPU_SEARCH_PREDICTION_ACCURACY_BASE",
    "prediction_accuracy = Some(crate::constants::GPU_SEARCH_PREDICTION_ACCURACY",
    "margin_safety: Some(0.0_f64)",
    "margin_safety: Some(0.0)",
];

/// Invented perfect SSIM or unconditional "approx" labeling (M229 — outside
/// M222–M228 lists).
const MC_FORBIDDEN_SILENT_MEASUREMENT_FORGERY: &[&str] = &[
    "ssim: Some(1.0_f64)",
    "ssim: Some(1.0)",
    "psnr: Some(1.0_f64)",
    "psnr: Some(1.0)",
    "ms_ssim_score: Some(1.0_f64)",
    "ms_ssim_score: Some(1.0)",
    "SSIM={ssim:.4}, approx.)",
];

/// Neutral-prior / default-score injection on inference paths (M233; aligns
/// with `algorithm_audit`).
const MC_FORBIDDEN_NUMERIC_PRIOR_INJECTION: &[&str] = &[
    "unwrap_or(0.5",
    "map_or(0.5,",
    "unwrap_or_else(|| 0.5",
    "seal_unit_probability_or",
    "quality_probability_or",
    "LOOP_INTENT_DEFAULT_KNN_CONFIDENCE",
];

/// Legacy explore confidence literals must not reappear in production code
/// (M234).
const MC_FORBIDDEN_EXPLORE_CONFIDENCE_LITERAL_USE: &[&str] = &[
    "EXPLORE_CONFIDENCE_HIGH",
    "EXPLORE_CONFIDENCE_NORMAL",
    "EXPLORE_CONFIDENCE_MEDIUM",
    "EXPLORE_CONFIDENCE_LOW",
];

/// Syntax-bypass hardening: quality-estimate helpers + perfect-score literals
/// (M242).
const MC_FORBIDDEN_SYNTAX_BYPASS_M242: &[&str] = &[
    "confidence: Some(1.0)",
    "confidence: Some(1.00)",
    "unwrap_or(1.0_f64)",
    "unwrap_or(1.0)",
    "map_or(1.0,",
    "map_or(1.0_f64,",
    "prediction_accuracy: Some(1.0",
    "psnr: Some(0.0",
    "ssim: Some(0.0",
];

/// Algorithm-audit parity + loop-verdict fabrication literals (M243).
const MC_FORBIDDEN_SYNTAX_BYPASS_M243: &[&str] = &[
    "unwrap_or_else(|| 0.0",
    "Verdict::LoopStrong(_) => Some(1.0)",
    "Verdict::LoopWeak(_) | Verdict::Error(_) => Some(0.0)",
    "confidence: Some(0.95)",
    "confidence: Some(0.9)",
    "return Some(1.0);",
    "return Some(0.0);",
];

/// Algorithm-audit full literal parity + dev synthetic fixture guard (M244).
const MC_FORBIDDEN_SYNTAX_BYPASS_M244: &[&str] = &[
    "unwrap_or(f64::NAN",
    "unwrap_or(knn",
    "_probability_raw",
    "_finite_scalar_raw",
    "confidence: Some(0.85)",
    "confidence: Some(0.75)",
    "confidence: Some(0.8)",
    "confidence: Some(0.65)",
    "confidence: Some(0.7)",
    "confidence: Some(0.6)",
    "confidence: Some(0.3)",
    "confidence: 0.0,",
    "confidence: 0.5,",
    "#[path = \"edge/",
    "pub mod synth_",
];

/// Final `algorithm_audit::FORBIDDEN_SUBSTRINGS` parity (M245).
const MC_FORBIDDEN_SYNTAX_BYPASS_M245: &[&str] = &[
    "0.5 neutral prior",
    "using 0.5 neutral",
    "preserving raw",
    "unwrap_or_else(|| {\n            crate::constants::LOOP_INTENT_DEFAULT",
];

/// Training/export hardcoding of absent embed 17/18 as numeric zero (M246
/// tier-C closure).
const MC_FORBIDDEN_SYNTAX_BYPASS_M246: &[&str] = &[
    "embedding_017\": 0.0",
    "embedding_018\": 0.0",
    "\"embedding_017\": 0.0",
    "\"embedding_018\": 0.0",
];

/// Full `algorithm_audit` inference surface (SSOT list lives in
/// `comprehensive_weakness_audit.rs` too).
const PRODUCTION_SCOPE_FABRICATION_TARGETS_M246: &[&str] = &[
    "crates/foundation/src/algorithm_seal.rs",
    "crates/foundation/src/algorithm_runtime.rs",
    "crates/foundation/src/loop_intent.rs",
    "crates/foundation/src/database.rs",
    "crates/foundation/src/image_quality_db.rs",
    "crates/foundation/src/quality_regression_model.rs",
    "crates/foundation/src/scenario_quality_lookup.rs",
    "crates/foundation/src/video_explorer.rs",
    "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    "crates/foundation/src/gpu_accel.rs",
    "crates/foundation/src/jxl_explorer.rs",
    "crates/foundation/src/quality_matcher.rs",
    "crates/foundation/src/multi_scenario_db.rs",
    "crates/foundation/src/image_quality_detector.rs",
    "crates/foundation/src/image_detection.rs",
    "crates/foundation/src/image_jpeg_analysis.rs",
    "crates/foundation/src/video_detection.rs",
    "crates/foundation/src/video_quality_detector.rs",
    "crates/foundation/src/explore_strategy.rs",
    "crates/foundation/src/hdr.rs",
    "crates/foundation/src/conversion.rs",
    "crates/foundation/src/database_vector.rs",
    "crates/foundation/src/animated_image_quality_features.rs",
    "crates/foundation/src/video_quality_features.rs",
    "crates/foundation/src/analysis_cache.rs",
    "crates/foundation/src/training_tier_audit.rs",
    "crates/foundation/src/c_api.rs",
    "crates/foundation/src/logging.rs",
    "crates/foundation/src/scenario.rs",
    "crates/foundation/src/video_explorer/stream_analysis.rs",
    "crates/foundation/src/video_explorer/precision.rs",
    "crates/foundation/src/image_metrics.rs",
    "crates/foundation/src/image_analyzer.rs",
    "crates/foundation/src/quality_verifier_enhanced.rs",
    // `media_conversion_gate.rs` is SSOT for audited `*_optional_*_or_zero` helpers — excluded.
];

/// Copy of `algorithm_audit::FORBIDDEN_SUBSTRINGS` — each entry must be covered
/// by `mc_unified_fabrication_patterns()`.
const ALGORITHM_AUDIT_FORBIDDEN_PARITY_M245: &[&str] = &[
    "unwrap_or(0.5",
    "unwrap_or(f64::NAN",
    "unwrap_or(knn",
    "unwrap_or_else(|| 0.5",
    "unwrap_or_else(|| 0.0",
    "unwrap_or_else(|| {\n            crate::constants::LOOP_INTENT_DEFAULT",
    "_probability_raw",
    "_finite_scalar_raw",
    "0.5 neutral prior",
    "using 0.5 neutral",
    "preserving raw",
    "LOOP_INTENT_DEFAULT_KNN_CONFIDENCE",
    "seal_unit_probability_or",
    "quality_probability_or",
    "confidence: 0.0,",
    "confidence: 0.5,",
    "map_or(0.5,",
    "Verdict::LoopStrong(_) => Some(1.0)",
    "Verdict::LoopWeak(_) | Verdict::Error(_) => Some(0.0)",
    "return Some(1.0);",
    "return Some(0.0);",
    "confidence: Some(0.85)",
    "confidence: Some(0.75)",
    "confidence: Some(0.7)",
    "confidence: Some(0.65)",
    "confidence: Some(0.6)",
    "confidence: Some(0.3)",
    "confidence: Some(0.8)",
    "confidence: Some(0.9)",
    "confidence: Some(0.95)",
    "confidence: Some(1.0)",
    "EXPLORE_CONFIDENCE_HIGH",
    "EXPLORE_CONFIDENCE_NORMAL",
    "EXPLORE_CONFIDENCE_MEDIUM",
    "EXPLORE_CONFIDENCE_LOW",
    "unwrap_or(1.0_f64)",
    "unwrap_or(1.0)",
];

/// PSNR→SSIM estimate is only allowed on audited `explore_strategy` +
/// `ssim_mapping` (M234).
const MC_FORBIDDEN_PSNR_TO_SSIM_OUTSIDE_GATEWAY: &str = "psnr_to_ssim_estimate";

/// Repo `scripts/*.sh` fabrication scan union (M235 decision chain + M236
/// explore/measurement).
fn mc_repo_shell_fabrication_patterns() -> Vec<&'static str> {
    let mut patterns = mc_unified_fabrication_patterns();
    for list in [
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
        MC_FORBIDDEN_SILENT_MEASUREMENT_FORGERY,
    ] {
        for pattern in list {
            if !patterns.contains(pattern) {
                patterns.push(pattern);
            }
        }
    }
    patterns
}

/// Union of all CI forbidden fabrication pattern lists (M232 closure; M233
/// numeric priors; M236 M234 literals).
fn mc_unified_fabrication_patterns() -> Vec<&'static str> {
    let mut patterns: Vec<&'static str> = Vec::new();
    for list in [
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
        MC_FORBIDDEN_EXPLORE_CONFIDENCE_FABRICATION,
        MC_FORBIDDEN_SILENT_MEASUREMENT_FORGERY,
        MC_FORBIDDEN_NUMERIC_PRIOR_INJECTION,
        MC_FORBIDDEN_EXPLORE_CONFIDENCE_LITERAL_USE,
        MC_FORBIDDEN_SYNTAX_BYPASS_M242,
        MC_FORBIDDEN_SYNTAX_BYPASS_M243,
        MC_FORBIDDEN_SYNTAX_BYPASS_M244,
        MC_FORBIDDEN_SYNTAX_BYPASS_M245,
        MC_FORBIDDEN_SYNTAX_BYPASS_M246,
    ] {
        for pattern in list {
            if !patterns.contains(pattern) {
                patterns.push(pattern);
            }
        }
    }
    patterns
}

fn production_scope_pattern_offenders(
    root: &Path,
    rel_paths: &[&str],
    patterns: &[&str],
) -> Vec<String> {
    let mut hits = Vec::new();
    for rel in rel_paths {
        let path = join_legacy_aware(&root, rel);
        let content =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for pattern in patterns {
            if prod.contains(pattern) {
                hits.push(format!("{rel}: production scope contains `{pattern}`"));
            }
        }
    }
    hits
}

fn workspace_unified_fabrication_rust_targets(root: &Path) -> Vec<PathBuf> {
    let mut files = workspace_whole_repo_rust_production_files(root);
    files.extend(workspace_bin_rust_files(root));
    files.extend(workspace_fuzz_rust_files(root));
    files.sort();
    files.dedup();
    files
}

/// Pattern-catalog integration tests (contain forbidden substrings by design).
fn fabrication_pattern_catalog_rust_file(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        name == std::ffi::OsStr::new("test_real_silent_fallbacks.rs")
            || name == std::ffi::OsStr::new("test_silent_numeric_fallbacks.rs")
            || name == std::ffi::OsStr::new("comprehensive_weakness_audit.rs")
    })
}

/// `crates/dev/src/tests/**/*.rs` excluding pattern-catalog harness files (M240
/// whole-repo parity).
fn workspace_dev_integration_test_rust_files(root: &Path) -> Vec<PathBuf> {
    let tests_dir = root.join("crates/dev/src/tests");
    let mut files = Vec::new();
    if tests_dir.is_dir() {
        for entry in WalkDir::new(&tests_dir).into_iter().filter_map(Result::ok) {
            let path = entry.path().to_path_buf();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
                && !fabrication_pattern_catalog_rust_file(&path)
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Whole-repository Rust fabrication scan surface (M240: production + bins +
/// fuzz + dev integration tests).
fn workspace_whole_repository_rust_fabrication_targets(root: &Path) -> Vec<PathBuf> {
    let mut files = workspace_unified_fabrication_rust_targets(root);
    files.extend(workspace_dev_integration_test_rust_files(root));
    files.sort();
    files.dedup();
    files
}

fn unified_fabrication_offenders(root: &Path) -> Vec<String> {
    let patterns = mc_unified_fabrication_patterns();
    let mut hits = silent_fabrication_offenders_in_files(
        root,
        &workspace_whole_repository_rust_fabrication_targets(root),
        &patterns,
    );
    let py_hits = silent_fabrication_offenders_in_files(
        root,
        &workspace_dev_scripts_py_files(root),
        &patterns,
    );
    hits.extend(py_hits);
    hits
}

fn fabrication_scan_skip_file(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        // Gate is scanned with line-level exemptions (M249); never blanket-skipped.
        name == std::ffi::OsStr::new("ssim_mapping.rs")
            || name == std::ffi::OsStr::new("algorithm_audit.rs")
            || name == std::ffi::OsStr::new("constants.rs")
            || name == std::ffi::OsStr::new("media_conversion_delivery_heatmap.py")
    })
}

/// Gate centralizes disclosed fallbacks; unified scan allows definitions +
/// audit strings only.
fn gate_unified_fabrication_line_exempt(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.starts_with("pub fn ")
        || trimmed.starts_with("pub const fn ")
        || trimmed.starts_with("const fn ")
    {
        return true;
    }
    if trimmed.contains("_audit(")
        || trimmed.contains("_batch_audit(")
        || trimmed.contains("probe_quality_batch_audit(")
        || trimmed.contains("delivery_progress_batch_audit(")
        || trimmed.contains("explore_precheck_batch_audit(")
    {
        return true;
    }
    false
}

fn is_fabrication_scan_comment_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("//")
        || trimmed.starts_with("///")
        || trimmed.starts_with('*')
        || trimmed.starts_with("/*")
}

fn code_line_fabrication_offenders(
    root: &Path,
    files: &[PathBuf],
    patterns: &[&str],
) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in files {
        if fabrication_scan_skip_file(file) {
            continue;
        }
        let is_gate = file
            .file_name()
            .is_some_and(|n| n == std::ffi::OsStr::new("media_conversion_gate.rs"));
        let content = fs::read_to_string(file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        let rel = file.strip_prefix(root).unwrap_or(file);
        for (idx, line) in prod.lines().enumerate() {
            if is_fabrication_scan_comment_line(line) {
                continue;
            }
            if is_gate && gate_unified_fabrication_line_exempt(line) {
                continue;
            }
            for pattern in patterns {
                if line.contains(pattern) {
                    offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
                }
            }
        }
    }
    offenders
}

fn python_tooling_fabrication_offenders(root: &Path, patterns: &[&str]) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in workspace_dev_scripts_py_files(root) {
        if fabrication_scan_skip_file(&file) {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let lines: Vec<&str> = content.lines().collect();
        let rel = file.strip_prefix(root).unwrap_or(&file);
        for (idx, line) in lines.iter().enumerate() {
            if is_fabrication_scan_comment_line(line) {
                continue;
            }
            let start = idx.saturating_sub(15);
            let in_test_context = lines[start..idx].iter().any(|l| {
                l.contains("def test_")
                    || l.contains("pytest")
                    || l.contains("if __name__ == \"__main__\"")
            });
            if in_test_context {
                continue;
            }
            for pattern in patterns {
                if line.contains(pattern) {
                    offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
                }
            }
        }
    }
    offenders
}

fn workspace_repo_shell_scripts(root: &Path) -> Vec<PathBuf> {
    let scripts = root.join("scripts");
    let mut files = Vec::new();
    if scripts.is_dir() {
        for entry in std::fs::read_dir(&scripts)
            .unwrap_or_else(|e| panic!("read {}: {e}", scripts.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("sh"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Repo automation surfaces beyond `crates/dev/scripts/*.py` (M237
/// known-weakness closure).
fn workspace_repo_automation_files(root: &Path) -> Vec<PathBuf> {
    let mut files = workspace_repo_shell_scripts(root);
    let just = root.join("justfile");
    if just.is_file() {
        files.push(just);
    }
    for rel in [".github/scripts", "crates/dev/scripts"] {
        let dir = join_legacy_aware(&root, rel);
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("sh"))
            {
                files.push(path);
            }
        }
    }
    let workflows = root.join(".github/workflows");
    if workflows.is_dir() {
        for entry in std::fs::read_dir(&workflows)
            .unwrap_or_else(|e| panic!("read {}: {e}", workflows.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|ext| {
                    ext == std::ffi::OsStr::new("yml") || ext == std::ffi::OsStr::new("yaml")
                })
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn is_automation_scan_comment_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('#')
        || trimmed.starts_with("//")
        || trimmed.starts_with("///")
        || trimmed.starts_with('*')
        || trimmed.starts_with("/*")
}

fn automation_surface_fabrication_offenders(
    root: &Path,
    files: &[PathBuf],
    patterns: &[&str],
) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in files {
        if fabrication_scan_skip_file(file) {
            continue;
        }
        let content = fs::read_to_string(file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let rel = file.strip_prefix(root).unwrap_or(file);
        for (idx, line) in content.lines().enumerate() {
            if is_automation_scan_comment_line(line) {
                continue;
            }
            for pattern in patterns {
                if line.contains(pattern) {
                    offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
                }
            }
        }
    }
    offenders
}

fn workspace_migration_sql_files(root: &Path) -> Vec<PathBuf> {
    let migrations = root.join("crates/dev/src/migrations");
    let mut files = Vec::new();
    if migrations.is_dir() {
        for entry in std::fs::read_dir(&migrations)
            .unwrap_or_else(|e| panic!("read {}: {e}", migrations.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
            .filter_map(Result::ok)
        {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("sql"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn workspace_stderr_line_unwrap_offenders(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in workspace_unified_fabrication_rust_targets(root) {
        if file
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("media_conversion_gate.rs"))
        {
            continue;
        }
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        let rel = file.strip_prefix(root).unwrap_or(&file);
        for (idx, line) in prod.lines().enumerate() {
            if is_fabrication_scan_comment_line(line) {
                continue;
            }
            if MC_FORBIDDEN_M176_STDERR_LINE
                .iter()
                .any(|pattern| line.contains(pattern))
            {
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

fn silent_fabrication_offenders_in_files(
    root: &Path,
    files: &[PathBuf],
    patterns: &[&str],
) -> Vec<String> {
    code_line_fabrication_offenders(root, files, patterns)
}

fn decision_chain_fabrication_offenders(root: &Path) -> Vec<String> {
    silent_fabrication_offenders_in_files(
        root,
        &production_rust_files(root),
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
    )
}

fn repo_wide_explore_confidence_fabrication_offenders(root: &Path) -> Vec<String> {
    silent_fabrication_offenders_in_files(
        root,
        &workspace_all_crate_production_rust_files(root),
        MC_FORBIDDEN_EXPLORE_CONFIDENCE_FABRICATION,
    )
}

fn repo_wide_silent_fabrication_offenders(root: &Path) -> Vec<String> {
    silent_fabrication_offenders_in_files(
        root,
        &workspace_all_crate_production_rust_files(root),
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
    )
}

#[test]
fn media_conversion_decision_chain_anti_fabrication_closure_m220() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 220),
        "contract must document M220 trust boundary"
    );
    assert!(
        contract.contains("Does not") && contract.contains("100%"),
        "M220 must document that closure is decision-chain scoped, not universal 100%"
    );

    let hdr = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/hdr.rs"))
        .expect("hdr.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let hdr_prod = production_scope(&hdr);
    assert!(
        hdr_prod.contains("delivery_metadata_batch_audit")
            && hdr_prod.contains("hdr_bt709_cicp_inference"),
        "HDR bt709 CICP inference must be audited, not silent (M220)"
    );

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let loop_prod = production_scope(&loop_intent);
    let without_policy = loop_prod
        .replace("loop_duration_or_fallback_policy", "")
        .replace("loop_scaled_duration_percentile_or_fallback_policy", "")
        .replace("loop_duration_p50_or_capped_p75_policy", "")
        .replace("loop_collection_duration_p90_or_baseline", "");
    for ban in [
        "loop_duration_or_fallback(",
        "loop_scaled_duration_percentile_or_fallback(",
        "loop_duration_p50_or_capped_p75(",
    ] {
        assert!(
            !without_policy.contains(ban),
            "loop_intent production must not use non-policy duration helpers: {ban}"
        );
    }

    let hits = decision_chain_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "decision-chain fabrication patterns must be absent from production \
         img/vid/foundation:\n{}",
        hits.join("\n")
    );

    let runtime = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/algorithm_runtime.rs",
    ))
    .expect("algorithm_runtime.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        runtime.contains("quality_inference_audit_only_mode"),
        "quality inference must support audit-only DB rows by default (M220)"
    );
}

#[test]
fn media_conversion_collection_duration_trust_m221() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 221),
        "contract must document M221 collection duration trust"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("collection_value_trusted")
            && gate.contains("collection_field_discarded")
            && gate.contains("strip_distribution_percentile_slots"),
        "gate must discard untrusted collection fields and strip fallback percentiles (M221)"
    );

    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_db = production_scope(&database);
    assert!(
        prod_db.contains("COLLECTION_BASELINE_TRUSTED")
            && prod_db.contains("loop_collection_secs_or_baseline_policy"),
        "KnnDistributionProfile::default must use trusted baseline policy for collection duration \
         fields (M221)"
    );
    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&loop_intent).contains("loop_collection_duration_p90_or_baseline"),
        "loop thresholds must gate collection P90 via provenance helper (M221)"
    );
    assert!(
        prod_db.contains("duration_stats_from_samples"),
        "GlobalCollectionStats must track sample-derived min/avg/max duration (M221)"
    );
}

#[test]
fn media_conversion_repo_wide_silent_fabrication_scan_m222() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 222),
        "contract must document M222 repo-wide silent-fabrication scan"
    );
    assert!(
        contract.contains("workspace_all_crate_production_rust_files")
            || contract.contains("crates/*/src"),
        "M222 must document full workspace crate src scan scope"
    );

    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn workspace_all_crate_production_rust_files"),
        "contract test harness must expose workspace-wide production scan (M222)"
    );

    let decision_files = production_rust_files(&root);
    let all_files = workspace_all_crate_production_rust_files(&root);
    assert!(
        all_files.len() > decision_files.len(),
        "repo-wide scan must cover more than img/vid/foundation only (decision={}, all={})",
        decision_files.len(),
        all_files.len()
    );

    let hits = repo_wide_silent_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "repo-wide silent fabrication patterns must be absent from all crate src production \
         scopes:\n{}",
        hits.join("\n")
    );

    let headless = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/headless_gif_regression.rs",
    ))
    .expect("headless_gif_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        headless.contains("build_synthetic_headless_sticker_gif"),
        "headless GIF regression must not depend on missing on-disk fixture (M222 CI)"
    );
}

#[test]
fn media_conversion_contract_m1_m222_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 222),
        "contract must document M222"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_repo_wide_silent_fabrication_scan_m222"),
        "M222 dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_repo_wide_silent_fabrication_scan_m222"),
        "M222 row must reference repo-wide scan dev test"
    );
}

#[test]
fn media_conversion_decision_metrics_anti_fabrication_m223() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 223),
        "contract must document M223"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("warm_start_predicted_anchor"),
        "warm-start without cache must be audited (M223)"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let build_result_body = gpu
        .split("fn build_result")
        .nth(1)
        .and_then(|rest| rest.split("fn search_anchor_crf").next())
        .expect("gpu build_result body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        build_result_body.contains("lossless_integrity_ok"),
        "M223: lossless GIF must not fabricate measured SSIM"
    );
    assert!(
        !build_result_body
            .contains("Some(1.0_f64)\n        } else {\n            calculate_ssim_enhanced"),
        "M223: build_result must not invent SSIM=1.0 on integrity branch"
    );

    let quality_model = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_regression_model.rs",
    ))
    .expect("quality_regression_model.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        quality_model.contains("psnr_measured") && quality_model.contains("ssim_measured"),
        "M223: model payload must expose PSNR/SSIM measurement provenance"
    );
    assert!(
        quality_model.contains("serde_json::Value::Null"),
        "M223: absent PSNR/SSIM embed dims must be null for LightGBM, not forged 0.0"
    );
}

#[test]
fn media_conversion_collection_p90_non_sample_trust_m224() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 224),
        "contract must document M224"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("ignoring non-sample") && gate.contains("despite profile histogram"),
        "M224: profile histogram must not legitimize non-sample collection P90"
    );
    assert!(
        gate.contains(
            "fn loop_collection_duration_p90_discards_non_sample_when_profile_has_histogram"
        ),
        "M224: gate unit test must cover profile/non-sample collection P90 boundary"
    );
    let optional_body = gate
        .split("pub fn loop_optional_secs_or_baseline")
        .nth(1)
        .and_then(|tail| {
            tail.split("pub fn loop_collection_secs_or_baseline_policy")
                .next()
        })
        .expect("loop_optional_secs_or_baseline body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        optional_body.contains("false, true"),
        "loop_optional_secs_or_baseline must not assume empirical profile histogram (M224)"
    );
}

#[test]
fn media_conversion_bins_silent_fabrication_scan_m224() {
    let root = workspace_root();
    let bins = workspace_bin_rust_files(&root);
    assert!(
        bins.len() >= 20,
        "bin scan must cover crate bin targets (got {})",
        bins.len()
    );
    let hits = silent_fabrication_offenders_in_files(
        &root,
        &bins,
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
    );
    assert!(
        hits.is_empty(),
        "decision-chain fabrication patterns must be absent from all crate bin sources:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_contract_m1_m224_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 224),
        "contract must document M224"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_collection_p90_non_sample_trust_m224",
        "fn media_conversion_bins_silent_fabrication_scan_m224",
    ] {
        assert!(tests.contains(sym), "M224 dev test {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_bins_silent_fabrication_scan_m224"),
        "M224 row must reference bin silent-fabrication scan dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m223_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 223),
        "contract must document M223"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_decision_metrics_anti_fabrication_m223"),
        "M223 dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_decision_metrics_anti_fabrication_m223"),
        "M223 row must reference decision-metrics anti-fabrication dev test"
    );
}

#[test]
fn media_conversion_python_predict_null_embed_m225() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 225),
        "contract must document M225"
    );

    let script = fs::read_to_string(root.join("crates/dev/scripts/quality_regression_model.py"))
        .expect("quality_regression_model.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        script.contains("NULLABLE_EMBED_FEATURES")
            && script.contains("predict_feature_scalar")
            && script.contains("LIGHTGBM_MISSING_MEASUREMENT"),
        "Python predict must map null/absent PSNR/SSIM embed dims to NaN, not 0.0 (M225)"
    );
    assert!(
        script.contains("OPTIONAL_MEASUREMENT_FLAGS"),
        "Python predict must accept psnr_measured/ssim_measured provenance flags (M225)"
    );
    assert!(
        !script.contains("[[as_float(features.get(name), name) for name in FEATURE_NAMES]]"),
        "predict must use metadata feature_names + predict_feature_scalar (M225)"
    );

    let scripts = workspace_dev_scripts_py_files(&root);
    assert!(
        scripts.len() >= 5,
        "dev scripts scan must cover project Python entrypoints (got {})",
        scripts.len()
    );
    let hits = silent_fabrication_offenders_in_files(
        &root,
        &scripts,
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
    );
    assert!(
        hits.is_empty(),
        "decision-chain fabrication patterns must be absent from dev scripts:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_fuzz_silent_fabrication_scan_m226() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 226),
        "contract must document M226"
    );

    let fuzz = workspace_fuzz_rust_files(&root);
    assert!(
        fuzz.len() >= 5,
        "fuzz scan must cover dev fuzz targets (got {})",
        fuzz.len()
    );
    let hits = silent_fabrication_offenders_in_files(
        &root,
        &fuzz,
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
    );
    assert!(
        hits.is_empty(),
        "decision-chain fabrication patterns must be absent from fuzz targets:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_gpu_coarse_confidence_m227() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 227),
        "contract must document M227"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let build_result_body = gpu
        .split("fn build_result")
        .nth(1)
        .and_then(|rest| rest.split("fn search_anchor_crf").next())
        .expect("gpu build_result body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        build_result_body.contains("measured_exploration_confidence"),
        "M227: non-ultimate build_result must delegate confidence to \
         measured_exploration_confidence"
    );
    for ban in MC_FORBIDDEN_EXPLORE_CONFIDENCE_FABRICATION {
        assert!(
            !build_result_body.contains(ban),
            "M227: build_result must not contain explore confidence fabrication pattern: {ban}"
        );
    }
    assert!(
        build_result_body.contains("exploration_size_margin_from_output"),
        "M227: GPU coarse build_result must preserve measured size-headroom margin (not Some(0.0) \
         fabrication)"
    );
}

#[test]
fn media_conversion_repo_wide_explore_confidence_fabrication_m228() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 228),
        "contract must document M228"
    );

    let prod_hits = repo_wide_explore_confidence_fabrication_offenders(&root);
    assert!(
        prod_hits.is_empty(),
        "explore confidence fabrication patterns must be absent from all crate production src:\n{}",
        prod_hits.join("\n")
    );

    let bin_hits = silent_fabrication_offenders_in_files(
        &root,
        &workspace_bin_rust_files(&root),
        MC_FORBIDDEN_EXPLORE_CONFIDENCE_FABRICATION,
    );
    assert!(
        bin_hits.is_empty(),
        "explore confidence fabrication patterns must be absent from crate bins:\n{}",
        bin_hits.join("\n")
    );
}

#[test]
fn media_conversion_whole_repo_measurement_forgery_m229() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract.contains("Silent fabrication vs explicit fallback"),
        "contract must define silent fabrication vs explicit fallback (M229)"
    );
    assert!(
        contract.contains("不是弄虚作假") || contract.contains("Not fabrication"),
        "contract must document allowed explicit fallback/heuristic paths (M229)"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let append_log = gpu
        .split("fn append_result_log_lines")
        .nth(1)
        .and_then(|body| body.split("fn log_gpu_mapping").next())
        .expect("append_result_log_lines body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        append_log.contains("lossless integrity gate, SSIM not measured"),
        "M229: integrity pass must not summarize as quality failed when SSIM unmeasured"
    );
    assert!(
        append_log.contains("used_fallback"),
        "M229: SSIM summary must distinguish predicted vs measured"
    );
    assert!(
        !append_log.contains(", approx.)"),
        "M229: must not label all SSIM as approx"
    );

    let hits = whole_repo_silent_measurement_forgery_offenders(&root);
    assert!(
        hits.is_empty(),
        "silent measurement forgery patterns (M229, outside M222–M228 lists) must be absent from \
         whole-repo production Rust:\n{}",
        hits.join("\n")
    );

    let bin_hits = silent_fabrication_offenders_in_files(
        &root,
        &workspace_bin_rust_files(&root),
        MC_FORBIDDEN_SILENT_MEASUREMENT_FORGERY,
    );
    assert!(
        bin_hits.is_empty(),
        "silent measurement forgery patterns (M230) must be absent from crate bins:\n{}",
        bin_hits.join("\n")
    );

    let fuzz_hits = silent_fabrication_offenders_in_files(
        &root,
        &workspace_fuzz_rust_files(&root),
        MC_FORBIDDEN_SILENT_MEASUREMENT_FORGERY,
    );
    assert!(
        fuzz_hits.is_empty(),
        "silent measurement forgery patterns (M230) must be absent from fuzz targets:\n{}",
        fuzz_hits.join("\n")
    );
}

#[test]
fn media_conversion_unified_fabrication_closure_m232() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 232),
        "contract must document M232"
    );
    assert!(
        contract.contains("M232 static closure"),
        "contract must define operational 100% closure via M232 unified scan"
    );

    let rust_targets = workspace_unified_fabrication_rust_targets(&root);
    assert!(
        rust_targets.len() >= 150,
        "M232 unified rust scan must cover whole workspace src + bins + fuzz (got {})",
        rust_targets.len()
    );
    let scripts = workspace_dev_scripts_py_files(&root);
    assert!(
        scripts.len() >= 5,
        "M232 must scan dev scripts Python (got {})",
        scripts.len()
    );

    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "unified fabrication patterns (M222–M246 whole-repo union) must be absent from workspace \
         rust+bins+fuzz+dev/tests+dev/scripts:\n{}",
        hits.join("\n")
    );

    let explore = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/explore_strategy.rs",
    ))
    .expect("explore_strategy.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let explore_prod = production_scope(&explore);
    assert!(
        explore_prod.contains("SsimResult::predicted")
            && explore_prod.contains("is_some_and(SsimResult::is_predicted)")
            && explore_prod.contains("explore_ssim_measurement_fallback_audit"),
        "PSNR→SSIM explore fallback must wire predicted SSIM + audit (M232)"
    );

    let py = fs::read_to_string(root.join("crates/dev/scripts/quality_regression_model.py"))
        .expect("quality_regression_model.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        py.contains("normalize_nullable_embed_slots"),
        "training must map absent PSNR/SSIM embed slots to NaN (M232 parity with M225 predict)"
    );
}

#[test]
fn media_conversion_numeric_prior_unified_scan_m233() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 233),
        "contract must document M233"
    );
    assert!(
        contract.contains("Beyond M232 closure"),
        "contract must list residual risk categories beyond unified scan"
    );

    let patterns = mc_unified_fabrication_patterns();
    for needle in [
        "unwrap_or(0.5",
        "seal_unit_probability_or",
        "LOOP_INTENT_DEFAULT_KNN_CONFIDENCE",
    ] {
        assert!(
            patterns.contains(&needle),
            "M233 numeric prior patterns must be in unified union (missing {needle})"
        );
    }

    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "M233 unified scan (incl. numeric prior injection) must have zero hits:\n{}",
        hits.join("\n")
    );

    let heatmap = root.join("crates/dev/scripts/media_conversion_delivery_heatmap.py");
    assert!(heatmap.is_file(), "heatmap audit script must exist");
    let heatmap_hits = silent_fabrication_offenders_in_files(
        &root,
        &[heatmap],
        MC_FORBIDDEN_NUMERIC_PRIOR_INJECTION,
    );
    assert!(
        heatmap_hits.is_empty(),
        "heatmap.py is allowlisted from numeric-prior scan (pattern catalog only):\n{}",
        heatmap_hits.join("\n")
    );
}

#[test]
fn media_conversion_psnr_ssim_estimate_scope_m234() {
    let root = workspace_root();
    let allowed = [
        "crates/foundation/src/quality/ssim_mapping.rs",
        "crates/foundation/src/convert/explore_strategy.rs",
    ];
    let mut offenders = Vec::new();
    for file in workspace_all_crate_production_rust_files(&root) {
        let rel = file.strip_prefix(&root).unwrap_or(&file).to_string_lossy();
        if allowed.iter().any(|a| rel == *a) {
            continue;
        }
        let content = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (idx, line) in prod.lines().enumerate() {
            if is_fabrication_scan_comment_line(line) {
                continue;
            }
            if line.contains(MC_FORBIDDEN_PSNR_TO_SSIM_OUTSIDE_GATEWAY) {
                offenders.push(format!("{rel}:{}: {}", idx + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "psnr_to_ssim_estimate must only appear in explore_strategy + ssim_mapping production:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn media_conversion_explore_confidence_literal_use_m234() {
    let root = workspace_root();
    let hits = code_line_fabrication_offenders(
        &root,
        &workspace_unified_fabrication_rust_targets(&root),
        MC_FORBIDDEN_EXPLORE_CONFIDENCE_LITERAL_USE,
    );
    assert!(
        hits.is_empty(),
        "legacy EXPLORE_CONFIDENCE_* literals must not be used in production code:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_numeric_forgery_workspace_closure_m234() {
    let root = workspace_root();
    let hits = offending_lines(
        &root,
        &workspace_unified_fabrication_rust_targets(&root),
        NUMERIC_FORGERY_PATTERNS,
    );
    assert!(
        hits.is_empty(),
        "numeric unwrap_or/map_or forgery patterns must be absent from workspace rust closure:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_stale_db_embed_runtime_guard_m235() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 235),
        "contract must document M235"
    );

    let model = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_regression_model.rs",
    ))
    .expect("quality_regression_model.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&model);
    assert!(
        prod.contains("fn embed_measurement_slot_json")
            && prod.contains("quality_regression_stale_embed"),
        "M235: runtime inference must null stale DB embed 17/18 when PSNR/SSIM unmeasured"
    );
    assert!(
        prod.contains("embed_measurement_slot_json(\n                *value,")
            || prod.contains("embed_measurement_slot_json("),
        "M235: embed slots must use measurement-gated JSON helper"
    );
}

#[test]
fn media_conversion_python_tooling_numeric_forgery_m235() {
    let root = workspace_root();
    let hits = python_tooling_fabrication_offenders(&root, NUMERIC_FORGERY_PATTERNS);
    assert!(
        hits.is_empty(),
        "Python dev tooling must not contain numeric forgery unwrap_or/map_or patterns:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_python_training_embed_pipeline_m235() {
    let root = workspace_root();
    let py = fs::read_to_string(root.join("crates/dev/scripts/quality_regression_model.py"))
        .expect("quality_regression_model.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        py.contains("normalize_nullable_embed_slots")
            && py.contains("build_feature_row")
            && py.contains("normalize_nullable_embed_slots(row)"),
        "M235: training row builder must normalize nullable embed slots after copy"
    );
    let normalize_script =
        root.join("crates/dev/src/bin/normalize_stale_embed_measurement_slots.rs");
    assert!(
        normalize_script.is_file(),
        "M235: DB backfill bin normalize_stale_embed_measurement_slots.rs must exist"
    );
    assert!(
        !py.contains("embedding_017\": 0.0") && !py.contains("embedding_018\": 0.0"),
        "M235: training script must not hardcode forged embed measurement literals"
    );
}

#[test]
fn media_conversion_repo_shell_scripts_fabrication_m235() {
    let root = workspace_root();
    let scripts = workspace_repo_shell_scripts(&root);
    if scripts.is_empty() {
        return;
    }
    let hits = silent_fabrication_offenders_in_files(
        &root,
        &scripts,
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
    );
    assert!(
        hits.is_empty(),
        "repo scripts/ must not contain decision-chain fabrication patterns:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_unified_closure_includes_m234_literals_m236() {
    let patterns = mc_unified_fabrication_patterns();
    for needle in MC_FORBIDDEN_EXPLORE_CONFIDENCE_LITERAL_USE {
        assert!(
            patterns.contains(needle),
            "M236: unified closure must include M234 explore confidence literal ban ({needle})"
        );
    }
}

#[test]
fn media_conversion_explore_sealed_ssim_gate_wiring_m236() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 236),
        "contract must document M236"
    );

    let explorer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&explorer);
    for needle in [
        "fn enforce_exploration_quality_gates",
        "Self::enforce_ssim_presence_quality_gate",
        "Self::enforce_ssim_measurement_quality_gate",
        "Self::enforce_ssim_threshold_quality_gate",
        "!result.used_fallback",
        "exploration_ssim_predicted_rejected",
    ] {
        assert!(
            prod.contains(needle),
            "M236: explore SSIM gate wiring must include {needle}"
        );
    }
    assert!(
        explorer.contains("fn explore_result_ssim_predicted_fallback_gate_rejects_pass"),
        "M236: runtime unit test must cover used_fallback strict gate rejection"
    );
}

#[test]
fn media_conversion_repo_shell_scripts_broad_fabrication_m236() {
    let root = workspace_root();
    let scripts = workspace_repo_shell_scripts(&root);
    if scripts.is_empty() {
        return;
    }
    let patterns = mc_repo_shell_fabrication_patterns();
    assert!(
        patterns.contains(&"infer_quality_embedding_psnr_ssim"),
        "M236 shell scan must include decision-chain patterns"
    );
    assert!(
        patterns.contains(&"sampling_coverage = Some(1.0_f64)"),
        "M236 shell scan must include explore-confidence fabrication patterns"
    );
    let hits = silent_fabrication_offenders_in_files(&root, &scripts, &patterns);
    assert!(
        hits.is_empty(),
        "repo scripts/ must not contain unified fabrication patterns:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_automation_surface_fabrication_m237() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 237),
        "contract must document M237"
    );

    let surfaces = workspace_repo_automation_files(&root);
    assert!(
        surfaces
            .iter()
            .any(|p| p.file_name().is_some_and(|n| n == "justfile")),
        "M237 must scan justfile"
    );
    assert!(
        surfaces
            .iter()
            .any(|p| p.components().any(|c| c.as_os_str() == ".github")),
        "M237 must scan .github automation files"
    );
    let patterns = mc_repo_shell_fabrication_patterns();
    let hits = automation_surface_fabrication_offenders(&root, &surfaces, &patterns);
    assert!(
        hits.is_empty(),
        "automation surfaces (justfile/scripts/workflows) must not contain fabrication \
         patterns:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_headless_gif_ci_regression_m238() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 238),
        "contract must document M238"
    );

    let check_all = fs::read_to_string(root.join("crates/dev/src/bin/check_all.rs"))
        .expect("check_all.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        check_all.contains("\"test\"") && check_all.contains("\"--workspace\""),
        "M238: check_all --ci must run headless_gif_regression (ffmpeg/runtime probe path)"
    );

    let headless = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/headless_gif_regression.rs",
    ))
    .expect("headless_gif_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        headless.contains("build_synthetic_headless_sticker_gif"),
        "headless GIF regression must use in-test synthetic fixture (no missing CI asset)"
    );
    assert!(
        headless.contains("scan_gif_headers") && headless.contains("evaluate_loop_tree"),
        "headless GIF regression must exercise probe + loop-intent runtime path"
    );
}

#[test]
fn media_conversion_workspace_stderr_fabrication_m239() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 239),
        "contract must document M239"
    );

    let targets = workspace_unified_fabrication_rust_targets(&root);
    assert!(
        targets.len() >= 150,
        "M239 stderr scan must cover workspace rust+bins+fuzz (got {})",
        targets.len()
    );
    let hits = workspace_stderr_line_unwrap_offenders(&root);
    assert!(
        hits.is_empty(),
        "M239: stderr line unwrap_or must be absent outside gate across workspace rust \
         closure:\n{}",
        hits.join("\n")
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("encode_stderr_last_line_or_unknown"),
        "M239: gate must export audited stderr helper"
    );
}

#[test]
fn media_conversion_migrations_sql_fabrication_m239() {
    let root = workspace_root();
    let sql_files = workspace_migration_sql_files(&root);
    assert!(
        !sql_files.is_empty(),
        "migrations/*.sql must exist for schema fabrication scan"
    );
    let patterns = mc_unified_fabrication_patterns();
    let hits = automation_surface_fabrication_offenders(&root, &sql_files, &patterns);
    assert!(
        hits.is_empty(),
        "migrations/*.sql must not contain unified fabrication patterns:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_whole_repository_surface_closure_m240() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 240),
        "contract must document M240"
    );
    assert!(
        contract.contains("Formal acceptance tiers"),
        "contract must define honest formal acceptance tiers (M240)"
    );

    let rust_targets = workspace_whole_repository_rust_fabrication_targets(&root);
    let dev_tests = workspace_dev_integration_test_rust_files(&root);
    assert!(
        dev_tests.len() >= 18,
        "M240 must scan dev integration tests excluding pattern catalogs (got {})",
        dev_tests.len()
    );
    assert!(
        rust_targets.len() >= 210,
        "M240 whole-repo rust surface must cover all crates + bins + fuzz + dev/tests (got {})",
        rust_targets.len()
    );
    assert!(
        rust_targets
            .iter()
            .any(|p| { p.components().any(|c| c.as_os_str() == "dispatch2") }),
        "M240 must include dispatch2 (众生平等 whole-repo scan)"
    );

    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "whole-repository fabrication closure must have zero hits:\n{}",
        hits.join("\n")
    );

    let automation = workspace_repo_automation_files(&root);
    let auto_hits = automation_surface_fabrication_offenders(
        &root,
        &automation,
        &mc_repo_shell_fabrication_patterns(),
    );
    assert!(
        auto_hits.is_empty(),
        "whole-repository automation surfaces must have zero fabrication hits:\n{}",
        auto_hits.join("\n")
    );
}

#[test]
fn media_conversion_runtime_probe_regression_m241() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 241),
        "contract must document M241"
    );

    let check_all = fs::read_to_string(root.join("crates/dev/src/bin/check_all.rs"))
        .expect("check_all.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        check_all.contains("\"test\"") && check_all.contains("\"--workspace\""),
        "M241: check_all --ci must run WebP/APNG runtime probe regression"
    );

    let probe = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/runtime_probe_regression.rs",
    ))
    .expect("runtime_probe_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "build_synthetic_two_frame_animated_webp",
        "build_synthetic_two_frame_apng",
        "video_detection::detect_video",
    ] {
        assert!(
            probe.contains(sym),
            "M241 runtime probe regression must use synthetic fixtures + detect_video ({sym})"
        );
    }
}

#[test]
fn media_conversion_unified_fabrication_patterns_m242_extension() {
    let root = workspace_root();
    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "M242 extended unified patterns must have zero whole-repo hits:\n{}",
        hits.join("\n")
    );

    let patterns = mc_unified_fabrication_patterns();
    for needle in [
        "estimate_psnr_from_quality(",
        "estimate_ssim_from_quality(",
        "confidence: Some(1.0)",
        "unwrap_or(1.0_f64)",
        "MC_FORBIDDEN_SYNTAX_BYPASS_M242",
    ] {
        if needle.starts_with("MC_") {
            let tests = fs::read_to_string(join_legacy_aware(
                &root,
                "crates/dev/src/tests/test_real_silent_fallbacks.rs",
            ))
            .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
            assert!(
                tests.contains(needle),
                "M242 pattern list {needle} must exist"
            );
        } else {
            assert!(
                patterns.contains(&needle),
                "M242 unified patterns must include {needle}"
            );
        }
    }
}

#[test]
fn media_conversion_unified_fabrication_patterns_m243_extension() {
    let root = workspace_root();
    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "M243 extended unified patterns must have zero whole-repo hits:\n{}",
        hits.join("\n")
    );

    let patterns = mc_unified_fabrication_patterns();
    for needle in [
        "unwrap_or_else(|| 0.0",
        "Verdict::LoopStrong(_) => Some(1.0)",
        "confidence: Some(0.9)",
        "MC_FORBIDDEN_SYNTAX_BYPASS_M243",
    ] {
        if needle.starts_with("MC_") {
            let tests = fs::read_to_string(join_legacy_aware(
                &root,
                "crates/dev/src/tests/test_real_silent_fallbacks.rs",
            ))
            .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
            assert!(
                tests.contains(needle),
                "M243 pattern list {needle} must exist"
            );
        } else {
            assert!(
                patterns.contains(&needle),
                "M243 unified patterns must include {needle}"
            );
        }
    }

    let probe = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/runtime_probe_regression.rs",
    ))
    .expect("runtime_probe_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        probe.contains("include!(\"edge/apng/synth_animated_apng.rs\")")
            || probe.contains("include!(\"../edge/apng/synth_animated_apng.rs\")"),
        "M243 runtime probe must include synthetic fixtures in-module (no #[path] submodule pub)"
    );

    for fixture in [
        "edge/apng/synth_animated_apng.rs",
        "edge/avif/synth_static_avif.rs",
        "edge/gifs/synth_headless_gif.rs",
        "edge/heic/synth_static_heic.rs",
        "edge/jxl/synth_static_jxl.rs",
        "edge/webp/synth_animated_webp.rs",
        "edge/gifs/synth_webp.rs",
    ] {
        let content = fs::read_to_string(root.join("crates/dev/src/tests").join(fixture))
            .unwrap_or_else(|e| panic!("{fixture}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            !content.contains("allow(unreachable_pub)"),
            "M243: {fixture} must not mask visibility with allow(unreachable_pub)"
        );
        assert!(
            !content.contains("pub fn build_synthetic_"),
            "M243: {fixture} must not use pub fn builder"
        );
        assert!(
            !content.contains("pub(super) fn build_synthetic_"),
            "M243: {fixture} must not use pub(super) fn builder"
        );
        assert!(
            !content.contains("pub(crate) fn build_synthetic_"),
            "M243: {fixture} must not use pub(crate) fn builder"
        );
        assert!(
            content.contains("fn build_synthetic_"),
            "M243: {fixture} must define synthetic builder fn"
        );
    }
}

#[test]
fn media_conversion_production_scope_fabrication_closure_m246() {
    let root = workspace_root();
    let hits = production_scope_pattern_offenders(
        &root,
        PRODUCTION_SCOPE_FABRICATION_TARGETS_M246,
        MC_FORBIDDEN_DECISION_CHAIN_FABRICATION,
    );
    assert!(
        hits.is_empty(),
        "decision-chain fabrication patterns must not appear in production scope (M246):\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_isobmff_animation_detection_structure_m246() {
    let root = workspace_root();
    let det = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&det);
    assert!(
        prod.contains("is_isobmff_animated_sequence(path)"),
        "detect_animation production path must consult ISOBMFF sequence brands"
    );
    assert!(
        prod.contains("isobmff_cover_stream_ambiguous(path)"),
        "detect_animation must not declare static when cover/thumbnail streams are ambiguous"
    );
    let explicit_single = prod
        .split("if explicit_count == 1")
        .nth(1)
        .and_then(|tail| tail.split("// HEIC/HEIF").next())
        .expect("explicit_count == 1 branch should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        explicit_single.contains("is_isobmff_animated_sequence(path)"),
        "ffprobe explicit_count==1 on AVIF/JXL must check sequence brands before static return"
    );
}

#[test]
fn media_conversion_embed_measurement_slot_json_closure_m246() {
    let root = workspace_root();
    let rust = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_regression_model.rs",
    ))
    .expect("quality_regression_model.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let rust_prod = production_scope(&rust);
    assert!(
        rust_prod.contains("embed_measurement_slot_json")
            && rust_prod.contains("embedding_017")
            && rust_prod.contains("embedding_018"),
        "LightGBM export must gate embed 17/18 via embed_measurement_slot_json (M246 tier C)"
    );
    let py = fs::read_to_string(root.join("crates/dev/scripts/quality_regression_model.py"))
        .expect("quality_regression_model.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        py.contains("normalize_nullable_embed_slots"),
        "training script must normalize nullable embed slots (M246 tier C)"
    );
}

#[test]
fn media_conversion_unified_fabrication_patterns_m246_extension() {
    let root = workspace_root();
    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "M246 extended unified patterns must have zero whole-repo hits:\n{}",
        hits.join("\n")
    );

    let patterns = mc_unified_fabrication_patterns();
    for needle in ["embedding_017\": 0.0", "MC_FORBIDDEN_SYNTAX_BYPASS_M246"] {
        if needle.starts_with("MC_") {
            let tests = fs::read_to_string(join_legacy_aware(
                &root,
                "crates/dev/src/tests/test_real_silent_fallbacks.rs",
            ))
            .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
            assert!(
                tests.contains(needle),
                "M246 pattern list {needle} must exist"
            );
        } else {
            assert!(
                patterns.contains(&needle),
                "M246 unified patterns must include {needle}"
            );
        }
    }
}

#[test]
fn media_conversion_runtime_probe_regression_m246() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 246),
        "contract must document M246"
    );

    let probe = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/runtime_probe_regression.rs",
    ))
    .expect("runtime_probe_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "isobmff_sequence_brand_matrix_probe",
        "avis_gate_static_only_rejection_probe",
        "msf1_gate_static_only_rejection_probe",
        "static_heic_cover_stream_not_ambiguous_probe",
        "fabricated_multi_frame_never_confirmed_static_probe",
    ] {
        assert!(
            probe.contains(sym),
            "M246 runtime probe must cover ISOBMFF gate + cover ambiguity ({sym})"
        );
    }
}

#[test]
fn media_conversion_contract_m1_m246_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 246),
        "contract must document M246"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_production_scope_fabrication_closure_m246",
        "fn media_conversion_isobmff_animation_detection_structure_m246",
        "fn media_conversion_embed_measurement_slot_json_closure_m246",
        "fn media_conversion_unified_fabrication_patterns_m246_extension",
        "fn media_conversion_runtime_probe_regression_m246",
    ] {
        assert!(tests.contains(sym), "M246 dev test {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_production_scope_fabrication_closure_m246"),
        "M246 row must reference production-scope fabrication closure dev test"
    );
}

#[test]
fn media_conversion_algorithm_audit_unified_parity_m245() {
    let unified = mc_unified_fabrication_patterns();
    let mut missing = Vec::new();
    for needle in ALGORITHM_AUDIT_FORBIDDEN_PARITY_M245 {
        if !unified.contains(needle) {
            missing.push(*needle);
        }
    }
    assert!(
        missing.is_empty(),
        "every algorithm_audit FORBIDDEN_SUBSTRINGS entry must be in \
         mc_unified_fabrication_patterns (M245):\n{}",
        missing.join("\n")
    );
}

#[test]
fn media_conversion_unified_fabrication_patterns_m245_extension() {
    let root = workspace_root();
    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "M245 extended unified patterns must have zero whole-repo hits:\n{}",
        hits.join("\n")
    );

    let patterns = mc_unified_fabrication_patterns();
    for needle in ["0.5 neutral prior", "MC_FORBIDDEN_SYNTAX_BYPASS_M245"] {
        if needle.starts_with("MC_") {
            let tests = fs::read_to_string(join_legacy_aware(
                &root,
                "crates/dev/src/tests/test_real_silent_fallbacks.rs",
            ))
            .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
            assert!(
                tests.contains(needle),
                "M245 pattern list {needle} must exist"
            );
        } else {
            assert!(
                patterns.contains(&needle),
                "M245 unified patterns must include {needle}"
            );
        }
    }
}

#[test]
fn media_conversion_runtime_probe_regression_m245() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 245),
        "contract must document M245"
    );

    let probe = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/runtime_probe_regression.rs",
    ))
    .expect("runtime_probe_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "animated_avif_avis_sequence_probe",
        "animated_heif_msf1_sequence_probe",
        "build_synthetic_animated_avif_avis_ftyp",
        "build_synthetic_animated_heif_msf1_ftyp",
    ] {
        assert!(
            probe.contains(sym),
            "M245 runtime probe must cover ISOBMFF animated sequence brands ({sym})"
        );
    }
}

#[test]
fn media_conversion_unified_fabrication_patterns_m244_extension() {
    let root = workspace_root();
    let hits = unified_fabrication_offenders(&root);
    assert!(
        hits.is_empty(),
        "M244 extended unified patterns must have zero whole-repo hits:\n{}",
        hits.join("\n")
    );

    let patterns = mc_unified_fabrication_patterns();
    for needle in [
        "unwrap_or(f64::NAN",
        "confidence: Some(0.85)",
        "#[path = \"edge/",
        "MC_FORBIDDEN_SYNTAX_BYPASS_M244",
    ] {
        if needle.starts_with("MC_") {
            let tests = fs::read_to_string(join_legacy_aware(
                &root,
                "crates/dev/src/tests/test_real_silent_fallbacks.rs",
            ))
            .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
            assert!(
                tests.contains(needle),
                "M244 pattern list {needle} must exist"
            );
        } else {
            assert!(
                patterns.contains(&needle),
                "M244 unified patterns must include {needle}"
            );
        }
    }
}

#[test]
fn media_conversion_synthetic_edge_fixture_closure_m244() {
    let root = workspace_root();
    for consumer in [
        "crates/dev/src/tests/runtime_probe_regression.rs",
        "crates/dev/src/tests/headless_gif_regression.rs",
        "crates/dev/src/tests/test_webp_duration_parser.rs",
        "crates/dev/src/tests/test_webp_animated_classification.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, consumer))
            .unwrap_or_else(|e| panic!("{consumer}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            !content.contains("#[path = \"edge/"),
            "M244: {consumer} must not use #[path] edge submodule (use include!)"
        );
        assert!(
            content.contains("include!(\"edge/") || content.contains("include!(\"../edge/"),
            "M244: {consumer} must include edge synthetic fixtures in-module"
        );
    }

    let webp = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/edge/gifs/synth_webp.rs",
    ))
    .expect("synth_webp.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        webp.contains("fn build_synthetic_animated_webp_without_vp8x_in_header"),
        "M244: synth_webp must define builder fn"
    );
    assert!(
        !webp.contains("pub fn build_synthetic_"),
        "M244: synth_webp must use private fn builder"
    );
}

#[test]
fn media_conversion_runtime_probe_regression_m243() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 243),
        "contract must document M243"
    );

    let probe = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/runtime_probe_regression.rs",
    ))
    .expect("runtime_probe_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        probe.contains("build_synthetic_static_avif_ftyp"),
        "M243 runtime probe must cover synthetic AVIF format probe"
    );
    assert!(
        probe.contains("static_avif_format_and_animation_probe"),
        "M243 runtime probe must exercise AVIF detect_format/detect_animation"
    );
}

#[test]
fn media_conversion_runtime_probe_regression_m242() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 242),
        "contract must document M242"
    );

    let check_all = fs::read_to_string(root.join("crates/dev/src/bin/check_all.rs"))
        .expect("check_all.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        check_all.contains("\"test\"") && check_all.contains("\"--workspace\""),
        "M242: check_all --ci must run HEIC/JXL runtime probe regression"
    );

    let probe = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/runtime_probe_regression.rs",
    ))
    .expect("runtime_probe_regression.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "build_synthetic_static_heic_ftyp",
        "build_synthetic_jxl_short_header",
        "build_synthetic_jxl_long_header",
        "detect_format_from_bytes",
        "detect_animation",
        "static_jxl_detect_video_single_frame",
    ] {
        assert!(
            probe.contains(sym),
            "M242 runtime probe regression must cover synthetic HEIC/JXL + detect_video ({sym})"
        );
    }
}

#[test]
fn media_conversion_ci_mpc_mirror_download_m241() {
    let root = workspace_root();
    let script = fs::read_to_string(root.join("crates/dev/src/bin/download_gnu_mpc.rs"))
        .expect("download_gnu_mpc.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        script.contains("ftpmirror.gnu.org") && script.contains("ftp.gnu.org"),
        "MPC download script must list GNU mirror fallbacks"
    );
    for workflow in [
        ".github/workflows/ci-quality.yml",
        ".github/workflows/cd-stable.yml",
        ".github/workflows/cd-nightly.yml",
    ] {
        let content =
            fs::read_to_string(root.join(workflow)).unwrap_or_else(|e| panic!("{workflow}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("--bin download_gnu_mpc"),
            "{workflow} must use mirror-aware Rust MPC download bin (M241)"
        );
        assert!(
            !content
                .contains("download_with_retry \"https://ftp.gnu.org/gnu/mpc/mpc-1.4.1.tar.xz\""),
            "{workflow} must not use single-host MPC download without mirrors"
        );
    }
}

#[test]
fn media_conversion_contract_m1_m226_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 226),
        "contract must document M226"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_python_predict_null_embed_m225",
        "fn media_conversion_fuzz_silent_fabrication_scan_m226",
    ] {
        assert!(tests.contains(sym), "M225/M226 dev test {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_python_predict_null_embed_m225"),
        "M225 row must reference Python null-embed predict dev test"
    );
    assert!(
        contract.contains("media_conversion_fuzz_silent_fabrication_scan_m226"),
        "M226 row must reference fuzz silent-fabrication scan dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m227_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 227),
        "contract must document M227"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_gpu_coarse_confidence_m227"),
        "M227 dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_gpu_coarse_confidence_m227"),
        "M227 row must reference GPU coarse confidence dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m228_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 228),
        "contract must document M228"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_repo_wide_explore_confidence_fabrication_m228"),
        "M228 dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_repo_wide_explore_confidence_fabrication_m228"),
        "M228 row must reference repo-wide explore confidence scan dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m229_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 229),
        "contract must document M229"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_whole_repo_measurement_forgery_m229"),
        "M229 dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_whole_repo_measurement_forgery_m229"),
        "M229 row must reference whole-repo measurement forgery dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m230_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 230),
        "contract must document M230"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("silent measurement forgery patterns (M230) must be absent from crate bins"),
        "M230 bin scan assert must exist in whole-repo measurement forgery test"
    );
    assert!(
        tests.contains(
            "silent measurement forgery patterns (M230) must be absent from fuzz targets"
        ),
        "M230 fuzz scan assert must exist in whole-repo measurement forgery test"
    );
}

#[test]
fn media_conversion_contract_m1_m232_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 232),
        "contract must document M232"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_unified_fabrication_closure_m232"),
        "M232 dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_unified_fabrication_closure_m232"),
        "M232 row must reference unified fabrication closure dev test"
    );
    assert!(
        tests.contains("fn mc_unified_fabrication_patterns"),
        "M232 must define unified pattern union helper"
    );
}

#[test]
fn media_conversion_contract_m1_m233_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 233),
        "contract must document M233"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_numeric_prior_unified_scan_m233"),
        "M233 dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_numeric_prior_unified_scan_m233"),
        "M233 row must reference numeric prior unified scan dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m234_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 234),
        "contract must document M234"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_psnr_ssim_estimate_scope_m234",
        "fn media_conversion_explore_confidence_literal_use_m234",
        "fn media_conversion_numeric_forgery_workspace_closure_m234",
    ] {
        assert!(tests.contains(sym), "M234 dev test {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_psnr_ssim_estimate_scope_m234"),
        "M234 row must reference PSNR→SSIM scope dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m235_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 235),
        "contract must document M235"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_stale_db_embed_runtime_guard_m235"),
        "M235 stale embed guard dev test must exist"
    );
    assert!(
        contract.contains("media_conversion_stale_db_embed_runtime_guard_m235"),
        "M235 row must reference stale DB embed runtime guard dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m236_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 236),
        "contract must document M236"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_unified_closure_includes_m234_literals_m236",
        "fn media_conversion_explore_sealed_ssim_gate_wiring_m236",
        "fn media_conversion_repo_shell_scripts_broad_fabrication_m236",
    ] {
        assert!(tests.contains(sym), "M236 dev test {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_explore_sealed_ssim_gate_wiring_m236"),
        "M236 row must reference explore sealed SSIM gate wiring dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m239_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for milestone in [237_u32, 238, 239] {
        assert!(
            contract_documents_milestone(&contract, milestone),
            "contract must document known-weakness milestone M{milestone}"
        );
    }
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_automation_surface_fabrication_m237",
        "fn media_conversion_headless_gif_ci_regression_m238",
        "fn media_conversion_workspace_stderr_fabrication_m239",
        "fn media_conversion_migrations_sql_fabrication_m239",
        "fn workspace_repo_automation_files",
    ] {
        assert!(
            tests.contains(sym),
            "M237–M239 dev test/helper {sym} must exist"
        );
    }
    assert!(
        !contract.contains("人工审查；按需纳入扫描根"),
        "Beyond M232 closure must not leave automation surfaces as manual-only after M237"
    );
}

#[test]
fn media_conversion_contract_m1_m240_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 240),
        "contract must document M240"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_whole_repository_surface_closure_m240",
        "fn workspace_whole_repository_rust_fabrication_targets",
        "fn workspace_dev_integration_test_rust_files",
    ] {
        assert!(tests.contains(sym), "M240 dev test/helper {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_whole_repository_surface_closure_m240"),
        "M240 row must reference whole-repository surface closure dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m244_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 244),
        "contract must document M244"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_unified_fabrication_patterns_m244_extension",
        "fn media_conversion_synthetic_edge_fixture_closure_m244",
        "fn media_conversion_unified_fabrication_patterns_m243_extension",
        "fn media_conversion_runtime_probe_regression_m243",
    ] {
        assert!(tests.contains(sym), "M244 dev test {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_synthetic_edge_fixture_closure_m244"),
        "M244 row must reference synthetic edge fixture closure dev test"
    );
}

#[test]
fn media_conversion_contract_m1_m245_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 245),
        "contract must document M245"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn media_conversion_algorithm_audit_unified_parity_m245",
        "fn media_conversion_unified_fabrication_patterns_m245_extension",
        "fn media_conversion_runtime_probe_regression_m245",
        "fn media_conversion_synthetic_edge_fixture_closure_m244",
    ] {
        assert!(tests.contains(sym), "M245 dev test {sym} must exist");
    }
    assert!(
        contract.contains("media_conversion_algorithm_audit_unified_parity_m245"),
        "M245 row must reference algorithm_audit unified parity dev test"
    );
}

#[test]
fn media_conversion_quality_paths_no_heuristic_knn_column_forgery_m219b() {
    let root = workspace_root();
    for (rel, prod_scope) in [
        ("crates/foundation/src/image_quality_db.rs", true),
        ("crates/foundation/src/scenario_quality_lookup.rs", true),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let scope = if prod_scope {
            production_scope(&content)
        } else {
            &content
        };
        assert!(
            scope.contains("is_heuristic_only_branch"),
            "{rel} must gate heuristic-only branches before writing knn_score columns"
        );
    }
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("scenario_heuristic_branches_do_not_populate_knn_score_columns"),
        "scenario quality must have regression test for knn column forgery (M219b)"
    );
}

#[test]
fn training_scripts_do_not_use_legacy_modern_format_enable_env() {
    let root = workspace_root();
    let scripts_dir = root.join("crates/dev/scripts");
    for entry in std::fs::read_dir(&scripts_dir)
        .unwrap_or_else(|err| panic!("read {}: {err:?}", scripts_dir.display())) // audited: contract test assertion path; panic/expect is test-only failure signal
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("py") {
            continue;
        }
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let content = fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {rel}: {err:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            !content.contains("MODERN_FORMAT_ENABLE_"),
            "{rel} must use MODERN_FORMAT_DISABLE_* kill-switches, not legacy ENABLE_* env keys"
        );
    }
}

#[test]
fn multi_scenario_db_defines_layer7_posterior_analytics_view() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/multi_scenario_db.rs",
    ))
    .expect("multi_scenario_db.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in [
        "CREATE OR REPLACE VIEW loop_inference_log_effective AS",
        "decode(file_hash, 'hex')",
        "legacy_file_hash",
        "is_layer7_policy_exit",
        "tree_probability_is_authoritative",
        "effective_final_probability",
        "runtime_final_probability",
    ] {
        assert!(
            content.contains(required),
            "multi_scenario_db.rs must define loop inference analytics view column `{required}`"
        );
    }
}

#[test]
fn scenario_quality_lookup_heuristic_contract_in_source() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/scenario_quality_lookup.rs",
    ))
    .expect("scenario_quality_lookup.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in [
        "is_heuristic_only_branch",
        "let heuristic_only = branch.is_heuristic_only_branch()",
        "(None, None, None)",
        "runtime_heuristic_score",
    ] {
        assert!(
            content.contains(required),
            "scenario_quality_lookup.rs must implement heuristic knn_score contract; missing \
             `{required}`"
        );
    }
}

#[test]
fn algorithm_contract_doc_exists_and_lists_allowlist() {
    let root = workspace_root();
    let content = read_hardening_doc(&root, "ALGORITHM_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in [
        "## Core invariants",
        "## Allowlisted fallbacks",
        "## Analytics (SQL)",
        "## Compliance matrix",
        "is_layer7_policy_exit",
        "tree_probability_is_authoritative",
        "I10",
    ] {
        assert!(
            content.contains(required),
            "ALGORITHM_LAYER_CONTRACT.md must document `{required}`"
        );
    }
}

#[test]
fn ui_contract_doc_exists() {
    let root = workspace_root();
    let content = read_hardening_doc(&root, "UI_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in [
        "## Core invariants",
        "U1",
        "U3",
        "U7",
        "U9",
        "U10",
        "U12",
        "U13",
        "U14",
        "U15",
        "## Compliance matrix",
        "MODERN_FORMAT_PLAIN_UI",
        "ui_stderr",
        "configure_terminal_ux",
    ] {
        assert!(
            content.contains(required),
            "UI_LAYER_CONTRACT.md must document `{required}`"
        );
    }
}

#[test]
fn unified_progress_uses_shared_progress_style_chars() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/unified_progress.rs",
    ))
    .expect("unified_progress.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("progress_style::PROGRESS_CHARS"),
        "unified_progress must use modern_ui::progress_style glyphs (U1)"
    );
    assert!(
        !content.contains("█▓░"),
        "unified_progress must not define a divergent block progress charset"
    );
}

#[test]
fn static_logs_uses_symbol_pick_in_macros() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/static_logs.rs",
    ))
    .expect("static_logs.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("media_conversion_gate::ui_icon_pick"),
        "static_logs must route icon picks through gate (M56/U7)"
    );
    assert!(
        !content.contains("symbols::pick"),
        "static_logs must not call symbols::pick directly after M56"
    );
}

#[test]
fn loop_intent_emit_stderr_no_raw_emoji_literals() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for forbidden in [
        "emit_stderr(&format!(\"   ⚠️",
        "emit_stderr(\"   🔍",
        "emit_stderr(&format!(\"✅",
    ] {
        assert!(
            !content.contains(forbidden),
            "loop_intent must not use raw emoji in emit_stderr; found `{forbidden}`"
        );
    }
    assert!(
        content.contains("ui_stderr::line"),
        "loop_intent verbose stderr must use ui_stderr::line (U7)"
    );
}

#[test]
fn rust_brand_hex_aligns_with_python_ui_tokens() {
    let root = workspace_root();
    let rust_ui = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/modern_ui.rs",
    ))
    .expect("modern_ui.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let py = fs::read_to_string(root.join("crates/dev/scripts/mfb_ui_tokens.py"))
        .expect("mfb_ui_tokens.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        rust_ui.contains("pub const HEX_BLUE: &str = \"#43a0ff\""),
        "modern_ui::brand::HEX_BLUE must be #43a0ff (U2)"
    );
    assert!(
        py.contains("BRAND_BLUE = \"#43a0ff\""),
        "mfb_ui_tokens.BRAND_BLUE must match Rust brand hex (U2)"
    );
}

#[test]
fn modern_ui_enhanced_log_macros_use_ui_stderr() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/modern_ui.rs",
    ))
    .expect("modern_ui.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for mac in [
        "log_enhanced_success",
        "log_enhanced_start",
        "log_enhanced_end",
    ] {
        let start = content
            .find(mac)
            .unwrap_or_else(|| panic!("macro {mac} must exist")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let slice = &content[start..start.saturating_add(400)];
        assert!(
            slice.contains("ui_stderr::line"),
            "{mac} must route stderr through ui_stderr::line (U7)"
        );
    }
}

#[test]
fn foundation_core_paths_use_ui_stderr_not_raw_success_emoji() {
    let root = workspace_root();
    for rel in [
        "crates/foundation/src/ctrlc_guard.rs",
        "crates/foundation/src/progress.rs",
        "crates/foundation/src/database.rs",
        "crates/foundation/src/image_quality_db.rs",
        "crates/foundation/src/media_penetration.rs",
        "crates/foundation/src/date_analysis.rs",
        "crates/foundation/src/jxl_utils.rs",
        "crates/foundation/src/bin/train_knn.rs",
        "crates/foundation/src/bin/train_quality.rs",
        "crates/foundation/src/bin/recompute_stats.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel} must be readable: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let checkmark = '\u{2705}';
        assert!(
            !content.contains(&format!("emit_stderr(&format!(\"{checkmark}")),
            "{rel} must not emit raw success emoji via format (U7)"
        );
        assert!(
            !content.contains(&format!("emit_stderr(\"{checkmark}")),
            "{rel} must not emit raw success emoji literal (U7)"
        );
    }
}

#[test]
fn format_level_and_upstream_use_symbol_pick() {
    let root = workspace_root();
    let ui = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/modern_ui.rs",
    ))
    .expect("modern_ui.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ui.contains("symbols::pick(symbols::CROSS, symbols::plain::ERROR)"),
        "format_level must use symbols::pick (U8)"
    );
    assert!(
        ui.contains("ui_stderr::line") && ui.contains("UpstreamToolLogger"),
        "UpstreamToolLogger must route errors through ui_stderr"
    );
}

#[test]
fn progress_mode_inference_analytics_hint_for_u5() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/progress_mode.rs",
    ))
    .expect("progress_mode.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("maybe_log_inference_analytics_hint"),
        "U5 requires maybe_log_inference_analytics_hint in progress_mode"
    );
    assert!(
        content.contains("loop_inference_log_effective"),
        "hint must reference effective inference view"
    );
    let img =
        fs::read_to_string(root.join("crates/img/src/main.rs")).expect("img main must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        img.contains("maybe_log_inference_analytics_hint"),
        "img run must emit inference analytics hint when verbose"
    );
}

#[test]
fn progress_bar_messages_use_symbol_pick() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/progress.rs",
    ))
    .expect("progress.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("fn bar_status_msg")
            && content.contains("media_conversion_gate::ui_icon_pick"),
        "indicatif bar messages must use bar_status_msg + gate ui_icon_pick (M62)"
    );
    assert!(
        content.contains("ExploreLogger") && content.contains("ok_fail_icon"),
        "explore test lines must use ok_fail_icon (U7)"
    );
    assert!(
        content.contains("DetailedCoarseProgressBar")
            && content.contains("media_conversion_gate::ui_icon_pick"),
        "detailed coarse finish must use gate ui_icon_pick (M62)"
    );
    assert!(
        !content.contains("symbols::pick"),
        "progress.rs must not call symbols::pick after M62"
    );
}

#[test]
fn gpu_accel_user_messages_use_symbol_pick() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/gpu_accel.rs",
    ))
    .expect("gpu_accel.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("styled_warning_icon")
            && content.contains("media_conversion_gate::ui_icon_pick"),
        "gpu_accel stderr/search messages must use gate ui_icon_pick (M66/U7)"
    );
    assert!(
        !content.contains("modern_ui::symbols::pick") && !content.contains("symbols::pick"),
        "gpu_accel must not call symbols::pick after M66"
    );
    assert!(
        !content.contains("\"   ⚡ ") && !content.contains("\"⚠️ Failed"),
        "gpu_accel must not use raw ⚡/⚠️ prefix literals in user messages"
    );
    assert!(
        content.contains("let curve_step") && !content.lines().any(|l| l.trim() == "..."),
        "gpu_accel Stage1B must retain curve_step logic (no placeholder ...)"
    );
    assert!(
        content.contains("FfmpegBuilder::new()") && content.contains("filter_complex(\"ssim\")"),
        "validate_final_gpu_quality must retain SSIM probe pipeline"
    );
}

#[test]
fn critical_algorithm_helpers_still_exported() {
    let root = workspace_root();
    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        db.contains("pub fn loop_inference_runtime_verdict_from_snapshot"),
        "I4 helper must not be deleted by accidental edits"
    );
    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/gpu_accel.rs",
    ))
    .expect("gpu_accel.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gpu.contains("fn validate_final_gpu_quality")
            && gpu.contains("FfmpegBuilder::new()")
            && gpu.contains("fn gpu_coarse_search_with_log_impl"),
        "gpu coarse search pipeline must remain intact"
    );
}

#[test]
fn explore_strategy_logs_use_symbol_pick() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/explore_strategy.rs",
    ))
    .expect("explore_strategy.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !content.contains("ctx.log(format!(\"🔍")
            && !content.contains("ctx.log(format!(\"🎯")
            && !content.contains("ctx.log(format!(\"💾"),
        "explore_strategy ctx.log must not start with raw emoji literals"
    );
    assert!(
        content.contains("media_conversion_gate::ui_icon_pick(\"🔍\"")
            || content.contains("media_conversion_gate::ui_icon_pick(\"🎯\"")
            || content.contains("media_conversion_gate::ui_icon_pick(\"💾\""),
        "explore_strategy must use gate ui_icon_pick in log lines (M59)"
    );
    assert!(
        !content.contains("modern_ui::symbols::pick"),
        "explore_strategy must not call modern_ui::symbols::pick after M59"
    );
}

#[test]
fn gpu_coarse_search_crf_lines_use_crf_ui() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("mod crf_ui") && content.contains("crf_ui::pass_prefix"),
        "CRF probe lines must use crf_ui helpers (U7)"
    );
    assert!(
        !content.contains(r#"format!("✓ [CPU]"#)
            && !content.contains(r#"format!("✗ [CPU]"#)
            && !content.contains(r#""✓ {source_label} CRF"#),
        "gpu_coarse_search must not use raw ✓/✗ CRF format literals"
    );
    assert!(
        !content.lines().any(|l| {
            l.contains("format!(") && (l.contains("\"✓ [CPU]") || l.contains("\"✗ [CPU]"))
        }),
        "gpu_coarse_search CRF log format strings must route through crf_ui"
    );
}

#[test]
fn media_penetration_and_video_detection_stderr_use_symbol_pick() {
    let root = workspace_root();
    let penetration = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_penetration.rs",
    ))
    .expect("media_penetration.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        penetration.contains("styled_warning_icon")
            && !penetration.contains("\"⚠️  Full transparency")
            && !penetration.contains("\"⚠️  Frame count")
            && !penetration.contains("\"⚠️  Interlace"),
        "media_penetration warnings must use styled_warning_icon (U7)"
    );
    let video = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        video.contains("media_conversion_gate::ui_icon_pick")
            && !video.contains("\"⚠️  [{}] Frame count"),
        "video_detection frame-count mismatch must use gate ui_icon_pick (M65/U7)"
    );
    let copier = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/smart_file_copier.rs",
    ))
    .expect("smart_file_copier.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        copier.contains("ui_stderr::line")
            && !copier.contains("emit_stderr(&format!(\"   📋 [PRESERVE]"),
        "smart_file_copier preserve path must use ui_stderr (U7)"
    );
}

#[test]
fn path_validator_display_uses_symbol_pick() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/path_validator.rs",
    ))
    .expect("path_validator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("symbols::pick") && !content.contains("\"⚠️ PATH CONVERSION ERROR"),
        "PathConversionError Display must use symbols::pick (U7)"
    );
}

#[test]
fn media_conversion_layer_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("static_image_conversion_verdict")
            && gate.contains("analysis_trusted_for_static_conversion")
            && gate.contains("analysis_uncertainty_ignore_reason")
            && gate.contains("strict_media_conversion_delivery_enabled")
            && gate.contains("effective_allow_size_tolerance")
            && gate.contains("video_explore_pipeline_acceptable")
            && gate.contains("delivery_fallback_audit")
            && gate.contains("resolve_output_dir_for_delivery")
            && gate.contains("delivery_frame_count_label_u64")
            && gate.contains("explore_quality_fail_reason")
            && gate.contains("explore_ms_ssim_score_display")
            && gate.contains("ffprobe_pix_fmt_or_empty")
            && gate.contains("process_exit_code_label")
            && gate.contains("warm_start_crf_or_predicted")
            && gate.contains("jpeg_magic_valid_for_delivery")
            && gate.contains("avif_quality_or_fallback")
            && gate.contains("conversion_ssim_message_token")
            && gate.contains("mutex_guard_or_recover")
            && gate.contains("temp_output_suffix_utf8")
            && gate.contains("size_delta_report_label")
            && gate.contains("explore_quality_gate_audit")
            && gate.contains("hdr_metadata_fallback_audit")
            && gate.contains("apple_compat_fallback_audit")
            && gate.contains("delivery_path_audit")
            && gate.contains("delivery_batch_audit")
            && gate.contains("delivery_cleanup_audit")
            && gate.contains("webp_frame_duration_pad_audit")
            && gate.contains("gif_encode_fps_from_probe")
            && gate.contains("infra_version_label_or_audit")
            && gate.contains("tool_stderr_last_line_label")
            && gate.contains("explore_progress_ssim_token")
            && gate.contains("probe_layer_audit")
            && gate.contains("recovery_format_name")
            && gate.contains("recovery_channel_type_label")
            && gate.contains("color_depth_or_baseline")
            && gate.contains("explore_boundary_crf_or_refined")
            && gate.contains("probe_r_frame_rate_baseline")
            && gate.contains("probe_pixel_lossless_or_false")
            && gate.contains("delivery_temp_suffix_epoch_nanos")
            && gate.contains("probe_idet_count_or_zero"),
        "media conversion gate must own static + explore + size tolerance + audited fallbacks"
    );
    let conversion = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/conversion.rs",
    ))
    .expect("conversion.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        conversion.contains("effective_allow_size_tolerance")
            && !conversion.contains("ConvertFlags::USE_GPU | ConvertFlags::ALLOW_SIZE_TOLERANCE"),
        "ConvertOptions default must not enable size tolerance"
    );
    for rel in ["crates/img/src/main.rs", "crates/vid/src/main.rs"] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("default_value_t = false") && content.contains("allow_size_tolerance"),
            "{rel} must default allow_size_tolerance to false (strict size)"
        );
    }
    let explorer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        explorer.contains("media_conversion_gate::video_explore_pipeline_acceptable"),
        "ExploreResult::pipeline_acceptable must delegate to media_conversion_gate"
    );
    assert!(!read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md").is_empty());
    for rel in [
        "crates/vid/src/animated_image.rs",
        "crates/vid/src/conversion_api.rs",
        "crates/img/src/conversion_api.rs",
        "crates/img/src/lossless_converter.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} delivery paths must use media_conversion_gate audits, not raw log_anomaly!"
        );
        assert!(
            content.contains("media_conversion_gate::")
                || content.contains("delivery_fallback_audit"),
            "{rel} must route delivery fallbacks through media_conversion_gate"
        );
    }
}

#[test]
fn media_conversion_probe_layer_strict_defaults() {
    let root = workspace_root();
    assert!(!read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md").is_empty());
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("probe_layer_audit") && gate.contains("probe_layer_batch_audit"),
        "probe layer must route through gate audits"
    );
    for rel in [
        "crates/foundation/src/video_detection.rs",
        "crates/foundation/src/image_analyzer.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} probe paths must use media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} probe paths must not use raw log_anomaly! (use probe_layer_audit)"
        );
    }
    let explorer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        explorer.contains("explore_boundary_crf_optional"),
        "boundary search must resolve CRF via gate optional helper (fail-closed Err, no silent \
         unwrap_or)"
    );
    assert!(
        !explorer.contains("refined.unwrap_or(boundary_crf)"),
        "boundary CRF must not silently unwrap_or"
    );
}

#[test]
fn media_conversion_delivery_substrate_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_temp_suffix_epoch_nanos",
        "probe_encoder_settings_search_string",
        "probe_idet_count_or_zero",
        "delivery_cleanup_audit",
    ] {
        assert!(
            gate.contains(sym),
            "media_conversion_gate must export {sym} for delivery substrate"
        );
    }
    for rel in [
        "crates/foundation/src/conversion.rs",
        "crates/foundation/src/media_penetration.rs",
        "crates/vid/src/main.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on delivery/probe fallback paths"
        );
    }
    let explorer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_explorer = explorer.split("#[cfg(test)]").next().unwrap_or(&explorer);
    assert!(
        !prod_explorer.contains("log_anomaly!("),
        "video_explorer production code must use gate audits for explore fallbacks"
    );
    assert!(
        prod_explorer.contains("media_conversion_gate::explore_delivery_explore_outcome_audit")
            || prod_explorer.contains("media_conversion_gate::delivery_batch_audit"),
        "video_explorer must audit explore outcomes via gate (M84 strict or legacy batch)"
    );
}

#[test]
fn media_conversion_tooling_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_tool_process_failed_audit",
        "delivery_progress_eta_unknown_audit",
        "delivery_msssim_progress_pct_invalid_audit",
        "delivery_ffmpeg_io_audit",
        "explore_crf_cache_key_rejected_audit",
        "explore_ssim_measurement_fallback_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for tooling layer"
        );
    }
    for rel in [
        "crates/foundation/src/process_runner.rs",
        "crates/foundation/src/progress.rs",
        "crates/foundation/src/msssim_progress.rs",
        "crates/foundation/src/ffmpeg_process.rs",
        "crates/foundation/src/explore_strategy.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route tooling fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on tooling fallback paths"
        );
    }
}

#[test]
fn media_conversion_quality_intel_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "analysis_cache_invalidate_audit",
        "probe_hash_buffer_slice",
        "probe_quality_layer_audit",
        "probe_quality_batch_audit",
        "probe_webp_vp8x_flags_or_zero",
        "probe_bool_or_false",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for quality intel layer"
        );
    }
    for rel in [
        "crates/foundation/src/analysis_cache.rs",
        "crates/foundation/src/quality_matcher.rs",
        "crates/foundation/src/video_quality_detector.rs",
        "crates/foundation/src/image_quality_detector.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route quality intel fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on quality intel fallback paths"
        );
    }
}

#[test]
fn media_conversion_probe_ffprobe_explore_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_format_duration_missing_audit",
        "probe_pix_fmt_label",
        "probe_stream_index_or_fallback",
        "probe_b_frames_u8_or_max",
        "probe_side_data_type_label",
        "probe_hdr_metadata_u8_or_skip",
        "probe_ffprobe_path_audit",
        "probe_ffprobe_input_audit",
        "explore_precheck_audit",
        "explore_precheck_batch_audit",
        "hdr_intensity_target_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for ffprobe/HDR/explore aux layer (M19)"
        );
    }
    for rel in [
        "crates/foundation/src/ffprobe.rs",
        "crates/foundation/src/ffprobe_json.rs",
        "crates/foundation/src/hdr.rs",
        "crates/foundation/src/video_explorer/precheck.rs",
        "crates/foundation/src/video_explorer/stream_analysis.rs",
        "crates/foundation/src/video_explorer/ssim_calculator.rs",
        "crates/foundation/src/video_explorer/dynamic_mapping.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route ffprobe/HDR/explore fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M19 fallback paths"
        );
    }
}

#[test]
fn media_conversion_gpu_coarse_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in ["explore_gpu_coarse_audit", "explore_gpu_coarse_batch_audit"] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for GPU coarse explore layer (M20)"
        );
    }
    for rel in [
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
        "crates/foundation/src/video_explorer.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route GPU coarse fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M20 fallback paths"
        );
    }
}

#[test]
fn media_conversion_resume_db_detection_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_checkpoint_path_audit",
        "delivery_checkpoint_batch_audit",
        "delivery_db_path_audit",
        "delivery_db_batch_audit",
        "probe_layer_audit",
        "probe_layer_batch_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for resume/DB/detection layer (M21)"
        );
    }
    for rel in [
        "crates/foundation/src/checkpoint.rs",
        "crates/foundation/src/image_detection.rs",
        "crates/foundation/src/database.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M21 fallback paths"
        );
    }
}

#[test]
fn media_conversion_image_pipeline_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_image_format_audit",
        "probe_image_format_batch_audit",
        "delivery_pipeline_path_audit",
        "delivery_pipeline_batch_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for image analysis / pipeline layer (M22)"
        );
    }
    for rel in [
        "crates/foundation/src/image_jpeg_analysis.rs",
        "crates/foundation/src/image_heic_analysis.rs",
        "crates/foundation/src/image_formats.rs",
        "crates/foundation/src/batch.rs",
        "crates/foundation/src/cli_runner.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M22 fallback paths"
        );
    }
}

#[test]
fn media_conversion_metadata_jxl_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_metadata_path_audit",
        "delivery_metadata_batch_audit",
        "delivery_jxl_path_audit",
        "delivery_jxl_batch_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for metadata/JXL layer (M23)"
        );
    }
    for rel in [
        "crates/foundation/src/metadata/mod.rs",
        "crates/foundation/src/metadata/exif.rs",
        "crates/foundation/src/metadata/network.rs",
        "crates/foundation/src/metadata/linux.rs",
        "crates/foundation/src/metadata/windows.rs",
        "crates/foundation/src/jxl_utils.rs",
        "crates/foundation/src/jxl_explorer.rs",
        "crates/foundation/src/xmp_merger.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M23 fallback paths"
        );
    }
}

#[test]
fn delivery_metadata_contract_artifacts_locked() {
    let root = workspace_root();
    for rel in [
        "crates/foundation/src/tests/exif_structural_repair_contract.rs",
        "crates/foundation/src/tests/xmp_jxl_apple_compat_contract.rs",
        "crates/foundation/src/tests/metadata_preservation_contract.rs",
        "crates/foundation/src/metadata/delivery_policy.rs",
        "crates/foundation/src/test_ci_contract.rs",
    ] {
        assert!(
            join_legacy_aware(&root, rel).is_file(),
            "CONTRACT artifact missing: {rel}"
        );
    }
    let exif = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/metadata/exif.rs",
    ))
    .expect("exif.rs readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "should_run_structural_repair",
        "stderr_triggers_extension_fallback",
        "append_nuclear_repair_exiftool",
        "structural_repair_contract",
    ] {
        assert!(
            exif.contains(sym),
            "exif.rs must retain CONTRACT symbol `{sym}`"
        );
    }
    let xmp = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/xmp_merger.rs",
    ))
    .expect("xmp_merger.rs readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "should_jxl_xmp_apple_nuclear_strip",
        "append_jxl_apple_nuclear_xmp_merge",
        "xmp_jxl_apple_compat_contract",
    ] {
        assert!(
            xmp.contains(sym),
            "xmp_merger.rs must retain CONTRACT symbol `{sym}`"
        );
    }
    let ffprobe = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
        .expect("ffprobe.rs readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ffprobe.contains("FFPROBE_FRAME_SHOW_ENTRIES"),
        "ffprobe.rs must use FFPROBE_FRAME_SHOW_ENTRIES constant"
    );
    assert!(
        ffprobe.contains("contract_hdr10_plus_requires_typed_frame_side_data"),
        "ffprobe.rs must retain HDR10+ CONTRACT test"
    );
    let metadata_mod = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/metadata/mod.rs",
    ))
    .expect("metadata/mod.rs readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "preserve_pro_delivery_layer_order",
        "metadata_preservation_contract",
        "preserve_for_delivery",
        "MetadataDeliveryReport",
        "delivery_policy",
        "MSG_METADATA_DELIVERY_SKIP_NO_SOURCE_EXIF",
        "should_preserve_xattr",
        "XATTR_PRESERVE_SKIP_KEYS",
        "XATTR_MACOS_METADATA_PREFIXES",
        "supplemental_xattr",
        "NETWORK_XATTR_PRIORITY_KEYS",
    ] {
        assert!(
            metadata_mod.contains(sym),
            "metadata/mod.rs must retain CONTRACT symbol `{sym}`"
        );
    }
    let jxl = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/jxl_builder.rs",
    ))
    .expect("jxl_builder.rs readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        jxl.contains("contract_cjxl_compress_boxes_gated_by_apple_compat"),
        "jxl_builder.rs must retain apple_compat compress_boxes CONTRACT test"
    );
}

#[test]
fn media_conversion_delivery_substrate_ext_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_intent_path_audit",
        "delivery_intent_batch_audit",
        "delivery_io_path_audit",
        "delivery_io_batch_audit",
        "delivery_gpu_path_audit",
        "delivery_gpu_batch_audit",
        "delivery_encode_path_audit",
        "delivery_encode_batch_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for delivery substrate extensions (M24)"
        );
    }
    for rel in [
        "crates/foundation/src/loop_intent.rs",
        "crates/foundation/src/gpu_accel.rs",
        "crates/foundation/src/io_utils.rs",
        "crates/foundation/src/file_copier.rs",
        "crates/foundation/src/stream_size.rs",
        "crates/foundation/src/x265_encoder.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M24 fallback paths"
        );
    }
}

#[test]
fn media_conversion_quality_runtime_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_msssim_fallback_audit",
        "delivery_numeric_fallback_audit",
        "delivery_runtime_path_audit",
        "delivery_runtime_batch_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for quality/runtime guards (M25)"
        );
    }
    for rel in [
        "crates/foundation/src/msssim_parallel.rs",
        "crates/foundation/src/msssim_sampling.rs",
        "crates/foundation/src/float_compare.rs",
        "crates/foundation/src/crf_constants.rs",
        "crates/foundation/src/media_meta_utils.rs",
        "crates/foundation/src/common_utils.rs",
        "crates/foundation/src/error_handler.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M25 fallback paths"
        );
    }
}

#[test]
fn media_conversion_infra_numeric_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_numeric_fallback_audit",
        "delivery_runtime_path_audit",
        "delivery_runtime_batch_audit",
        "delivery_progress_batch_audit",
        "delivery_path_validate_batch_audit",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for infra/numeric guards (M26)"
        );
    }
    for rel in [
        "crates/foundation/src/numeric_cast.rs",
        "crates/foundation/src/system_memory.rs",
        "crates/foundation/src/modern_ui.rs",
        "crates/foundation/src/progress_mode.rs",
        "crates/foundation/src/path_validator.rs",
        "crates/foundation/src/date_analysis.rs",
        "crates/foundation/src/smart_file_copier.rs",
        "crates/foundation/src/ctrlc_guard.rs",
        "crates/foundation/src/lru_cache.rs",
        "crates/foundation/src/safety.rs",
        "crates/foundation/src/path_safety.rs",
        "crates/foundation/src/image_metrics.rs",
        "crates/foundation/src/x265_params.rs",
        "crates/foundation/src/file_sorter.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::"),
            "{rel} must route fallbacks through media_conversion_gate"
        );
        assert!(
            !content.contains("log_anomaly!("),
            "{rel} must not use raw log_anomaly! on M26 fallback paths"
        );
    }
}

#[test]
fn media_conversion_logging_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("delivery_logging_path_audit"),
        "gate must export delivery_logging_path_audit for session logging (M27)"
    );
    let rel = "crates/foundation/src/logging.rs";
    let content =
        fs::read_to_string(join_legacy_aware(&root, rel)).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("media_conversion_gate::delivery_logging_path_audit"),
        "{rel} must route log rotation fallbacks through media_conversion_gate"
    );
    assert!(
        !content.contains("log_anomaly!("),
        "{rel} must not use raw log_anomaly! on M27 fallback paths"
    );
}

#[test]
fn media_conversion_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "path_extension_lowercase_or_empty",
        "path_extension_lowercase_or_empty_unchecked",
        "path_extension_label",
        "meta_extension_lowercase_or_empty",
        "meta_container_lowercase_or_empty",
        "strip_prefix_or_self",
        "trace_label_or_default",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for audited routing defaults (M28)"
        );
    }
    let file_copier = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/file_copier.rs",
    ))
    .expect("file_copier.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        file_copier.contains("path_extension_lowercase_or_empty_unchecked")
            && file_copier.contains("path_extension_label")
            && !file_copier.contains("unwrap_or_default()"),
        "file_copier must use gate extension helpers, not unwrap_or_default"
    );
    let smart = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/smart_file_copier.rs",
    ))
    .expect("smart_file_copier.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        smart.contains("path_extension_lowercase_or_empty_unchecked")
            && smart.contains("strip_prefix_or_self")
            && !smart.contains("unwrap_or_default()"),
        "smart_file_copier must use gate helpers for extension and strip_prefix"
    );
    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        loop_intent.contains("meta_extension_lowercase_or_empty")
            && !loop_intent
                .contains(".source_extension\n            .as_deref()\n            .unwrap_or"),
        "loop_intent must not silently default source_extension via unwrap_or"
    );
    let lru = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/lru_cache.rs",
    ))
    .expect("lru_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        lru.contains("unix_duration_since_epoch_optional"),
        "lru_cache clock must use gate optional epoch SSOT (no fabricated zero duration)"
    );
}

#[test]
fn media_conversion_resume_gpu_cli_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "unix_epoch_secs_or_zero",
        "unix_duration_since_epoch_or_zero",
        "gpu_concurrency_max_or_default",
        "gpu_output_extension_segment",
        "pipeline_outcome_reason",
        "db_labeled_by_or_default",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for resume/GPU/CLI unwrap-or guards (M29)"
        );
    }
    let checkpoint = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/checkpoint.rs",
    ))
    .expect("checkpoint.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        checkpoint.contains("unix_epoch_secs_optional")
            && checkpoint.contains("unix_duration_since_epoch_optional")
            && !checkpoint.contains("unix_epoch_secs_or_zero")
            && !checkpoint.contains("unix_duration_since_epoch_or_zero")
            && !checkpoint
                .contains("duration_since(UNIX_EPOCH)\n        .unwrap_or(Duration::ZERO)"),
        "checkpoint timestamps must use optional epoch SSOT (no audited-zero fabrication)"
    );
    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/gpu_accel.rs",
    ))
    .expect("gpu_accel.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gpu.contains("gpu_concurrency_max_or_default")
            && gpu.contains("gpu_output_extension_segment")
            && !gpu.contains(r#".unwrap_or("MP4")"#),
        "gpu_accel must use gate helpers for concurrency and temp extension"
    );
    let cli = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/cli_runner.rs",
    ))
    .expect("cli_runner.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cli.contains("pipeline_outcome_reason")
            && cli.contains("path_file_name_for_log")
            && !cli.contains("file_name().unwrap_or_default()"),
        "cli_runner must use gate outcome and file name helpers"
    );
    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("path_extension_lowercase_or_empty_unchecked")
            && database.contains("db_labeled_by_or_default")
            && !database.contains("labeled_by.as_deref().unwrap_or(\"loop_samples_refresh\")"),
        "database ingest must use gate extension and labeled_by helpers"
    );
}

#[test]
fn media_conversion_metadata_encode_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "path_stem_root_segment",
        "path_parent_or_dot",
        "path_file_stem_or_empty",
        "path_file_name_or_empty",
        "encode_stderr_last_line_or_unknown",
        "dv_profile8_compat_id_or_default",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for metadata/encode unwrap-or guards (M30)"
        );
    }
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/xmp_merger.rs",
            &[
                "path_stem_root_segment",
                "path_file_stem_or_empty",
                "path_parent_or_dot",
            ][..],
            &["file_stem().and_then(|s| s.to_str()).unwrap_or(\"\")"][..],
        ),
        (
            "crates/foundation/src/metadata/mod.rs",
            &["strip_prefix_or_self", "path_file_stem_or_empty"][..],
            &["strip_prefix(src_dir).unwrap_or(src_path)"][..],
        ),
        (
            "crates/foundation/src/x265_encoder.rs",
            &[
                "path_extension_lowercase_or_empty_unchecked",
                "encode_stderr_last_line_or_unknown",
            ][..],
            &["stderr.lines().last().unwrap_or(\"Unknown error\")"][..],
        ),
        (
            "crates/foundation/src/hdr.rs",
            &["dv_profile8_compat_id_or_default"][..],
            &["compat_id.unwrap_or(crate::constants::DV_PROFILE8_DEFAULT_COMPAT_ID)"][..],
        ),
        (
            "crates/foundation/src/jxl_utils.rs",
            &["encode_stderr_last_line_or_unknown"][..],
            &["magick_stderr.lines().next().unwrap_or(\"\")"][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        for needle in needles {
            assert!(
                content.contains(needle),
                "{rel} must use gate helper {needle} (M30)"
            );
        }
        for ban in forbidden {
            assert!(
                !content.contains(ban),
                "{rel} must not use silent {ban} (M30)"
            );
        }
    }
}

#[test]
fn media_conversion_jpeg_explore_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_jpeg_byte_at",
        "probe_jpeg_buffer_slice",
        "jpeg_weighted_quality_or_luma",
        "explore_metric_numeric_end",
        "probe_ffprobe_codec_name_lowercase",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for JPEG/explore unwrap-or guards (M31)"
        );
    }
    let jpeg = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_jpeg_analysis.rs",
    ))
    .expect("image_jpeg_analysis.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let jpeg_prod = jpeg.split("#[cfg(test)]").next().unwrap_or(&jpeg);
    assert!(
        jpeg.contains("probe_jpeg_byte_at")
            && jpeg.contains("probe_jpeg_buffer_slice")
            && jpeg.contains("jpeg_weighted_quality_or_luma")
            && !jpeg_prod.contains(".copied().unwrap_or(JPEG_MISSING_BYTE)")
            && !jpeg_prod.contains(".unwrap_or(&[])"),
        "image_jpeg_analysis production paths must use gate JPEG helpers"
    );
    let precheck = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precheck.rs",
    ))
    .expect("precheck.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        precheck.contains("probe_ffprobe_codec_name_lowercase"),
        "precheck must use gate codec_name helper"
    );
    for rel in [
        "crates/foundation/src/explore_strategy.rs",
        "crates/foundation/src/video_explorer.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        assert!(
            content.contains("parse_explore_ssim_metric_token")
                || content.contains("parse_explore_psnr_metric_token"),
            "{rel} must use central explore metric parsers (M31/M74)"
        );
        assert!(
            !prod.contains(".unwrap_or(value_str.len())"),
            "{rel} production must not parse metric tokens via silent unwrap_or(len)"
        );
    }
    let detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let detection_prod = detection.split("#[cfg(test)]").next().unwrap_or(&detection);
    assert!(
        detection.contains("probe_jpeg_buffer_slice")
            && !detection_prod.contains(".get(pos + 4..pos + 8).unwrap_or(&[])"),
        "image_detection JP2 probe must use gate buffer slice helper"
    );
}

#[test]
fn media_conversion_gpu_ssim_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_metric_numeric_end",
        "x265_params_segment_or_empty",
        "explore_best_crf_or_backtrack_anchor",
        "backup_extension_label_or_tmp",
        "probe_buffer_prefix_or_empty",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for GPU/SSIM explore unwrap-or guards (M32)"
        );
    }
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            &[
                "path_extension_lowercase_or_empty_unchecked",
                "x265_params_segment_or_empty",
                "explore_best_crf_or_backtrack_anchor",
                "parse_explore_ssim_metric_token",
            ][..],
            &[
                ".unwrap_or(after_all.len())",
                "best_crf.unwrap_or(",
                ".unwrap_or(\"tmp\")",
            ][..],
        ),
        (
            "crates/foundation/src/video_explorer/ssim_calculator.rs",
            &["explore_metric_numeric_end"][..],
            &[".unwrap_or(after_colon.len())", ".unwrap_or(after.len())"][..],
        ),
        (
            "crates/foundation/src/video_explorer/stream_analysis.rs",
            &["parse_explore_ssim_metric_token"][..],
            &[".unwrap_or(after_all.len())", ".unwrap_or(after.len())"][..],
        ),
        (
            "crates/foundation/src/hdr.rs",
            &["probe_buffer_prefix_or_empty"][..],
            &["data.get(..end).unwrap_or(&[])"][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        for needle in needles {
            assert!(
                content.contains(needle),
                "{rel} must use gate helper {needle} (M32)"
            );
        }
        for ban in forbidden {
            assert!(
                !prod.contains(ban),
                "{rel} production must not use silent {ban} (M32)"
            );
        }
    }
}

#[test]
fn media_conversion_probe_intel_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_stdout_first_token",
        "probe_optional_f64_or_zero",
        "probe_animated_color_richness_unit_interval",
        "animated_delay_variation_or_default",
        "probe_ffprobe_optional_string",
        "path_extension_lossy_or_empty",
        "utf8_prefix_or_empty",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for probe/quality intel unwrap-or guards (M33)"
        );
    }
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/common_utils.rs",
            &[
                "path_extension_lowercase_or_empty_unchecked",
                "unix_epoch_secs_optional",
            ][..],
            &[".unwrap_or_default()", "unix_epoch_secs_or_zero"][..],
        ),
        (
            "crates/foundation/src/ffprobe.rs",
            &["probe_ffprobe_optional_string"][..],
            &["codec_long_name\"]\n        .as_str()\n        .unwrap_or(\"\")"][..],
        ),
        (
            "crates/foundation/src/animated_image_quality_features.rs",
            &[
                "probe_animated_color_richness_unit_interval",
                "animated_delay_variation_or_default",
                "quality_embedding_optional_unit_interval_f32",
            ][..],
            &[
                ".unwrap_or_default()",
                ".unwrap_or(if probe.is_variable_frame_rate",
                "palette_size.map_or_else",
                "probe_optional_f64_or_zero",
            ][..],
        ),
        (
            "crates/foundation/src/media_penetration.rs",
            &["probe_stdout_first_token"][..],
            &[".split_whitespace().next().unwrap_or(\"\")"][..],
        ),
        (
            "crates/foundation/src/scenario_quality_lookup.rs",
            &["probe_image_format_audit"][..],
            &["detect_animation(path, &format).unwrap_or((false, None, None))"][..],
        ),
        (
            "crates/foundation/src/video_quality_features.rs",
            &["quality_embedding_optional_unit_interval_f32"][..],
            &[".unwrap_or(0.0)", "probe_optional_f64_or_zero"][..],
        ),
        (
            "crates/foundation/src/depth_channel.rs",
            &["path_extension_lossy_or_empty"][..],
            &[".to_string_lossy().to_string()"][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must use gate helper {needle} (M33)"
            );
        }
        for ban in forbidden {
            assert!(
                !prod.contains(ban),
                "{rel} production must not use silent {ban} (M33)"
            );
        }
    }
}

#[test]
fn media_conversion_db_precision_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "db_physics_embedding_or_empty",
        "db_optional_bool_or_false",
        "db_optional_string_or_empty",
        "utf8_suffix_or_empty",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for DB/precision unwrap-or guards (M34)"
        );
    }
    for (rel, needles, forbidden, use_full_file) in [
        (
            "crates/foundation/src/media_precision.rs",
            &["path_extension_lowercase_or_empty_unchecked"][..],
            &[".map(str::to_ascii_lowercase)"][..],
            false,
        ),
        (
            "crates/foundation/src/database.rs",
            &["db_physics_embedding_or_empty", "trace_label_or_default"][..],
            &[
                "physics_225.as_deref().unwrap_or(&[])",
                "resolution_path.unwrap_or(\"\")",
            ][..],
            true,
        ),
        (
            "crates/foundation/src/multi_scenario_db.rs",
            &["db_optional_bool_or_false", "db_optional_string_or_empty"][..],
            &[
                "has_audio.unwrap_or(false)",
                "is_variable_frame_rate.unwrap_or(false)",
                "is_hdr.unwrap_or(false)",
                ".unwrap_or_default()",
            ][..],
            false,
        ),
        (
            "crates/foundation/src/loop_intent.rs",
            &["utf8_suffix_or_empty", "probe_stdout_first_token"][..],
            &[".get(idx + 23..).unwrap_or(\"\")"][..],
            false,
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        let scope = if use_full_file { &content } else { prod };
        for needle in needles {
            assert!(
                scope.contains(needle),
                "{rel} must use gate helper {needle} (M34)"
            );
        }
        for ban in forbidden {
            assert!(
                !scope.contains(ban),
                "{rel} must not use silent {ban} (M34)"
            );
        }
    }
}

#[test]
fn media_conversion_quality_ui_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "db_numeric_stats_triple_or_none",
        "db_numeric_stats_triple_or_zero",
        "ui_spinner_glyph_at",
        "ui_optional_f64_display_suffix",
        "ui_optional_crf_display_suffix",
        "path_relative_parent_or_self",
        "gpu_vaapi_device_path_or_default",
        "quality_embedding_optional_f64_or_zero",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for quality/UI unwrap-or guards (M35)"
        );
    }
    for (rel, needles, forbidden, use_full_file) in [
        (
            "crates/foundation/src/image_quality_db.rs",
            &["quality_embed_measured_dimension_f32"][..],
            &[
                "analysis.psnr.map(f64_to_f32_feature).unwrap_or_default()",
                "analysis.ssim.map(f64_to_f32_feature).unwrap_or_default()",
                "quality_embedding_optional_f64_or_zero(analysis.psnr",
                "quality_embedding_optional_f64_or_zero(analysis.ssim",
                "probe_optional_f64_or_zero",
                "infer_quality_embedding_psnr_ssim",
            ][..],
            true,
        ),
        (
            "crates/foundation/src/database.rs",
            &["numeric_summary(&sizes)"][..],
            &[
                "numeric_summary(&sizes).unwrap_or((0.0, 0.0, 0.0))",
                "db_numeric_stats_triple_or_zero(",
            ][..],
            true,
        ),
        (
            "crates/foundation/src/modern_ui.rs",
            &["ui_spinner_glyph_at", "ui_optional_f64_display_suffix"][..],
            &[".unwrap_or(\"-\")", ".unwrap_or(\"*\")"][..],
            false,
        ),
        (
            "crates/foundation/src/unified_progress.rs",
            &["ui_optional_f64_display_suffix"][..],
            &["final_ssim\n            .map(|s| format!(\"SSIM"][..],
            false,
        ),
        (
            "crates/foundation/src/gpu_accel.rs",
            &["gpu_vaapi_device_path_or_default"][..],
            &[".unwrap_or_else(|_| \"/dev/dri/renderD128\".to_string())"][..],
            false,
        ),
        (
            "crates/foundation/src/metadata/mod.rs",
            &["path_relative_parent_or_self"][..],
            &["rel.parent().unwrap_or(rel)"][..],
            false,
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        let scope = if use_full_file { &content } else { prod };
        for needle in needles {
            assert!(
                scope.contains(needle),
                "{rel} must use gate helper {needle} (M35)"
            );
        }
        for ban in forbidden {
            assert!(
                !scope.contains(ban),
                "{rel} must not use silent {ban} (M35)"
            );
        }
    }
}

#[test]
fn media_conversion_runtime_tooling_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_run_logs_dir_or_dot",
        "delivery_cwd_display_or_unknown",
        "delivery_disk_check_path_or_input",
        "delivery_tool_executable_or_default",
        "str_first_segment_or_whole",
        "db_sorted_distance_at",
        "db_feature_weight_or_default",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for runtime/tooling unwrap-or guards (M36)"
        );
    }
    for (rel, needles, forbidden, use_full_file) in [
        (
            "crates/foundation/src/progress_mode.rs",
            &["unified_log_dir"][..],
            &["current_dir()\n            .unwrap_or_else(|_| std::path::PathBuf::from(\".\")"][..],
            false,
        ),
        (
            "crates/foundation/src/logging.rs",
            &["delivery_cwd_display_or_unknown"][..],
            &["current_dir()\n        .unwrap_or_default()"][..],
            true,
        ),
        (
            "crates/foundation/src/cli_runner.rs",
            &["delivery_disk_check_path_or_input"][..],
            &["config.output.as_deref().unwrap_or(input)"][..],
            false,
        ),
        (
            "crates/foundation/src/tool_builders.rs",
            &["delivery_tool_executable_or_default"][..],
            &["executable.as_deref().unwrap_or(constants::TOOL_RSYNC)"][..],
            false,
        ),
        (
            "crates/foundation/src/date_analysis.rs",
            &["str_first_segment_or_whole"][..],
            &["split('+').next().unwrap_or(date_str)"][..],
            false,
        ),
        (
            "crates/foundation/src/database_vector.rs",
            &["db_feature_weight_optional"][..],
            &[
                "db_feature_weight_or_default",
                ".unwrap_or(crate::constants::KNN_VECTOR_DEFAULT_FEATURE_WEIGHT)",
            ][..],
            false,
        ),
        (
            "crates/foundation/src/database.rs",
            &["db_sorted_distance_at"][..],
            &["distances.get(distances.len() / 4).unwrap_or(&distances[0])"][..],
            true,
        ),
        (
            "crates/foundation/src/gpu_accel.rs",
            &[] as &[&str],
            &[
                ".get(i).unwrap_or(&(0.0_f64, 0.0_f64))",
                ".first().unwrap_or(&(0.0, 0.0))",
                ".last().unwrap_or(&(0.0, 0.0))",
            ][..],
            false,
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        let scope = if use_full_file { &content } else { prod };
        for needle in needles {
            assert!(
                scope.contains(needle),
                "{rel} must use gate helper {needle} (M36)"
            );
        }
        for ban in forbidden {
            assert!(
                !scope.contains(ban),
                "{rel} must not use silent {ban} (M36)"
            );
        }
    }
}

#[test]
fn media_conversion_loop_numeric_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "algorithm_feature_distribution_required",
        "loop_optional_secs_or_baseline",
        "json_finite_f64_or_null",
        "f64_sort_cmp",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for loop/numeric unwrap-or guards (M37)"
        );
    }
    for (rel, needles, forbidden, use_full_file) in [
        (
            "crates/foundation/src/loop_intent.rs",
            &[
                "for_evaluation",
                "from_reference_profile",
                "loop_collection_duration_p90_or_baseline",
                "f64_sort_cmp",
                "Verdict::Uncertain",
            ][..],
            &[
                "from_legacy_constants",
                "loop_reference_profile_or_default",
                "reference.cloned().unwrap_or_default()",
                ".unwrap_or(crate::constants::DEFAULT_LOOP_BASELINE_DURATION_SECS)",
                ".unwrap_or(crate::constants::MODERN_FORMAT_VIDEO_BIAS_THRESHOLD_SECS)",
            ][..],
            false,
        ),
        (
            "crates/foundation/src/database.rs",
            &[
                "json_inference_optional_f64_or_null",
                "json_finite_f64_or_null",
                "f64_sort_cmp",
            ][..],
            &[
                ".unwrap_or(json!(null))",
                "partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)",
            ][..],
            true,
        ),
        (
            "crates/foundation/src/gpu_accel.rs",
            &["f64_sort_cmp"][..],
            &["partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)"][..],
            false,
        ),
        (
            "crates/foundation/src/media_conversion_gate.rs",
            &["algorithm_feature_distribution_required"][..],
            &[
                "loop_reference_profile_or_default",
                "algorithm_feature_distribution_or_fallback",
                "delivery_db_bpp_frame_count_f64_or_one",
            ][..],
            false,
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        let scope = if use_full_file { &content } else { prod };
        for needle in needles {
            assert!(
                scope.contains(needle),
                "{rel} must use gate helper {needle} (M37)"
            );
        }
        for ban in forbidden {
            assert!(
                !scope.contains(ban),
                "{rel} must not use silent {ban} (M37)"
            );
        }
    }
}

#[test]
fn media_conversion_inference_snapshot_unwrap_or_strict_defaults() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "json_required_finite_f64_or_null",
        "json_optional_i32_or_null",
        "json_optional_bool_or_null",
        "json_optional_string_or_null",
        "loop_duration_or_fallback",
        "algorithm_env_usize_or_default",
        "io_error_or_metadata_label",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for inference snapshot guards (M38)"
        );
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 38),
        "contract must document M38 inference snapshot guards"
    );
    for (rel, needles, forbidden, use_full_file) in [
        (
            "crates/foundation/src/database.rs",
            &[
                "json_finite_f64_or_null",
                "json_required_finite_f64_or_null",
                "json_inference_optional_i32_or_null",
                "json_inference_optional_bool_or_null",
                "json_inference_optional_string_or_null",
            ][..],
            &[
                "map_or_else(|| json!(null), |v| if v.is_finite()",
                "map_or_else(|| json!(null), |id| json!(id))",
                "map_or_else(|| json!(null), |s| json!(s))",
                "\"layer6b_resolved\": crate::media_conversion_gate::json_optional_bool_or_null",
            ][..],
            true,
        ),
        (
            "crates/foundation/src/loop_intent.rs",
            &["loop_duration_or_fallback"][..],
            &[".p75\n            .unwrap_or(collection_duration_p90)"][..],
            false,
        ),
        (
            "crates/foundation/src/algorithm_runtime.rs",
            &["algorithm_env_usize_or_default"][..],
            &[".unwrap_or(crate::constants::LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS)"][..],
            true,
        ),
        (
            "crates/foundation/src/metadata/mod.rs",
            &["io_error_or_metadata_label"][..],
            &["first_error.unwrap_or_else(|| io::Error::other("][..],
            false,
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        let scope = if use_full_file { &content } else { prod };
        for needle in needles {
            assert!(
                scope.contains(needle),
                "{rel} must use gate helper {needle} (M38)"
            );
        }
        for ban in forbidden {
            assert!(
                !scope.contains(ban),
                "{rel} must not use silent {ban} (M38)"
            );
        }
    }
}

#[test]
fn media_conversion_blind_spot_guards_m40() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_chroma_factor_or_default",
        "explore_encode_size_improvement_pct",
        "probe_compression_ratio_or_estimate",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for M40 blind-spot guards"
        );
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 40),
        "contract must document M40 blind-spot guards"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/quality_matcher.rs",
            &["probe_chroma_factor_optional"][..],
            &[
                "pix_fmt.map_or(1.0, |fmt| {",
                "probe_chroma_factor_or_default",
            ][..],
        ),
        (
            "crates/foundation/src/gpu_accel.rs",
            &["explore_encode_size_improvement_pct_optional"][..],
            &[
                "state.best_size.map_or(100.0, |best| {",
                "explore_encode_size_improvement_pct(",
            ][..],
        ),
        (
            "crates/foundation/src/animated_image_quality_features.rs",
            &["probe_compression_ratio_or_estimate"][..],
            &[".unwrap_or_else(|| estimate_compression_ratio(bytes_per_pixel, frame_count))"][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must use gate helper {needle} (M40)"
            );
        }
        for ban in forbidden {
            assert!(!prod.contains(ban), "{rel} must not use silent {ban} (M40)");
        }
    }
}

#[test]
fn media_conversion_explore_jxl_guards_m41() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_ultimate_gate_sample_rate",
        "explore_latest_encoded_size_optional",
        "explore_latest_encoded_size_or_zero",
        "explore_elapsed_secs_optional",
        "explore_elapsed_secs_or_zero",
        "explore_dynamic_mapping_offset_or_zero",
        "explore_gif_frame_count_optional",
        "explore_webp_frame_count_optional",
        "jxl_best_telemetry_optional",
        "jxl_best_telemetry_or_zero",
        "jxl_screened_output_size_or_max",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for M41 explore/JXL guards"
        );
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 41),
        "contract must document M41 explore/JXL guards"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            &["explore_ultimate_gate_sample_rate"][..],
            &[
                "duration_hint.map_or(1, ultimate_final_sample_rate)",
                "fn ultimate_final_sample_rate",
            ][..],
        ),
        (
            "crates/foundation/src/video_explorer.rs",
            &[
                "explore_latest_encoded_size_optional",
                "explore_elapsed_secs_optional",
            ][..],
            &[
                "size_history.last().map_or(0, |(_, size)| *size)",
                ".map_or(0.0_f64, |t| t.elapsed().as_secs_f64())",
            ][..],
        ),
        (
            "crates/foundation/src/video_explorer/dynamic_mapping.rs",
            &[
                "explore_dynamic_mapping_offset_or_zero",
                "explore_calibration_duration_optional",
            ][..],
            &[
                ".map_or(0.0, |a| Self::calculate_offset_from_ratio(a.size_ratio))",
                ".unwrap_or_else(|| f64::from(sample_duration))",
            ][..],
        ),
        (
            "crates/foundation/src/jxl_explorer.rs",
            &[
                "jxl_best_telemetry_optional",
                "jxl_screened_output_size_or_max",
            ][..],
            &[
                "best_candidate.map_or((0.0, 0), |c| (c.distance, c.output_size))",
                ".map_or(u64::MAX, |candidate| candidate.output_size)",
            ][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must use gate helper {needle} (M41)"
            );
        }
        for ban in forbidden {
            assert!(!prod.contains(ban), "{rel} must not use silent {ban} (M41)");
        }
    }
}

#[test]
fn media_conversion_progress_loop_guards_m42() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "progress_explore_crf_or_zero",
        "progress_explore_size_or_zero",
        "progress_explore_ssim_or_zero",
        "loop_missing_duration_z_neutral",
        "loop_extended_short_tail_headroom_or_zero",
        "loop_modern_bias_overflow_or_zero",
        "loop_short_proximity_ramp_or_zero",
        "loop_long_proximity_ramp_or_one",
        "loop_baseline_median_frames_or_zero",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for M42 progress/loop guards"
        );
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 42),
        "contract must document M42 progress/loop guards"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/progress.rs",
            &[
                "progress_explore_crf_or_zero",
                "progress_explore_size_or_zero",
                "progress_explore_ssim_or_zero",
                "ui_f32_display_or_placeholder",
            ][..],
            &[
                "current_crf.lock().map_or(0.0, |c| *c)",
                "current_size.lock().map_or(0, |s| *s)",
                "best_crf.lock().map_or(0.0, |c| *c)",
                "best_ssim.lock().map_or(0.0",
                "best_crf > 0.0",
            ][..],
        ),
        (
            "crates/foundation/src/loop_intent.rs",
            &[
                "loop_duration_z_or_neutral",
                "loop_extended_short_tail_headroom_optional",
                "loop_modern_bias_overflow_optional",
                "loop_short_proximity_ramp_optional",
                "loop_long_proximity_ramp_or_one",
                "loop_baseline_median_frames_optional",
            ][..],
            &[
                "duration_secs.map_or(0.0, |d| Self::clamp_z(self.reference.duration.z_score(d)))",
                "let tail_headroom = self.meta.duration_secs.map_or(0.0_f64, |duration| {",
                "let overflow = self.meta.duration_secs.map_or(0.0_f64, |duration| {",
                ".map_or(0.0_f64, |dur| 1.0_f64 - (dur - short_veto) / short_buf)",
                ".map_or(1.0_f64, |dur| (dur - long_ramp_bottom) / long_buf)",
                ".map_or(0.0_f64, crate::numeric_cast::u64_to_f64)",
            ][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must use gate helper {needle} (M42)"
            );
        }
        for ban in forbidden {
            assert!(!prod.contains(ban), "{rel} must not use silent {ban} (M42)");
        }
    }
}

#[test]
fn media_conversion_final_allowlist_cleared_m43() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "runtime_elapsed_secs_or_zero",
        "gif_palette_byte_size_or_zero",
        "gpu_compression_potential_adjustment_or_zero",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for M43 final allowlist clearance"
        );
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 43),
        "contract must document M43 final allowlist clearance"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/ctrlc_guard.rs",
            &["runtime_elapsed_secs_or_zero"][..],
            &["START_INSTANT.get().map_or(0, |t| t.elapsed().as_secs())"][..],
        ),
        (
            "crates/foundation/src/media_meta_utils.rs",
            &["gif_palette_byte_size_optional"][..],
            &["palette_size.map_or(0, |p| p as usize * 3)"][..],
        ),
        (
            "crates/foundation/src/gpu_accel.rs",
            &["gpu_compression_potential_adjustment_optional"][..],
            &["compression_potential.map_or(0.0, |potential| {"][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = content.split("#[cfg(test)]").next().unwrap_or(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must use gate helper {needle} (M43)"
            );
        }
        for ban in forbidden {
            assert!(!prod.contains(ban), "{rel} must not use silent {ban} (M43)");
        }
    }
    let offenders = delivery_numeric_forgery_offenders(&root);
    assert!(
        offenders.is_empty(),
        "M43 requires zero unallowlisted numeric-forgery lines:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn media_conversion_session_mutex_hardening_m44() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "logging_mutex_guard_or_recover",
        "path_tracing_log_file_name_or_app_log",
        "ui_result_box_width_or_title_default",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for M44 session/UI path hardening"
        );
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 44),
        "contract must document M44"
    );
    for rel in [
        "crates/foundation/src/logging.rs",
        "crates/foundation/src/progress_mode.rs",
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("logging_mutex_guard_or_recover"),
            "{rel} must use logging_mutex_guard_or_recover (M44)"
        );
        let poison = offending_lines(&root, &[path], &[".lock().unwrap_or_else(|err|"]);
        assert!(
            poison.is_empty(),
            "{rel} must not use inline poison recovery outside tests (M44):\n{}",
            poison.join("\n")
        );
    }
    let logging = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/logging.rs"))
        .expect("logging.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logging.contains("delivery_logging_path_audit"),
        "logging.rs must audit invalid RUST_LOG via delivery_logging_path_audit (M44)"
    );
    let modern_path = join_legacy_aware(&root, "crates/foundation/src/modern_ui.rs");
    let modern = fs::read_to_string(&modern_path).expect("modern_ui.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for needle in [
        "path_parent_or_dot",
        "path_tracing_log_file_name_or_app_log",
        "ui_result_box_width_or_title_default",
    ] {
        assert!(
            modern.contains(needle),
            "modern_ui.rs must use gate helper {needle} (M44/U11)"
        );
    }
    let ui_poison = offending_lines(&root, &[modern_path], &["path.parent().unwrap_or_else"]);
    assert!(
        ui_poison.is_empty(),
        "modern_ui.rs must not use raw path.parent unwrap_or (U11):\n{}",
        ui_poison.join("\n")
    );
}

#[test]
fn media_conversion_path_and_log_config_m45() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_log_dir_from_env_or_temp",
        "delivery_scratch_temp_dir_or_system_temp",
        "path_magick_relativized_lossy",
        "path_search_temp_stem_or_output",
        "path_search_temp_ext_or_tmp",
        "algorithm_env_flag_enabled_or_default",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M45");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 45),
        "contract must document M45"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/logging.rs",
            &["delivery_log_dir_from_env_or_temp", "unified_log_dir"][..],
            &["std::env::temp_dir()", "workspace_logs_dir_from_cwd"][..],
        ),
        (
            "crates/foundation/src/path_safety.rs",
            &[
                "path_magick_relativized_lossy",
                "path_search_temp_stem_or_output",
                "path_search_temp_ext_or_tmp",
            ][..],
            &["std::env::current_dir().map_or_else"][..],
        ),
        (
            "crates/foundation/src/progress.rs",
            &["delivery_progress_mutex_string_or_empty"][..],
            &["progress_message_mutex_poisoned"][..],
        ),
        (
            "crates/foundation/src/c_api.rs",
            &["mutex_guard_or_recover"][..],
            &["LAST_INGEST_ERR.lock().map_or_else"][..],
        ),
        (
            "crates/foundation/src/loop_intent.rs",
            &[
                "delivery_scratch_temp_dir_or_system_temp",
                "algorithm_env_flag_enabled_or_default",
            ][..],
            &["std::env::temp_dir()", ".map_or(true,"][..],
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        for needle in needles {
            assert!(content.contains(needle), "{rel} must use {needle} (M45)");
        }
        let hits = offending_lines(&root, &[path], forbidden);
        assert!(
            hits.is_empty(),
            "{rel} must not retain forbidden fallback after M45:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_progress_and_log_detail_m46() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "progress_explore_optional_f64_or_none",
        "delivery_log_detail_with_optional_path",
        "ui_ssim_inline_or_na",
        "ui_f64_display_or_placeholder",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M46");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 46),
        "contract must document M46"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/progress.rs",
            &[
                "mutex_guard_or_recover",
                "progress_explore_optional_f64_or_none",
                "ui_f64_display_or_placeholder",
            ][..],
            &[
                concat!("current_ssim.lock().", "ok", "()"),
                concat!("ACTIVE_PROGRESS_LINE.lock().", "ok", "()"),
            ][..],
        ),
        (
            "crates/foundation/src/static_logs.rs",
            &["delivery_log_detail_with_optional_path"][..],
            &["path.map_or_else"][..],
        ),
        (
            "crates/foundation/src/unified_progress.rs",
            &["ui_ssim_inline_or_na"][..],
            &["ssim.map_or_else(|| \"N/A\""][..],
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        for needle in needles {
            assert!(content.contains(needle), "{rel} must use {needle} (M46)");
        }
        let hits = offending_lines(&root, &[path], forbidden);
        assert!(
            hits.is_empty(),
            "{rel} must not retain forbidden fallback after M46:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_explore_metric_display_m47() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ui_f64_or_na",
        "ui_f64_pair_slash_or_na",
        "ui_f64_pair_labeled_or_na",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M47");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 47),
        "contract must document M47"
    );
    for rel in [
        "crates/foundation/src/video_explorer.rs",
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
        "crates/foundation/src/image_analyzer.rs",
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("ui_f64_or_na") || content.contains("ui_optional_f64_display_or_map"),
            "{rel} must use gate explore metric helpers (M47)"
        );
        let hits = offending_lines(&root, &[path], MC_FORBIDDEN_MAP_OR_NA);
        assert!(
            hits.is_empty(),
            "{rel} must not use inline N/A map_or_else after M47:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_quality_intel_metric_display_m48() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ui_optional_u32_or_na",
        "ui_optional_u64_or_na",
        "ui_f64_percent_or_na",
        "ui_duration_secs_label_or_na",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M48");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 48),
        "contract must document M48"
    );
    for rel in [
        "crates/foundation/src/image_quality_detector.rs",
        "crates/foundation/src/video_quality_detector.rs",
        "crates/foundation/src/image_jpeg_analysis.rs",
        "crates/foundation/src/quality_verifier_enhanced.rs",
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("ui_f64_or_na")
                || content.contains("ui_optional_u32_or_na")
                || content.contains("ui_duration_secs_label_or_na"),
            "{rel} must use gate quality-intel metric helpers (M48)"
        );
        let hits = offending_lines(&root, &[path], MC_FORBIDDEN_MAP_OR_NA);
        assert!(
            hits.is_empty(),
            "{rel} must not use inline N/A map_or_else after M48:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_confidence_and_terminal_m49() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ui_confidence_pct_whole_or_na",
        "ui_confidence_scale100_one_decimal_or_na",
        "ui_bit_depth_format_label_or_na",
        "ui_metric_not_applicable_label",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M49");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 49),
        "contract must document M49"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/quality_matcher.rs",
            &["ui_confidence_pct_whole_or_na"][..],
            MC_FORBIDDEN_NA_STRING,
        ),
        (
            "crates/foundation/src/image_detection.rs",
            &["ui_confidence_scale100_one_decimal_or_na"][..],
            MC_FORBIDDEN_NA_STRING,
        ),
        (
            "crates/foundation/src/media_precision.rs",
            &["ui_bit_depth_format_label_or_na"][..],
            MC_FORBIDDEN_NONE_NA,
        ),
        (
            "crates/foundation/src/ctrlc_guard.rs",
            &["mutex_guard_or_recover"][..],
            MC_FORBIDDEN_MUTEX_OK,
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        for needle in needles {
            assert!(content.contains(needle), "{rel} must use {needle} (M49)");
        }
        let hits = offending_lines(&root, &[path], forbidden);
        assert!(
            hits.is_empty(),
            "{rel} must not retain forbidden fallback after M49:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_probe_stderr_m50() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_imagemagick_animation_detected_audit",
        "ui_probe_stats_stderr",
        "ui_penetration_warning_stderr",
        "ui_optional_u32_display_or_unknown",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M50");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 50),
        "contract must document M50"
    );
    let analyzer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        analyzer.contains("probe_imagemagick_animation_detected_audit")
            && analyzer.contains("ui_probe_stats_stderr"),
        "image_analyzer must route probe stderr through gate (M50)"
    );
    let detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        detection.contains("ui_penetration_warning_stderr"),
        "image_detection penetration warnings must use gate (M50)"
    );
    for rel in [
        "crates/foundation/src/image_analyzer.rs",
        "crates/foundation/src/image_detection.rs",
    ] {
        let path = join_legacy_aware(&root, rel);
        let hits = offending_lines(&root, &[path], MC_FORBIDDEN_M50_EMOJI);
        assert!(
            hits.is_empty(),
            "{rel} must not emit raw probe/penetration emoji literals after M50:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_quality_user_errors_m51() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ui_user_facing_error",
        "ui_quality_user_error",
        "ui_log_summary_title_with_icon",
        "ui_visual_artifact_audit_title",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M51");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 51),
        "contract must document M51"
    );
    let video_q = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_quality_detector.rs",
    ))
    .expect("video_quality_detector.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        video_q.contains("ui_quality_user_error"),
        "video_quality_detector must use ui_quality_user_error (M51)"
    );
    let image_q = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_quality_detector.rs",
    ))
    .expect("image_quality_detector.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        image_q.contains("ui_visual_artifact_audit_title"),
        "image_quality_detector report header must use gate (M51)"
    );
    for rel in [
        "crates/foundation/src/video_quality_detector.rs",
        "crates/foundation/src/image_quality_detector.rs",
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            !content.contains("log_summary_header!(&\"🔍"),
            "{rel} must not use raw emoji in log_summary_header literals (M51)"
        );
        let hits = offending_lines(&root, &[path], &["\"❌"]);
        assert!(
            hits.is_empty(),
            "{rel} must not retain inline ❌ user error literals after M51:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_infra_user_errors_m52() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("ui_user_facing_error") && gate.contains("ui_log_summary_title_with_icon"),
        "gate must export infra error helpers for M52"
    );
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 52),
        "contract must document M52"
    );
    for (rel, needle) in [
        (
            "crates/foundation/src/path_validator.rs",
            "ui_user_facing_error",
        ),
        (
            "crates/foundation/src/multi_scenario_db.rs",
            "ui_user_facing_error",
        ),
        (
            "crates/foundation/src/cli_runner.rs",
            "ui_user_facing_error",
        ),
        (
            "crates/foundation/src/flag_validator.rs",
            "ui_user_facing_error",
        ),
        (
            "crates/foundation/src/pure_media_verifier.rs",
            "ui_user_facing_error",
        ),
        (
            "crates/foundation/src/video_explorer.rs",
            "ui_log_summary_title_with_icon",
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains(needle),
            "{rel} must route infra user errors through gate ({needle}, M52)"
        );
        let hits = offending_lines(&root, &[path], MC_FORBIDDEN_M52_BAIL);
        assert!(
            hits.is_empty(),
            "{rel} must not retain raw bail/error emoji after M52:\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_precision_preservation_policy_m67() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ui_bit_depth_format_label_or_na",
        "path_extension_lowercase_or_empty_unchecked",
        "probe_animated_color_richness_unit_interval",
        "precision_still_pipe_rgb_pix_fmt",
        "precision_png16_decode_rgb_pix_fmt",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M67");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 67),
        "contract must document M67"
    );

    for (rel, needles, scan_unwrap_or) in [
        (
            "crates/foundation/src/media_precision.rs",
            &[
                "trait MediaPrecision",
                "ImagePrecisionProfile",
                "hevc_yuv420_output_pix_fmt",
                "still_pipe_rgb_pix_fmt",
                "png16_decode_rgb_pix_fmt_name",
                "PIX_FMT_YUV420P10LE",
                "PIX_FMT_RGB48LE",
                "ui_bit_depth_format_label_or_na",
            ][..],
            true,
        ),
        (
            "crates/foundation/src/ffprobe_json.rs",
            &[
                "BitDepthMetadata",
                "bit_depth_metadata",
                "has_confirmed_high_bit_depth",
                "should_preserve_high_bit_depth",
                "media_precision::ImagePrecisionProfile::from_media_context",
            ][..],
            true,
        ),
        (
            "crates/foundation/src/animated_image_quality_features.rs",
            &["probe_animated_color_richness_unit_interval"][..],
            true,
        ),
        (
            "crates/foundation/src/hdr.rs",
            &[
                "precision_png16_decode_rgb_pix_fmt",
                "should_use_high_precision_png16_decode",
            ][..],
            false,
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must implement precision policy ({needle}, M67)"
            );
        }
        if scan_unwrap_or {
            let hits = offending_lines(&root, &[path], MC_FORBIDDEN_PRECISION_UNWRAP);
            assert!(
                hits.is_empty(),
                "{rel} must not use silent unwrap-or on precision paths (M67):\n{}",
                hits.join("\n")
            );
        }
    }

    for (rel, needle) in [
        (
            "crates/img/src/lossless_converter.rs",
            "precision_still_pipe_rgb_pix_fmt",
        ),
        ("crates/vid/src/conversion_api.rs", "hdr_pix_fmt"),
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            "hevc_yuv420_output_pix_fmt",
        ),
        (
            "crates/foundation/src/video_explorer/dynamic_mapping.rs",
            "explore_calibration_pix_fmt_optional",
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains(needle),
            "{rel} must route encode pix_fmt through precision policy ({needle}, M67)"
        );
        if rel == "crates/foundation/src/video_explorer/dynamic_mapping.rs" {
            let gate = fs::read_to_string(join_legacy_aware(
                &root,
                "crates/foundation/src/media_conversion_gate.rs",
            ))
            .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
            assert!(
                gate.contains("hevc_yuv420_output_pix_fmt"),
                "explore_calibration_pix_fmt_optional must delegate to hevc_yuv420_output_pix_fmt \
                 (M67/M72)"
            );
        }
        if rel == "crates/vid/src/conversion_api.rs" {
            assert!(
                prod.contains("foundation::hevc_yuv420_output_pix_fmt"),
                "{rel} hdr_pix_fmt must delegate to foundation::hevc_yuv420_output_pix_fmt (M67)"
            );
        }
        if rel != "crates/foundation/src/ffmpeg_builder.rs" {
            for pattern in MC_FORBIDDEN_PRECISION_PIX_FMT_LITERAL {
                assert!(
                    !prod.contains(pattern),
                    "{rel} must not hardcode pix_fmt literal {pattern} in production (M67)"
                );
            }
        }
    }

    let ffmpeg_builder = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/ffmpeg_builder.rs",
    ))
    .expect("ffmpeg_builder.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_fb = production_scope(&ffmpeg_builder);
    assert!(
        prod_fb.contains("PIX_FMT_RGB48LE"),
        "ffmpeg_builder must map Rgb48le through PIX_FMT_RGB48LE constant (M67)"
    );

    let media_precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_precision.rs",
    ))
    .expect("media_precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_mp = media_precision
        .split("#[cfg(test)]")
        .next()
        .unwrap_or(&media_precision);
    assert!(
        !prod_mp.contains(MC_FORBIDDEN_NA_STRING[0]) && !prod_mp.contains("map_or_else(|| \"N/A\""),
        "media_precision production must not synthesize N/A labels inline (M67)"
    );
}

#[test]
fn media_conversion_extended_defaults_m68() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "runtime_available_parallelism_or_default",
        "runtime_available_parallelism_capped_or_default",
        "probe_animated_promoted_frame_count_or_min_two",
        "delivery_env_enabled_unless_opt_out",
        "delivery_output_file_len_or_estimate",
        "ui_log_file_type_icon_prefix",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M68");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 68),
        "contract must document M68"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/thread_manager.rs",
            &["runtime_available_parallelism_or_default"][..],
            &["available_parallelism().map_or(4"][..],
        ),
        (
            "crates/foundation/src/cli_runner.rs",
            &["runtime_available_parallelism_or_default"][..],
            &["available_parallelism().map_or(4"][..],
        ),
        (
            "crates/foundation/src/video_explorer/ssim_calculator.rs",
            &["runtime_available_parallelism_capped_or_default"][..],
            &["available_parallelism().map_or(4"][..],
        ),
        (
            "crates/img/src/main.rs",
            &["runtime_available_parallelism_or_default"][..],
            &["available_parallelism().map_or(4"][..],
        ),
        (
            "crates/foundation/src/video_detection.rs",
            &["probe_animated_promoted_frame_count_or_min_two"][..],
            &["frames.filter(|c| *c > 1).unwrap_or(2)"][..],
        ),
        (
            "crates/foundation/src/training_progress.rs",
            &["delivery_env_enabled_unless_opt_out"][..],
            &["map_or(true, |v|"][..],
        ),
        (
            "crates/foundation/src/progress_mode.rs",
            &["ui_log_file_type_icon_prefix"][..],
            &[".unwrap_or_default()"][..],
        ),
        (
            "crates/vid/src/conversion_api.rs",
            &["delivery_output_file_len_or_estimate"][..],
            &["fs::metadata(&output_path).map_or(output_size"][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must use gate helper {needle} (M68)"
            );
        }
        for ban in forbidden {
            assert!(!prod.contains(ban), "{rel} must not use silent {ban} (M68)");
        }
    }
    let extended_hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M68_EXTENDED,
    );
    let extended_hits: Vec<String> = extended_hits
        .into_iter()
        .filter(|line| !line.contains("std::env::var(env_key).map_or(true, |raw|"))
        .collect();
    assert!(
        extended_hits.is_empty(),
        "M68 extended blind spots must be cleared:\n{}",
        extended_hits.join("\n")
    );
    let numeric_hits = delivery_numeric_forgery_offenders(&root);
    assert!(
        numeric_hits.is_empty(),
        "M68 requires zero unallowlisted numeric-forgery lines:\n{}",
        numeric_hits.join("\n")
    );
}

#[test]
fn media_conversion_substrate_defaults_m69() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ffi_probe_json_ptr_or_null",
        "jxl_previous_candidate_size_or_fallback",
        "loop_gif_logical_screen_optional",
        "loop_gif_logical_screen_or_zero",
        "loop_filename_or_empty_for_density",
        "algorithm_feature_distribution_required",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M69");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 69),
        "contract must document M69"
    );
    for (rel, needles, forbidden) in [
        (
            "crates/foundation/src/c_api.rs",
            &["probe_json_ptr"][..],
            &[".map_or(std::ptr::null_mut(), CString::into_raw)"][..],
        ),
        (
            "crates/foundation/src/jxl_explorer.rs",
            &["jxl_previous_candidate_size_or_fallback"][..],
            &[".map_or(fallback, |candidate| candidate.output_size)"][..],
        ),
        (
            "crates/foundation/src/loop_intent.rs",
            &[
                "loop_gif_logical_screen_optional",
                "loop_filename_or_empty_for_density",
            ][..],
            &["map_or((0, 0),", "file_name.as_deref().unwrap_or(\"\")"][..],
        ),
        (
            "crates/foundation/src/database.rs",
            &["algorithm_feature_distribution_required"][..],
            &[
                "algorithm_feature_distribution_or_fallback",
                ".get(key).map_or(fallback, DistributionStats::from)",
            ][..],
        ),
        (
            "crates/foundation/src/image_jpeg_analysis.rs",
            &[] as &[&str],
            &[".map_or(luma_estimate.quality, |chroma|"][..],
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{rel} must use gate helper {needle} (M69)"
            );
        }
        for ban in forbidden {
            assert!(!prod.contains(ban), "{rel} must not use silent {ban} (M69)");
        }
    }
    let substrate_hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M69_SUBSTRATE,
    );
    assert!(
        substrate_hits.is_empty(),
        "M69 substrate blind spots must be cleared:\n{}",
        substrate_hits.join("\n")
    );
}

#[test]
fn media_conversion_precision_metric_sealing_m70() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 70),
        "contract must document M70"
    );

    let precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precision.rs",
    ))
    .expect("precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in ["seal_vmaf_y", "seal_cambi", "seal_ms_ssim"] {
        assert!(
            precision.contains(sym),
            "precision.rs must define {sym} for M70 metric sealing"
        );
    }

    let ssim_calc = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/ssim_calculator.rs",
    ))
    .expect("ssim_calculator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ssim_calc.contains("precision::parse_explore_cambi_metric_token")
            || ssim_calc.contains("precision::seal_cambi"),
        "ssim_calculator must route CAMBI through precision seal/parse (M70/M79)"
    );
    assert!(
        ssim_calc.contains("precision::seal_ms_ssim"),
        "ssim_calculator must route MS-SSIM through precision::seal_ms_ssim (M70)"
    );
    assert!(
        ssim_calc.contains("precision::parse_explore_vmaf_y_metric_token")
            || ssim_calc.contains("precision::seal_vmaf_y"),
        "ssim_calculator must route VMAF-Y through precision seal/parse (M70/M77)"
    );
}

#[test]
fn media_conversion_precision_metric_sealing_m71() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 71),
        "contract must document M71"
    );

    let precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precision.rs",
    ))
    .expect("precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        precision.contains("seal_ms_ssim_yuv_bundle"),
        "precision.rs must define seal_ms_ssim_yuv_bundle for M71 YUV bundle sealing"
    );

    let ssim_calc = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/ssim_calculator.rs",
    ))
    .expect("ssim_calculator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ssim_calc.contains("precision::seal_ms_ssim_yuv_bundle"),
        "calculate_ms_ssim_yuv must seal YUV bundle via precision (M71)"
    );
    assert!(
        !ssim_calc.contains("y_ms_ssim.clamp(0.0, 1.0)"),
        "calculate_ms_ssim_yuv must not silently clamp MS-SSIM channels (M71)"
    );

    let vmaf_standalone = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/vmaf_standalone.rs",
    ))
    .expect("vmaf_standalone.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        vmaf_standalone.contains("precision::seal_ms_ssim"),
        "vmaf_standalone must seal MS-SSIM via precision (M71)"
    );
    assert!(
        !vmaf_standalone.contains("ms_ssim.clamp(0.0, 1.0)"),
        "vmaf_standalone must not silently clamp MS-SSIM (M71)"
    );
}

#[test]
fn media_conversion_precision_metric_sealing_m72() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 72),
        "contract must document M72"
    );

    let precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precision.rs",
    ))
    .expect("precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in ["seal_ssim", "seal_ssim_yuv_all_bundle", "seal_psnr"] {
        assert!(
            precision.contains(sym),
            "precision.rs must define {sym} for M72 explore metric sealing"
        );
    }

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("explore_calibration_pix_fmt_or_default"),
        "gate must export explore_calibration_pix_fmt_or_default (M72)"
    );

    let stream = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/stream_analysis.rs",
    ))
    .expect("stream_analysis.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        stream.contains("precision::seal_ssim_yuv_all_bundle"),
        "SSIM-All must seal via precision (M72)"
    );
    assert!(
        stream.contains("precision::seal_ssim"),
        "SSIM parsers must seal via precision (M72)"
    );

    let ssim_calc = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/ssim_calculator.rs",
    ))
    .expect("ssim_calculator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ssim_calc.contains("parse_explore_psnr_metric_token"),
        "PSNR parser must use central precision parser (M72/M73)"
    );

    let dynamic = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/dynamic_mapping.rs",
    ))
    .expect("dynamic_mapping.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        dynamic.contains("explore_calibration_pix_fmt_optional"),
        "dynamic_mapping CPU calibration must use gate pix_fmt helper (M72)"
    );

    let metric_hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M72_EXPLORE_METRICS,
    );
    assert!(
        metric_hits.is_empty(),
        "M72 explore metric blind spots must be cleared:\n{}",
        metric_hits.join("\n")
    );
}

#[test]
fn media_conversion_hardening_audit_snapshot() {
    let root = workspace_root();
    let audit_path = root.join("crates/dev/src/fixtures/media_conversion_deep_audit.json");
    assert!(
        audit_path.is_file(),
        "run: python3 crates/dev/scripts/media_conversion_delivery_heatmap.py --deep"
    );
    let audit: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&audit_path).expect("audit json readable")) // audited: contract test assertion path; panic/expect is test-only failure signal
            .expect("audit json must parse"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let unallowlisted = audit
        .pointer("/numeric_forgery_scan/unallowlisted")
        .and_then(serde_json::Value::as_u64)
        .expect("deep audit numeric_forgery_scan/unallowlisted should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_eq!(
        unallowlisted, 0,
        "deep audit must report 0 unallowlisted numeric forgery hits"
    );
    let allowlist_total = audit
        .pointer("/allowlist/total")
        .and_then(serde_json::Value::as_u64)
        .expect("deep audit allowlist/total should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_eq!(
        allowlist_total, 0,
        "M43 requires empty ALLOWLIST in deep audit snapshot"
    );
    let offenders = delivery_numeric_forgery_offenders(&root);
    assert!(
        offenders.is_empty(),
        "live M39 scan must match audit snapshot (0 offenders):\n{}",
        offenders.join("\n")
    );
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for milestone in [
        "M70", "M71", "M72", "M73", "M74", "M75", "M76", "M77", "M78",
    ] {
        let n: u32 = milestone
            .trim_start_matches('M')
            .parse()
            .expect("milestone id must be numeric"); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            contract_documents_milestone(&contract, n),
            "contract must document {milestone}"
        );
    }
}

#[test]
fn media_conversion_precision_metric_sealing_m75() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 75),
        "contract must document M75"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("explore_metric_parse_reject_audit"),
        "gate must export explore_metric_parse_reject_audit (M75)"
    );

    let precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precision.rs",
    ))
    .expect("precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        precision.contains("explore_metric_parse_reject_audit"),
        "precision PSNR parser must use strict-gated reject audit (M75)"
    );

    for rel in [
        "crates/foundation/src/video_explorer/ssim_calculator.rs",
        "crates/foundation/src/video_explorer/stream_analysis.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains("explore_metric_parse_reject_audit"),
            "{rel} must use strict-gated metric reject audits (M75)"
        );
        assert!(
            !prod.contains("is out of [0,1] finite domain"),
            "{rel} must not use pre-M75 always-on seal reject wording (M75)"
        );
    }
}

#[test]
fn media_conversion_gpu_coarse_fallback_audit_m77() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 77),
        "contract must document M77"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("explore_gpu_coarse_fallback_audit"),
        "gate must export explore_gpu_coarse_fallback_audit (M77)"
    );
    let best_crf = gate_fn_body(&gate, "explore_best_crf_or_backtrack_anchor");
    assert!(
        best_crf.contains("explore_gpu_coarse_fallback_audit"),
        "explore_best_crf_or_backtrack_anchor must use strict-gated fallback audit (M77)"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&gpu);
    assert!(
        prod.contains("explore_gpu_coarse_fallback_audit"),
        "search_anchor_crf must use explore_gpu_coarse_fallback_audit (M77)"
    );
    assert!(
        !prod.contains("map_or(baseline_crf,"),
        "gpu_coarse_search must not use silent map_or(baseline_crf) for anchor (M77)"
    );
    let ssim_calc = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/ssim_calculator.rs",
    ))
    .expect("ssim_calculator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&ssim_calc).contains("parse_explore_vmaf_y_metric_token"),
        "VMAF-Y parse must use central token helper (M77)"
    );
}

#[test]
fn media_conversion_stream_size_probe_audit_m78() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 78),
        "contract must document M78"
    );

    let stream = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/stream_size.rs",
    ))
    .expect("stream_size.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&stream);
    assert!(
        prod.contains("stream_size_probe_failure_audit"),
        "stream_size must route ffprobe failures through strict-gated audit (M78)"
    );
    assert!(
        !prod.contains("delivery_encode_path_audit"),
        "stream_size must not call delivery_encode_path_audit directly (M78)"
    );
}

#[test]
fn media_conversion_stream_size_duration_audit_m76() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 76),
        "contract must document M76"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let body = gate_fn_body(&gate, "stream_size_duration_fallback_audit");
    assert!(
        body.contains("delivery_encode_path_audit"),
        "stream_size duration fallback must delegate to encode path audit (M76/M98)"
    );
    assert!(
        !body.contains("strict_media_conversion_delivery_enabled"),
        "stream_size duration fallback must not double-gate strict (M98)"
    );

    let stream = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/stream_size.rs",
    ))
    .expect("stream_size.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&stream).contains("stream_size_duration_fallback_audit"),
        "stream_size must use duration fallback audit helper (M76)"
    );
}

#[test]
fn media_conversion_explore_ssim_policy_m79() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 79),
        "contract must document M79"
    );

    let precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precision.rs",
    ))
    .expect("precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        precision.contains("parse_explore_cambi_metric_token"),
        "precision.rs must define CAMBI central parser (M79)"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let degraded = gate_fn_body(&gate, "explore_ssim_metric_degraded_audit");
    assert!(
        degraded.contains("explore_precheck_degraded_audit"),
        "explore_ssim_metric_degraded_audit must delegate to precheck degraded audit (M79/M84)"
    );

    let ssim_path = join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/ssim_calculator.rs",
    );
    let ssim = fs::read_to_string(&ssim_path).expect("ssim_calculator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&ssim);
    assert!(
        prod.contains("parse_explore_cambi_metric_token"),
        "CAMBI JSON parse must use central token helper (M79)"
    );
    assert!(
        prod.contains("explore_ssim_metric_degraded_audit"),
        "ssim_calculator operational failures must use strict-gated audit (M79)"
    );

    let policy_hits = offending_lines(&root, &[ssim_path], MC_FORBIDDEN_M79_SSIM_CALC);
    assert!(
        policy_hits.is_empty(),
        "M79 ssim_calculator must not use always-on precheck audits or inline CAMBI seal:\n{}",
        policy_hits.join("\n")
    );
}

fn assert_media_conversion_contract_registry(
    contract: &str,
    tests: &str,
    min_m: u32,
    max_m: u32,
    min_distinct_dev_tests: usize,
) {
    for n in min_m..=max_m {
        assert!(
            contract_documents_milestone(contract, n),
            "contract table must include row M{n}"
        );
    }

    let mut seen_milestones = std::collections::BTreeSet::new();
    for line in contract.lines() {
        let Some(id) = line.strip_prefix("| M") else {
            continue;
        };
        let Some(num_str) = id.split('|').next().map(str::trim) else {
            continue;
        };
        match num_str.parse::<u32>() {
            Ok(n) if (min_m..=max_m).contains(&n) => {
                seen_milestones.insert(n);
            }
            Ok(_) => {}
            Err(_parse_err) => {}
        }
    }
    let expected = usize::try_from(max_m - min_m + 1).expect("milestone range must fit in usize"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_eq!(
        seen_milestones.len(),
        expected,
        "contract must define exactly M{min_m}–M{max_m} ({expected} rows, found \
         {seen_milestones:?})"
    );

    let mut required_dev_tests = std::collections::BTreeSet::new();
    for line in contract.lines() {
        if !line.starts_with("| M") {
            continue;
        }
        let Some(m_col) = line.split('|').nth(1) else {
            continue;
        };
        let m_id = m_col.trim();
        let Ok(n) = m_id.trim_start_matches('M').parse::<u32>() else {
            continue;
        };
        if !(min_m..=max_m).contains(&n) {
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find('`') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('`') else {
                break;
            };
            let token = &rest[..end];
            let rust_test = token == "production_code_has_no_numeric_forgery_fallbacks"
                || (token.starts_with("media_conversion_")
                    && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            if rust_test {
                required_dev_tests.insert(token.to_string());
            }
            rest = &rest[end + 1..];
        }
    }

    let mut missing = Vec::new();
    for name in &required_dev_tests {
        if !tests.contains(&format!("fn {name}(")) {
            missing.push(name.as_str());
        }
    }
    assert!(
        missing.is_empty(),
        "every dev test referenced in contract M{min_m}–M{max_m} must exist:\n{}",
        missing.join("\n")
    );
    assert!(
        required_dev_tests.len() >= min_distinct_dev_tests,
        "expected at least {min_distinct_dev_tests} distinct dev tests across M{min_m}–M{max_m}, \
         got {}",
        required_dev_tests.len()
    );
}

#[test]
fn media_conversion_contract_m1_m78_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 78, 55);
}

#[test]
fn media_conversion_contract_m1_m112_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 102),
        "contract must document M102"
    );
    assert!(
        contract_documents_milestone(&contract, 103),
        "contract must document M103"
    );
    assert!(
        contract_documents_milestone(&contract, 104),
        "contract must document M104"
    );
    assert!(
        contract_documents_milestone(&contract, 105),
        "contract must document M105"
    );
    assert!(
        contract_documents_milestone(&contract, 106),
        "contract must document M106"
    );
    assert!(
        contract_documents_milestone(&contract, 107),
        "contract must document M107"
    );
    assert!(
        contract_documents_milestone(&contract, 108),
        "contract must document M108"
    );
    assert!(
        contract_documents_milestone(&contract, 109),
        "contract must document M109"
    );
    assert!(
        contract_documents_milestone(&contract, 110),
        "contract must document M110"
    );
    assert!(
        contract_documents_milestone(&contract, 111),
        "contract must document M111"
    );
    assert!(
        contract_documents_milestone(&contract, 112),
        "contract must document M112"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 112, 80);
}

#[test]
fn media_conversion_contract_m1_m113_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 113),
        "contract must document M113"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 113, 81);
}

#[test]
fn media_conversion_batch_path_tree_m103() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 103),
        "contract must document M103"
    );

    let batch = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/batch.rs"))
        .expect("batch.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !batch.contains(".canonicalize().unwrap_or_else(|_| "),
        "batch path-tree must not silently canonicalize (M103)"
    );
    let gate_hits = batch.matches("canonicalize_for_tool_input").count();
    assert!(
        gate_hits >= 5,
        "batch path-tree must use canonicalize_for_tool_input at all cache roots (M103), got \
         {gate_hits}"
    );
}

#[test]
fn media_conversion_path_tree_cache_pg_m213() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 213),
        "contract must document M213"
    );

    let sql = fs::read_to_string(root.join("crates/dev/src/config/sql/analysis_cache_pg.sql"))
        .expect("analysis_cache_pg.sql must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        sql.contains("path_tree_snapshots"),
        "PG schema must define path_tree_snapshots (M213)"
    );

    let ptc = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/path_tree_cache.rs",
    ))
    .expect("path_tree_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ptc.contains("save_path_tree_snapshot") && ptc.contains("load_path_tree_snapshot"),
        "path_tree_cache must expose PG load/save (M213)"
    );

    let batch = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/batch.rs"))
        .expect("batch.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        batch.contains("path_tree_cache::save_path_tree_snapshot"),
        "batch must persist via path_tree_cache PG (M213)"
    );
    assert!(
        !batch.contains("serde_json::to_string_pretty(snapshot)"),
        "batch must not write path-tree JSON files (M213)"
    );
    assert!(
        !batch.contains("migrate_legacy_json_file") && !batch.contains("PATH_TREE_CACHE_DIR"),
        "batch must not retain legacy path-tree JSON migration (M213)"
    );

    let cargo = fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cargo.contains("rusqlite = { version = \"0.40"),
        "workspace rusqlite must be 0.40.x (M214)"
    );
}

#[test]
fn media_conversion_m214_sqlite_store_ssot() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 214),
        "contract must document M214"
    );

    let store = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/mfb_sqlite_store.rs",
    ))
    .expect("mfb_sqlite_store.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        store.contains("blob_store") && store.contains("payload_crc32"),
        "mfb_sqlite_store must use CRC-protected blob_store (M214)"
    );

    let checkpoint = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/checkpoint.rs",
    ))
    .expect("checkpoint.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        checkpoint.contains("save_progress_to_sqlite"),
        "checkpoint must persist via mfb_sqlite_store (M214)"
    );
    assert!(
        !checkpoint.contains("format!(\"{dir_hash}.txt\")")
            && !checkpoint.contains("legacy_entries"),
        "checkpoint must not use legacy .txt progress files (M214)"
    );
    assert!(
        !store.contains("unwrap_or(0)"),
        "mfb_sqlite_store must not silently forge row counts (M214)"
    );

    let ptc = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/path_tree_cache.rs",
    ))
    .expect("path_tree_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ptc.contains("save_sqlite_snapshot") && ptc.contains("load_sqlite_snapshot"),
        "path_tree must tier PG + SQLite (M214)"
    );
    assert!(
        ptc.contains("PATH_TREE_SCHEMA_VERSION"),
        "path_tree schema version must be SSOT in path_tree_cache (M214)"
    );
    assert!(
        store.contains("schema version mismatch"),
        "mfb_sqlite_store must reject schema mismatch (M214)"
    );

    let cargo = fs::read_to_string(root.join("Cargo.toml")).expect("Cargo.toml must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !cargo.contains("github.com/rusqlite/rusqlite"),
        "rusqlite git patch must not override crates.io 0.40 (M214)"
    );
}

#[test]
fn media_conversion_processed_list_m215() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 215),
        "contract must document M215"
    );

    let conversion = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/conversion.rs",
    ))
    .expect("conversion.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        conversion.contains("NS_PROCESSED")
            && conversion.contains("load_processed_list(session_key"),
        "processed list must use mfb_store blob_store (M215)"
    );
    assert!(
        !conversion.contains("flock_exclusive") && !conversion.contains("BufReader::new"),
        "processed list must not use line-based text files (M215)"
    );

    let store = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/mfb_sqlite_store.rs",
    ))
    .expect("mfb_sqlite_store.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        store.contains(r#"pub const NS_PROCESSED: &str = "processed""#),
        "mfb_sqlite_store must define NS_PROCESSED (M215)"
    );

    let cleaner = fs::read_to_string(root.join("crates/dev/src/bin/cache_cleaner.rs"))
        .expect("cache_cleaner.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cleaner.contains("remove_legacy_analysis_sqlite_files"),
        "cache_cleaner must drop legacy image_analysis_v2 DB files (M215)"
    );
    assert!(
        cleaner.contains("PG_INFERENCE_LOG_TABLES")
            && cleaner.contains("loop_intent_inference_log"),
        "cache_cleaner full purge must truncate inference-log cache tables"
    );
    assert!(
        cleaner.contains("PG_TRAINING_PROTECTED_TABLES") && cleaner.contains("loop_samples"),
        "cache_cleaner must protect PostgreSQL training sample tables from truncation"
    );
    assert!(
        !cleaner.contains("gif_value_samples_v2.db"),
        "cache_cleaner must not delete local gif_value training SQLite files"
    );
    assert!(
        !cleaner.contains("cargo clean"),
        "cache_cleaner full purge must not run cargo clean (cache-only scope)"
    );
    assert!(
        cleaner.contains("cache_metadata"),
        "cache_cleaner full purge must truncate PostgreSQL cache_metadata"
    );
    assert!(
        cleaner.contains("check_postgres_reachable") && cleaner.contains("PostgreSQL is required"),
        "cache_cleaner must require PostgreSQL for cache purge (no skip-on-unavailable)"
    );

    let drag = fs::read_to_string(root.join("crates/dev/src/bin/drag_and_drop_processor.rs"))
        .expect("drag_and_drop_processor.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        drag.contains("plan_cli_invocations")
            && drag.contains("rust_run_command")
            && drag.contains("build_fast_img_command")
            && drag.contains("build_fast_img_restore_command")
            && drag.contains("build_fast_vid_command"),
        "drag_and_drop must invoke Rust CLI/fastmode implementations directly"
    );
    assert!(
        !drag.contains("refresh_tools_for_processing") && !drag.contains("mfb_tool_refresh"),
        "drag_and_drop must not shell out to tool-refresh before processing"
    );
}

#[test]
fn media_conversion_quality_content_type_m104() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 104),
        "contract must document M104"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("quality_content_type_missing_audit"),
        "gate must export quality_content_type_missing_audit (M104)"
    );
    let body = gate_fn_body(&gate, "quality_content_type_missing_audit");
    assert!(
        body.contains("quality_heuristic_fallback_audit"),
        "quality_content_type_missing_audit must delegate to quality_heuristic_fallback_audit \
         (M104)"
    );

    let qm = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_matcher.rs",
    ))
    .expect("quality_matcher.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&qm);
    assert!(
        prod.contains("content_type_for_crf_analysis"),
        "quality_matcher production must use content_type_for_crf_analysis (M104)"
    );
    assert!(
        prod.contains("quality_content_type_for_crf_or_unknown"),
        "quality_matcher must route missing content_type through gate SSOT (M104/M182)"
    );
    assert!(
        !prod.contains("quality_content_type_unknown"),
        "quality_matcher must not inline quality_content_type_unknown branch string (M104)"
    );
    assert!(
        !prod.contains("content_type.unwrap_or_else"),
        "quality_matcher must not inline content_type unwrap_or_else (M182)"
    );
}

#[test]
fn media_conversion_path_canonicalize_m105() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 105),
        "contract must document M105"
    );

    for rel in [
        "crates/foundation/src/safety.rs",
        "crates/foundation/src/path_validator.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            !content.contains(".canonicalize().unwrap_or_else(|_| "),
            "{rel} must not silently canonicalize (M105)"
        );
        assert!(
            content.contains("canonicalize_for_tool_input"),
            "{rel} must use canonicalize_for_tool_input (M105)"
        );
    }
}

#[test]
fn media_conversion_loop_intent_numeric_ssot_m109() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 109),
        "contract must document M109"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "loop_bytes_per_frame_optional",
        "loop_bytes_per_frame_or_zero",
        "loop_total_pixels_optional",
        "loop_total_pixels_or_zero",
        "loop_audible_audio_fail_closed",
        "loop_fps_kinetic_weights_or_neutral",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M109)");
        let audit_sym = if sym.ends_with("_or_zero") {
            sym.replace("_or_zero", "_optional")
        } else {
            sym.to_string()
        };
        let body = gate_fn_body(&gate, &audit_sym);
        assert!(
            body.contains("delivery_intent_batch_audit"),
            "{sym} must audit via delivery_intent_batch_audit (M109)"
        );
    }

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&loop_intent);
    for needle in [
        "loop_bytes_per_frame_optional",
        "loop_total_pixels_optional",
        "loop_audible_audio_fail_closed",
        "loop_fps_kinetic_weights_or_neutral",
        "loop_duration_z_or_neutral",
        "loop_collection_duration_p90_or_baseline",
    ] {
        assert!(
            prod.contains(needle),
            "loop_intent production must use gate helper {needle} (M109/M219)"
        );
    }
    for ban in [
        "MSG_SIGNAL_VOID_AUDIO_SILENT",
        "meta.fps.map_or((0.0, 0.0)",
        "frame_count.map_or_else",
    ] {
        assert!(
            !prod.contains(ban),
            "loop_intent must not use silent fallback {ban} (M109)"
        );
    }
}

#[test]
fn media_conversion_loop_thresholds_ssot_m110() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 110),
        "contract must document M110"
    );

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&loop_intent);
    assert!(
        prod.contains("loop_duration_or_fallback_policy("),
        "loop_intent thresholds must use loop_duration_or_fallback_policy (M110/M218)"
    );
    for ban in [
        ".or(reference.duration.p10)\n            .unwrap_or_else(|| {",
        ".p75\n                    .map(|value| \
         value.min(crate::constants::LOOP_INTENT_MAX_DURATION))\n            })\n            \
         .unwrap_or_else(|| {",
    ] {
        assert!(
            !prod.contains(ban),
            "loop_intent thresholds must not inline silent fallback {ban} (M110)"
        );
    }
}

#[test]
fn media_conversion_loop_inference_ssot_m111() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 111),
        "contract must document M111"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "loop_scaled_duration_percentile_or_fallback",
        "loop_total_pixels_or_zero",
        "loop_duration_z_or_neutral",
        "loop_top_keywords_or_empty",
        "loop_frame_count_label_or_unknown",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M111)");
    }
    let scaled_policy_body =
        gate_fn_body(&gate, "loop_scaled_duration_percentile_or_fallback_policy");
    assert!(
        scaled_policy_body.contains("delivery_intent_batch_audit"),
        "loop_scaled_duration_percentile_or_fallback_policy must audit missing percentiles (M111)"
    );
    let depth_body = gate_fn_body(&gate, "loop_parent_directory_depth");
    assert!(
        depth_body.contains("delivery_intent_batch_audit"),
        "loop_parent_directory_depth must audit missing ancestry (M111)"
    );

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&loop_intent);
    for needle in [
        "loop_scaled_duration_percentile_or_fallback_policy",
        "loop_total_pixels_optional",
        "loop_duration_z_or_neutral",
        "loop_top_keywords_or_empty",
        "loop_frame_count_label_or_unknown",
    ] {
        assert!(
            prod.contains(needle),
            "loop_intent production must use gate helper {needle} (M111/M218)"
        );
    }
    for ban in [
        "duration.p50.map_or(short_percentile",
        "let total_pixels = match (meta.width, meta.height)",
        ".map_or(&[][..], |profile| profile.top_keywords",
        "map_or_else(|| \"unknown\".to_string(), |count| count.to_string())",
        "duration_secs.map_or_else(\n            || {\n                \
         crate::media_conversion_gate::loop_missing_duration_z_neutral",
    ] {
        assert!(
            !prod.contains(ban),
            "loop_intent must not use silent fallback {ban} (M111)"
        );
    }
}

#[test]
fn media_conversion_loop_diagnostic_ssot_m112() {
    const NEEDLES: &[&str] = &[
        "loop_format_optional_probability_or_na",
        "loop_format_duration_secs_label",
        "loop_neighbor_count_suffix_or_empty",
        "loop_layer_tag_from_reason_or_unknown",
    ];
    const BANS: &[&str] = &[
        "map_or_else(|| \"None\".to_string(), |d| format!(\"{d:.2}s\"))",
        "map_or_else(|| \"n/a\".to_string()",
        "map_or_else(String::new, |count| format!(\", n={count}\"))",
    ];
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 112),
        "contract must document M112"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "loop_format_optional_probability_or_na",
        "loop_format_duration_secs_label",
        "loop_neighbor_count_suffix_or_empty",
        "loop_layer_tag_from_reason_or_unknown",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M112)");
        let body = gate_fn_body(&gate, sym);
        assert!(
            body.contains("delivery_intent_batch_audit"),
            "{sym} must audit via delivery_intent_batch_audit (M112)"
        );
    }

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&loop_intent);
    for needle in NEEDLES {
        assert!(
            prod.contains(needle),
            "loop_intent production must use gate helper {needle} (M112)"
        );
    }
    for ban in BANS {
        assert!(
            !prod.contains(ban),
            "loop_intent must not use silent diagnostic fallback {ban} (M112)"
        );
    }
}

#[test]
fn media_conversion_progress_ssim_exit_suffix_m113() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 113),
        "contract must document M113"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ui_ssim_inline_or_empty",
        "ui_exit_code_suffix_or_empty",
        "explore_progress_ssim_token",
        "conversion_ssim_message_token",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M113)");
    }
    let ssim_body = gate_fn_body(&gate, "ui_ssim_inline_or_empty");
    assert!(
        ssim_body.contains("delivery_progress_batch_audit"),
        "ui_ssim_inline_or_empty must audit via delivery_progress_batch_audit (M113)"
    );
    let exit_body = gate_fn_body(&gate, "ui_exit_code_suffix_or_empty");
    assert!(
        exit_body.contains("delivery_runtime_batch_audit"),
        "ui_exit_code_suffix_or_empty must audit via delivery_runtime_batch_audit (M113)"
    );
    let explore_token = gate_fn_body(&gate, "explore_progress_ssim_token");
    assert!(
        explore_token.contains("ui_ssim_inline_or_empty"),
        "explore_progress_ssim_token must delegate to ui_ssim_inline_or_empty (M113)"
    );

    let progress = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/progress.rs",
    ))
    .expect("progress.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let progress_prod = production_scope(&progress);
    assert!(
        !progress_prod.contains("ssim.map_or_else(String::new"),
        "progress.rs must not silently omit SSIM segment (M113)"
    );
    assert!(
        progress_prod.contains("explore_progress_ssim_token")
            || progress_prod.contains("explore_progress_ssim_token_pending"),
        "progress.rs must use gated explore SSIM token for iterations (M113+)"
    );
    assert!(
        progress_prod.contains("explore_progress_ssim_token"),
        "progress.rs finish path must audit missing final SSIM via explore_progress_ssim_token \
         (M113)"
    );

    for (rel, needle, ban) in [
        (
            "crates/foundation/src/unified_error.rs",
            "ui_exit_code_suffix_or_empty",
            "exit_code.map_or_else(String::new",
        ),
        (
            "crates/foundation/src/app_error.rs",
            "ui_exit_code_suffix_or_empty",
            "exit_code.map_or_else(String::new",
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(prod.contains(needle), "{rel} must use {needle} (M113)");
        assert!(
            !prod.contains(ban),
            "{rel} must not use silent exit-code suffix fallback (M113)"
        );
    }
}

#[test]
fn media_conversion_animated_timing_m114() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 114),
        "contract must document M114"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_progress_ssim_token_pending",
        "ui_ssim_inline_when_unmeasured",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M114)");
    }

    let image_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&image_detection);
    assert!(
        prod.contains("image_formats::gif::timing_stats_from_bytes"),
        "GIF detect_animation must derive fps from GCE delays (M114)"
    );
    assert!(
        prod.contains("image_formats::webp::timing_stats_from_bytes"),
        "WebP detect_animation must derive fps from ANMF delays (M114)"
    );

    let image_formats = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_formats.rs",
    ))
    .expect("image_formats.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        image_formats.contains("pub struct WebpTimingStats"),
        "image_formats must export WebpTimingStats (M114)"
    );
}

#[test]
fn media_conversion_contract_m1_m114_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 114),
        "contract must document M114"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 114, 82);
}

#[test]
fn media_conversion_apng_timing_m115() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 115),
        "contract must document M115"
    );

    let image_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&image_detection);
    assert!(
        prod.contains("apng_timing_stats_from_bytes"),
        "image_detection must export apng_timing_stats_from_bytes (M115)"
    );
    assert!(
        prod.contains("apng_frame_delay_secs"),
        "APNG delay must use fcTL delay_num/delay_den (M115)"
    );

    let analyzer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let analyzer_prod = production_scope(&analyzer);
    assert!(
        analyzer_prod.contains("apng_timing_stats_from_bytes"),
        "get_animation_duration must use native APNG timing (M115)"
    );
    assert!(
        analyzer_prod.contains("webp::timing_stats_from_bytes"),
        "get_animation_duration must use native WebP timing (M115)"
    );
}

#[test]
fn media_conversion_contract_m1_m115_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 115),
        "contract must document M115"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 115, 83);
}

#[test]
fn media_conversion_probe_duration_ladder_m116() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 116),
        "contract must document M116"
    );

    let ffprobe = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
        .expect("ffprobe.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let ffprobe_prod = production_scope(&ffprobe);
    assert!(
        ffprobe_prod.contains("probe_duration_from_frame_count_and_fps"),
        "ffprobe must derive duration from nb_frames/fps (M116)"
    );
    assert!(
        ffprobe_prod.contains("gif::timing_stats_from_bytes"),
        "resolve_probe_duration must use native GIF timing (M116)"
    );
    assert!(
        ffprobe_prod.contains("apng_timing_stats_from_bytes"),
        "resolve_probe_duration must use native APNG timing (M116)"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let gpu_prod = production_scope(&gpu);
    assert!(
        !gpu_prod.contains("SSIM N/A"),
        "gpu_coarse_search must not emit SSIM N/A placeholders (M116)"
    );
}

#[test]
fn media_conversion_contract_m1_m116_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 116),
        "contract must document M116"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 116, 84);
}

#[test]
fn media_conversion_loop_inference_telemetry_m117() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 117),
        "contract must document M117"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "json_inference_optional_f64_or_null",
        "loop_duration_or_fallback_policy",
        "loop_scaled_duration_percentile_or_fallback_policy",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M117)");
    }

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        loop_intent.contains("loop_duration_or_fallback_policy"),
        "LoopThresholds must use policy-aware duration fallback (M117)"
    );
    assert!(
        loop_intent.contains("duration_percentiles_available"),
        "LoopThresholds must gate audits on profile percentile availability (M117)"
    );

    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("json_inference_optional_f64_or_null"),
        "build_signal_snapshot must use policy-silent JSON helpers (M117)"
    );
    let db_prod = production_scope(&database);
    for field in [
        "knn_lookup_succeeded",
        "knn_telemetry_lookup_succeeded",
        "layer6b_resolved",
        "resolution_path",
        "hdbscan_cluster_id",
    ] {
        assert!(
            db_prod.contains(field),
            "build_signal_snapshot must include {field} (M117)"
        );
    }
    assert!(
        !db_prod.contains(
            "\"layer6b_resolved\": crate::media_conversion_gate::json_optional_bool_or_null"
        ),
        "layer6b_resolved must not use strict-audit json_optional_bool_or_null (M216)"
    );
}

#[test]
fn media_conversion_loop_profile_percentiles_m118() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 118),
        "contract must document M118"
    );

    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("duration_has_empirical_percentiles"),
        "LoopReferenceProfile must track empirical duration histogram provenance (M118)"
    );
    assert!(
        loop_intent.contains("reference.duration_has_empirical_percentiles"),
        "LoopThresholds must gate on empirical duration percentiles, not inferred slots (M118)"
    );
    let prod_db = production_scope(&database);
    assert!(
        !prod_db.contains("fill_missing_percentiles_from_moments()"),
        "M118 production path must not call fill_missing_percentiles_from_moments"
    );
    assert!(
        !prod_db.contains("merge_duration_distribution_from_collection"),
        "M118 production path must not fabricate duration percentiles from collection"
    );
}

#[test]
fn media_conversion_contract_m1_m117_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 117),
        "contract must document M117"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 117, 85);
}

#[test]
fn media_conversion_image_quality_ingest_json_m119() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 119),
        "contract must document M119"
    );

    let image_quality = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_quality_db.rs",
    ))
    .expect("image_quality_db.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !image_quality.contains("fn json_finite_f64"),
        "image_quality_db must not duplicate local json_finite helper (M119)"
    );
    assert!(
        image_quality.contains("json_inference_optional_f64_or_null"),
        "build_image_quality_ingest_metadata must use policy-silent JSON helper (M119)"
    );
    let prod = production_scope(&image_quality);
    assert!(
        prod.contains("build_image_quality_ingest_metadata"),
        "M119 scope must cover ingest metadata builder"
    );
}

#[test]
fn media_conversion_contract_m1_m118_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 118),
        "contract must document M118"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 118, 87);
}

#[test]
fn media_conversion_checkpoint_canonicalize_m120() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 120),
        "contract must document M120"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("canonicalize_for_checkpoint_path"),
        "gate must export canonicalize_for_checkpoint_path (M120)"
    );
    assert!(
        gate.contains("canonicalize_path_or_preserve"),
        "tool and checkpoint canonicalize must share core helper (M120)"
    );

    let checkpoint = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/checkpoint.rs",
    ))
    .expect("checkpoint.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        checkpoint.contains("canonicalize_for_checkpoint_path"),
        "checkpoint must use gate canonicalize (M120)"
    );
    assert!(
        !checkpoint.contains("Falling back to cwd join"),
        "checkpoint must not cwd-join on canonicalize failure (M120)"
    );
    assert!(
        !checkpoint.contains("path.canonicalize()"),
        "checkpoint must not call path.canonicalize directly (M120)"
    );
}

#[test]
fn media_conversion_contract_m1_m119_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 119),
        "contract must document M119"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 119, 88);
}

#[test]
fn media_conversion_loop_collection_duration_m121() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 121),
        "contract must document M121"
    );

    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("duration_p90_empirical"),
        "collection stats must compute duration_p90 from samples (M121)"
    );
    let prod_db = production_scope(&database);
    assert!(
        !prod_db.contains("merge_duration_distribution_from_collection"),
        "M121 must not forge collection aggregates into duration percentiles"
    );
    assert!(
        prod_db.contains("build_loop_collection_stats"),
        "M121 must expose collection duration stats separately from profile percentiles"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gpu.contains("quality_passed_check")
            && gpu.contains("Phase 3 verifier")
            && gpu.contains("CheckResult::NotChecked"),
        "ultimate build_result must defer quality_passed until Phase 3 (M121)"
    );
    let build_result_body = gpu
        .split("fn build_result")
        .nth(1)
        .and_then(|rest| rest.split("fn search_anchor_crf").next())
        .expect("gpu build_result body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !build_result_body.contains(".sealed()"),
        "build_result must not seal before ExploreQualityVerifier (M121)"
    );
    assert!(
        build_result_body.contains("lossless_integrity_ok"),
        "M121 build_result must use integrity gate for lossless GIF (no fabricated SSIM)"
    );
    assert!(
        !build_result_body
            .contains("Some(1.0_f64)\n        } else {\n            calculate_ssim_enhanced"),
        "M121 build_result must not fabricate SSIM=1.0 for lossless integrity path"
    );
}

#[test]
fn media_conversion_contract_m1_m120_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 120),
        "contract must document M120"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 120, 89);
}

#[test]
fn media_conversion_probe_dimension_recovery_m122() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 122),
        "contract must document M122"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_bitstream_dimension_recovery_audit",
        "loop_collection_secs_or_baseline_policy",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M122)");
    }

    let ffprobe = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
        .expect("ffprobe.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ffprobe.contains("probe_bitstream_dimension_recovery_audit"),
        "ffprobe must audit bitstream dimension recovery (M122)"
    );

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        loop_intent.contains("loop_collection_duration_p90_or_baseline")
            || loop_intent.contains("loop_collection_secs_or_baseline_policy"),
        "LoopThresholds must use policy-aware collection duration P90 (M122/M218)"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        video_detection.contains("probe_bitstream_dimension_recovery_audit"),
        "WebP header backfill must audit dimension recovery (M122)"
    );
}

#[test]
fn media_conversion_contract_m1_m121_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 121),
        "contract must document M121"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 121, 90);
}

#[test]
fn media_conversion_contract_m1_m122_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 122),
        "contract must document M122"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 122, 91);
}

#[test]
fn media_conversion_webp_header_preflight_m123() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 123),
        "contract must document M123"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "try_probe_from_animated_webp_header",
        "webp_animated_header_preflight",
    ] {
        assert!(
            video_detection.contains(sym),
            "video_detection must implement {sym} (M123)"
        );
    }
    assert!(
        video_detection.contains("\"animated_container_ffprobe_recovery\"")
            && video_detection.contains("probe_layer_audit"),
        "animated promote recovery must be path-scoped (M123)"
    );

    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/tests/video_detection.rs",
    ))
    .expect("video_detection tests must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("detect_video_animated_webp_header_preflight_m123"),
        "unit test must cover animated WebP preflight (M123)"
    );
}

#[test]
fn media_conversion_contract_m1_m123_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 123),
        "contract must document M123"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 123, 92);
}

#[test]
fn media_conversion_gif_header_preflight_m124() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 124),
        "contract must document M124"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "try_probe_from_animated_gif_header",
        "gif_animated_header_preflight",
    ] {
        assert!(
            video_detection.contains(sym),
            "video_detection must implement {sym} (M124)"
        );
    }

    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/tests/video_detection.rs",
    ))
    .expect("video_detection tests must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("detect_video_animated_gif_header_preflight_m124"),
        "unit test must cover animated GIF preflight (M124)"
    );
}

#[test]
fn media_conversion_contract_m1_m124_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 124),
        "contract must document M124"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 124, 93);
}

#[test]
fn media_conversion_apng_header_preflight_m125() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 125),
        "contract must document M125"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "try_probe_from_animated_apng_header",
        "apng_header_preflight",
        "png_ihdr_dimensions_from_bytes",
    ] {
        assert!(
            video_detection.contains(sym),
            "video_detection must implement {sym} (M125)"
        );
    }

    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/tests/video_detection.rs",
    ))
    .expect("video_detection tests must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("detect_video_animated_apng_header_preflight_m125"),
        "unit test must cover APNG preflight (M125)"
    );
}

#[test]
fn media_conversion_contract_m1_m125_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 125),
        "contract must document M125"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 125, 94);
}

#[test]
fn media_conversion_ffprobe_native_frame_override_m126() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 126),
        "contract must document M126"
    );

    let ffprobe = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
        .expect("ffprobe.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ffprobe
            .contains("Root fix: ffprobe often under-reports `nb_frames` for animated GIF (M126)")
            && ffprobe.contains("Root fix: APNG via `png_pipe`"),
        "ffprobe must override GIF/APNG nb_frames from native parsers (M126)"
    );

    let ffprobe_tests =
        fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
            .expect("ffprobe.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ffprobe_tests.contains("parse_video_stream_fields_gif_native_frame_override_m126")
            && ffprobe_tests.contains("parse_video_stream_fields_apng_native_frame_override_m126"),
        "ffprobe unit tests must cover M126"
    );
}

#[test]
fn media_conversion_contract_m1_m126_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 126),
        "contract must document M126"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 126, 95);
}

#[test]
fn media_conversion_detection_bitstream_repair_m127() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 127),
        "contract must document M127"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "repair_animated_container_detection_from_bitstream_header",
        "backfill_detection_canvas_from_bitstream_header",
        "animated_frame_count_bitstream_recovery",
    ] {
        assert!(
            video_detection.contains(sym),
            "video_detection must export {sym} (M127)"
        );
    }
    assert!(
        video_detection.contains(
            "repair_animated_container_detection_from_bitstream_header(path, &mut result)"
        ),
        "detect_video_impl must run post-ffprobe repair (M127)"
    );

    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/tests/video_detection.rs",
    ))
    .expect("video_detection tests must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("repair_detection_canvas_and_frames_m127"),
        "unit test must cover bitstream repair (M127)"
    );
}

#[test]
fn media_conversion_contract_m1_m127_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 127),
        "contract must document M127"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 127, 96);
}

#[test]
fn media_conversion_video_cache_bitstream_repair_m128() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 128),
        "contract must document M128"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "cached_detection_needs_bitstream_repair",
        "video_cache_bitstream_repair",
    ] {
        assert!(
            video_detection.contains(sym),
            "video_detection must implement {sym} (M128)"
        );
    }
    assert!(
        video_detection.contains(
            "repair_animated_container_detection_from_bitstream_header(path, detection);"
        ) && video_detection.contains("pub fn promote_animated_container_for_vid"),
        "promote must delegate to M127 repair first (M128)"
    );

    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/tests/video_detection.rs",
    ))
    .expect("video_detection tests must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("cached_detection_bitstream_repair_m128"),
        "unit test must cover cache bitstream repair (M128)"
    );
}

#[test]
fn media_conversion_contract_m1_m128_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 128),
        "contract must document M128"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 128, 97);
}

#[test]
fn media_conversion_video_cache_revalidate_m129() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 129),
        "contract must document M129"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        video_detection.contains("video_cache_repair_incomplete"),
        "cache path must audit incomplete repair and re-run detection (M129)"
    );
    assert!(
        video_detection.contains("if should_refresh_cached_result(&cached)")
            && video_detection.contains("Cache bitstream repair incomplete"),
        "detect_video_with_cache must revalidate after repair (M129)"
    );
}

#[test]
fn media_conversion_contract_m1_m129_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 129),
        "contract must document M129"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 129, 98);
}

#[test]
fn media_conversion_analysis_cache_positive_policy_m130() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 130),
        "contract must document M130"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "video_analysis_canvas_trustworthy",
        "purge_negative_video_cache",
        "analysis_cache_negative_video_rejected",
    ] {
        assert!(
            cache.contains(sym),
            "analysis_cache must implement {sym} (M130)"
        );
    }
    assert!(
        cache.contains("DELETE FROM path_index WHERE file_path = $1"),
        "negative video cache purge must drop path_index rows (M130)"
    );

    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("multi_frame_zero_canvas_video_analysis_is_not_cacheable_m130"),
        "unit tests must cover zero-canvas rejection (M130)"
    );
}

#[test]
fn media_conversion_contract_m1_m130_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 130),
        "contract must document M130"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 130, 99);
}

#[test]
fn media_conversion_analysis_cache_image_policy_m131() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 131),
        "contract must document M131"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "image_analysis_canvas_trustworthy",
        "purge_negative_image_cache",
        "analysis_cache_negative_image_rejected",
    ] {
        assert!(
            cache.contains(sym),
            "analysis_cache must export {sym} (M131)"
        );
    }
    assert!(
        cache.contains("purge_negative_image_cache"),
        "image negative cache must purge analysis_records + path_index (M131)"
    );
}

#[test]
fn media_conversion_analysis_cache_video_algorithm_m132() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 132),
        "contract must document M132"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.contains("cache_record_algorithm_current"),
        "cache hits must gate algorithm_version (M132)"
    );
    assert!(
        cache.contains("r.algorithm_version, p.ctime, p.btime, p.content_hash")
            && cache.contains("algorithm_version FROM video_records"),
        "video path/hash queries must read algorithm_version (M132)"
    );
}

#[test]
fn media_conversion_contract_m1_m132_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 132),
        "contract must document M132"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 132, 101);
}

#[test]
fn media_conversion_analysis_cache_quality_policy_m133() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 133),
        "contract must document M133"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "quality_analysis_is_positive_cache_entry",
        "purge_negative_quality_cache",
        "analysis_cache_negative_quality_rejected",
        "cache_record_algorithm_current",
    ] {
        assert!(
            cache.contains(sym),
            "analysis_cache must export {sym} (M133)"
        );
    }
    assert!(
        cache.contains("algorithm_version FROM quality_records"),
        "quality cache must gate algorithm_version on hash hit (M133)"
    );
}

#[test]
fn media_conversion_contract_m1_m133_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 133),
        "contract must document M133"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 133, 102);
}

#[test]
fn media_conversion_analysis_cache_image_algorithm_m134() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 134),
        "contract must document M134"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.contains(
            "cache_record_algorithm_current(algorithm_version, path, \"image-path-hit\")"
        ),
        "image path hits must gate algorithm_version (M134)"
    );
    assert!(
        cache.contains(
            "cache_record_algorithm_current(algorithm_version, path, \"image-hash-hit\")"
        ),
        "image hash hits must gate algorithm_version (M134)"
    );
    assert!(
        cache.contains("purge_negative_image_cache"),
        "stale image cache must purge analysis_records + path_index (M134)"
    );
}

#[test]
fn media_conversion_contract_m1_m134_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 134),
        "contract must document M134"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 134, 103);
}

#[test]
fn media_conversion_analysis_cache_checksum_purge_m135() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 135),
        "contract must document M135"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.contains("fn purge_corrupt_cache_record"),
        "checksum failures must purge corrupt rows (M135)"
    );
    for table in ["analysis_records", "quality_records", "video_records"] {
        assert!(
            cache.contains(&format!("\"{table}\"")),
            "checksum purge must reference {table} (M135)"
        );
    }
    assert!(
        cache.matches("purge_corrupt_cache_record").count() >= 12,
        "image/quality/video path+hash checksum paths must purge (M135)"
    );
}

#[test]
fn media_conversion_analysis_cache_age_prune_m136() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 136),
        "contract must document M136"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.contains("fn purge_orphan_path_index_entries"),
        "orphan path_index purge must be shared SSOT (M136)"
    );
    let cleanup = cache
        .split("pub fn cleanup_old_records")
        .nth(1)
        .and_then(|s| s.split("pub fn get_statistics").next())
        .expect("cleanup_old_records body"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for table in ["analysis_records", "quality_records", "video_records"] {
        assert!(
            cleanup.contains(table),
            "cleanup_old_records must prune {table} (M136)"
        );
    }
    assert!(
        cleanup.contains("purge_orphan_path_index_entries"),
        "cleanup must purge orphan path_index after age prune (M136)"
    );
}

#[test]
fn media_conversion_contract_m1_m136_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 136),
        "contract must document M136"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 136, 105);
}

#[test]
fn media_conversion_analysis_cache_payload_decode_m137() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 137),
        "contract must document M137"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.contains("fn unpack_cached_payload"),
        "cache hits must use unpack_cached_payload (M137)"
    );
    assert!(
        cache.contains("analysis_cache_payload_decode_failed"),
        "decode failures must audit analysis_cache_payload_decode_failed (M137)"
    );
    assert!(
        cache.matches("unpack_cached_payload").count() >= 6,
        "image/quality/video path+hash must decode via unpack_cached_payload (M137)"
    );
}

#[test]
fn media_conversion_analysis_cache_fingerprint_gate_m138() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 138),
        "contract must document M138"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "reject_cache_hit_on_content_fingerprint_mismatch",
        "stored_content_fingerprint_matches_path",
        "analysis_cache_content_fingerprint_mismatch",
    ] {
        assert!(
            cache.contains(sym),
            "analysis_cache must export {sym} (M138)"
        );
    }
    assert!(
        cache.contains("content_fingerprint_hash"),
        "cache SELECT must read content_fingerprint_hash on hits (M138)"
    );
    assert!(
        cache
            .matches("reject_cache_hit_on_content_fingerprint_mismatch")
            .count()
            >= 6,
        "fingerprint gate must cover image/quality/video path+hash (M138)"
    );
}

#[test]
fn media_conversion_contract_m1_m138_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 138),
        "contract must document M138"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 138, 107);
}

#[test]
fn media_conversion_image_analyzer_cache_audit_m139() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 139),
        "contract must document M139"
    );

    let analyzer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        analyzer.contains("analyzer_cache_load_failed"),
        "image analyzer must audit cache load failures (M139)"
    );
    assert!(
        analyzer.contains("analyzer_cache_store_failed"),
        "image analyzer must audit cache store failures (M139)"
    );
    let load_block = analyzer
        .split("analyzer_cache_load_failed")
        .nth(1)
        .and_then(|s| s.get(..400))
        .expect("analyzer cache load audit block should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !load_block.contains("ENV_DEBUG"),
        "analyzer_cache_load_failed must not be ENV_DEBUG-gated (M139)"
    );
}

#[test]
fn media_conversion_analysis_cache_algorithm_purge_audit_m140() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 140),
        "contract must document M140"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.contains("analysis_cache_algorithm_upgrade_purge"),
        "algorithm upgrade purge must batch-audit (M140)"
    );
    let invalidate = cache
        .split("fn invalidate_old_algorithm_entries")
        .nth(1)
        .and_then(|s| s.split("fn purge_orphan_path_index_entries").next())
        .expect("invalidate_old_algorithm_entries body"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        invalidate.contains("purge_orphan_path_index_entries"),
        "algorithm upgrade must purge orphan path_index (M140)"
    );
}

#[test]
fn media_conversion_contract_m1_m140_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 140),
        "contract must document M140"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 140, 109);
}

#[test]
fn media_conversion_vid_cache_startup_prune_m141() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 141),
        "contract must document M141"
    );

    let vid_main = fs::read_to_string(root.join("crates/vid/src/main.rs"))
        .expect("vid main.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        vid_main.contains("cleanup_old_records(foundation::constants::CACHE_PRUNE_AGE_SECS)"),
        "vid must age-prune cache on startup (M141)"
    );
    assert!(
        vid_main.contains("analysis_cache_age_prune_failed"),
        "vid prune failures must audit analysis_cache_age_prune_failed (M141)"
    );
}

#[test]
fn media_conversion_cli_cache_lifecycle_audit_m142() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 142),
        "contract must document M142"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("fn analysis_cache_lifecycle_batch_audit"),
        "gate must export analysis_cache_lifecycle_batch_audit (M142)"
    );

    let img_main = fs::read_to_string(root.join("crates/img/src/main.rs"))
        .expect("img main.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let vid_main = fs::read_to_string(root.join("crates/vid/src/main.rs"))
        .expect("vid main.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for (name, src) in [("img", img_main.as_str()), ("vid", vid_main.as_str())] {
        assert!(
            src.contains("analysis_cache_lifecycle_batch_audit"),
            "{name} must use analysis_cache_lifecycle_batch_audit (M142)"
        );
        assert!(
            !src.contains("delivery_jxl_batch_fallback_audit(\n                \"analysis_cache"),
            "{name} must not route cache lifecycle via delivery_jxl_* (M142)"
        );
    }
    assert!(
        img_main.contains("analysis_cache_age_prune_failed"),
        "img must use analysis_cache_age_prune_failed branch (M142)"
    );
}

#[test]
fn media_conversion_contract_m1_m142_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 142),
        "contract must document M142"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 142, 111);
}

#[test]
fn media_conversion_vid_cache_store_audit_m143() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 143),
        "contract must document M143"
    );

    let conversion = fs::read_to_string(root.join("crates/vid/src/conversion_api.rs"))
        .expect("conversion_api.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !conversion.contains("video_cache_update"),
        "conversion_api must not use legacy video_cache_update branch (M143)"
    );
    assert!(
        conversion.matches("video_cache_store_failed").count() >= 2,
        "CRF/GIF hint store failures must audit video_cache_store_failed (M143)"
    );
    assert!(
        conversion.contains("video_cache_store_failed_audit"),
        "video cache store must use video_cache_store_failed_audit (M143/M145)"
    );
}

#[test]
fn media_conversion_cli_cache_prune_completed_m144() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 144),
        "contract must document M144"
    );

    let img_main = fs::read_to_string(root.join("crates/img/src/main.rs"))
        .expect("img main.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let vid_main = fs::read_to_string(root.join("crates/vid/src/main.rs"))
        .expect("vid main.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for (name, src) in [("img", img_main.as_str()), ("vid", vid_main.as_str())] {
        assert!(
            src.contains("analysis_cache_age_prune_completed"),
            "{name} must audit successful age prune (M144)"
        );
        assert!(
            src.contains("removed="),
            "{name} prune-completed audit must include removed count (M144)"
        );
    }
}

#[test]
fn media_conversion_contract_m1_m144_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 144),
        "contract must document M144"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 144, 113);
}

#[test]
fn media_conversion_video_cache_store_audit_ssot_m145() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 145),
        "contract must document M145"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("fn video_cache_store_failed_audit"),
        "gate must export video_cache_store_failed_audit (M145)"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let conversion = fs::read_to_string(root.join("crates/vid/src/conversion_api.rs"))
        .expect("conversion_api.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for (name, src) in [
        ("video_detection", video_detection.as_str()),
        ("conversion_api", conversion.as_str()),
    ] {
        assert!(
            src.contains("video_cache_store_failed_audit"),
            "{name} must use video_cache_store_failed_audit (M145)"
        );
        assert!(
            !src.contains("probe_layer_audit(\n                \"video_cache_store_failed\""),
            "{name} must not inline video_cache_store_failed probe (M145)"
        );
    }
}

#[test]
fn media_conversion_analysis_cache_path_index_stale_m146() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 146),
        "contract must document M146"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "reject_stale_path_index_hit",
        "path_index_content_hash_matches_file",
        "analysis_cache_path_index_stale",
    ] {
        assert!(
            cache.contains(sym),
            "analysis_cache must export {sym} (M146)"
        );
    }
    assert!(
        cache.matches("reject_stale_path_index_hit").count() >= 3,
        "image/quality/video path hits must guard path_index staleness (M146)"
    );
}

#[test]
fn media_conversion_contract_m1_m146_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 146),
        "contract must document M146"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 146, 115);
}

#[test]
fn media_conversion_image_cache_io_audit_ssot_m147() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 147),
        "contract must document M147"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn analyzer_cache_load_failed_audit",
        "fn analyzer_cache_store_failed_audit",
        "fn image_quality_cache_load_failed_audit",
        "fn image_quality_cache_store_failed_audit",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M147)");
    }

    let analyzer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        analyzer.contains("analyzer_cache_load_failed_audit"),
        "image_analyzer must use analyzer_cache_load_failed_audit (M147)"
    );
    assert!(
        analyzer.contains("analyzer_cache_store_failed_audit"),
        "image_analyzer must use analyzer_cache_store_failed_audit (M147)"
    );
    assert!(
        !analyzer.contains("probe_audit!(\n                    \"analyzer_cache_load_failed\""),
        "image_analyzer must not inline analyzer cache load audit (M147)"
    );

    let quality = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_quality_detector.rs",
    ))
    .expect("image_quality_detector.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        quality.contains("image_quality_cache_load_failed_audit"),
        "image_quality must use image_quality_cache_load_failed_audit (M147)"
    );
    assert!(
        quality.contains("image_quality_cache_store_failed_audit"),
        "image_quality must use image_quality_cache_store_failed_audit (M147)"
    );
    assert!(
        !quality.contains(
            "probe_quality_layer_audit(\n                    \"image_quality_cache_load_failed\""
        ),
        "image_quality must not inline quality cache load audit (M147)"
    );
}

#[test]
fn media_conversion_contract_m1_m147_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 147),
        "contract must document M147"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 147, 116);
}

#[test]
fn media_conversion_delivery_cache_io_audit_matrix_m148() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 148),
        "contract must document M148"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "fn video_cache_load_failed_audit",
        "fn video_cache_store_failed_audit",
        "fn analyzer_cache_load_failed_audit",
        "fn analyzer_cache_store_failed_audit",
        "fn image_quality_cache_load_failed_audit",
        "fn image_quality_cache_store_failed_audit",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M148)");
    }

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        video_detection.contains("video_cache_load_failed_audit"),
        "video_detection must use video_cache_load_failed_audit (M148)"
    );
    assert!(
        video_detection.contains("video_cache_store_failed_audit"),
        "video_detection must use video_cache_store_failed_audit (M148)"
    );

    let conversion = fs::read_to_string(root.join("crates/vid/src/conversion_api.rs"))
        .expect("conversion_api.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        conversion.contains("video_cache_store_failed_audit"),
        "conversion_api must use video_cache_store_failed_audit (M148)"
    );
}

#[test]
fn media_conversion_contract_m1_m148_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 148),
        "contract must document M148"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 148, 117);
}

#[test]
fn media_conversion_analysis_cache_hash_file_size_m149() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 149),
        "contract must document M149"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.contains("reject_cache_hit_on_record_file_size_mismatch"),
        "hash hits must validate stored file_size (M149)"
    );
    assert!(
        cache.contains("analysis_cache_record_file_size_mismatch"),
        "file_size mismatch must audit (M149)"
    );
    for table in ["analysis_records", "quality_records", "video_records"] {
        assert!(
            cache.contains(&format!("file_size FROM {table} WHERE content_hash")),
            "hash query must read file_size from {table} (M149)"
        );
    }
}

#[test]
fn media_conversion_analysis_cache_schema_cutover_m150() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 150),
        "contract must document M150"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let cutover = cache
        .split("fn reset_cache_for_schema_cutover")
        .nth(1)
        .and_then(|s| s.split("fn invalidate_old_algorithm_entries").next())
        .expect("reset_cache_for_schema_cutover body"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cutover.contains("analysis_cache_schema_cutover_purge"),
        "schema cutover must batch-audit (M150)"
    );
    assert!(
        cutover.contains("TRUNCATE TABLE path_index"),
        "schema cutover must truncate cache tables (M150)"
    );
}

#[test]
fn media_conversion_contract_m1_m150_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 150),
        "contract must document M150"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 150, 119);
}

#[test]
fn media_conversion_analysis_cache_path_file_size_m151() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 151),
        "contract must document M151"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        cache.matches("r.file_size FROM path_index").count() >= 3
            || cache
                .matches("r.content_fingerprint_hash, r.file_size FROM path_index")
                .count()
                >= 3,
        "path JOIN must read r.file_size (M151)"
    );
    for phase in ["image-path-hit", "quality-path-hit", "video-path-hit"] {
        assert!(
            cache.contains(phase),
            "path file_size gate must cover {phase} (M151)"
        );
    }
}

#[test]
fn media_conversion_analysis_cache_hit_validation_chain_m152() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 152),
        "contract must document M152"
    );

    let cache = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/analysis_cache.rs",
    ))
    .expect("analysis_cache.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "reject_stale_path_index_hit",
        "reject_cache_hit_on_record_file_size_mismatch",
        "cache_record_algorithm_current",
        "reject_cache_hit_on_content_fingerprint_mismatch",
        "unpack_cached_payload",
        "image_analysis_is_positive_cache_entry",
        "quality_analysis_is_positive_cache_entry",
        "video_analysis_is_positive_cache_entry",
    ] {
        assert!(
            cache.contains(sym),
            "cache hit chain must include {sym} (M152)"
        );
    }
}

#[test]
fn media_conversion_training_corpus_maturity_ssot_m153() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 153),
        "contract must document M153"
    );
    assert!(
        contract.contains("image_quality_inference_log"),
        "M153 must document inference_log is not training corpus"
    );
    assert!(
        contract.contains("--training-mode"),
        "M153 must document run_training training-mode scope"
    );

    let quality_db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_quality_db.rs",
    ))
    .expect("image_quality_db.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        quality_db.contains("pub fn check_quality_db_maturity"),
        "static quality maturity gate must exist (M153)"
    );

    let database = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        database.contains("fn check_loop_intent_db_maturity"),
        "loop intent maturity gate must exist (M153)"
    );
    assert!(
        database.contains("evaluate_training_corpus_maturity"),
        "db-health must evaluate loop + static corpora (M153)"
    );

    let algorithm_runtime = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/algorithm_runtime.rs",
    ))
    .expect("algorithm_runtime.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "loop_corpus_samples_shortfall",
        "quality_corpus_samples_shortfall",
        "TrainingCorpusMaturity",
    ] {
        assert!(
            algorithm_runtime.contains(sym),
            "algorithm_runtime must define {sym} (M153 corpus SSOT)"
        );
    }

    let corpus_py = fs::read_to_string(root.join("crates/dev/scripts/mfb_corpus_thresholds.py"))
        .expect("mfb_corpus_thresholds.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        corpus_py.contains("loop_corpus_samples_shortfall"),
        "Python corpus thresholds must mirror Rust shortfall formula"
    );

    let pipeline = fs::read_to_string(root.join("crates/dev/scripts/training_pipeline.py"))
        .expect("training_pipeline.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "evaluate_image_quality_model_status",
        "evaluate_loop_intent_runtime_status",
        "verify_stack_readiness",
    ] {
        assert!(
            pipeline.contains(sym),
            "training_pipeline must define {sym} (M153)"
        );
    }

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("--training-mode"),
        "run_training must expose --training-mode (M153)"
    );
}

#[test]
fn media_conversion_static_training_runtime_fill_m154() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 154),
        "contract must document M154"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("training_mode: str = \"all\""),
        "fill_runtime_assets must accept training_mode (M154)"
    );
    assert!(
        run_training.contains("training_mode=static: runtime fill skips loop_intent"),
        "static mode must skip loop finalize (M154)"
    );
    assert!(
        run_training.contains("include_loop_intent = training_mode in (\"all\", \"loop\")"),
        "loop finalize must be gated by training_mode (M154)"
    );
    assert!(
        run_training.contains("include_image_quality = training_mode in (\"all\", \"static\")"),
        "image_quality finalize must be gated by training_mode (M154)"
    );
}

#[test]
fn media_conversion_training_ingest_balance_caps_m155() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 155),
        "contract must document M155"
    );
    assert!(
        contract.contains("max_non_loop<=0"),
        "M155 must document single-sided loop cap when max_non_loop unset"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("static_pair_target"),
        "balance summary must record static pair target (M155)"
    );
    assert!(
        run_training.contains("\"loop_balance_mode\""),
        "balance summary must record loop balance mode (M155)"
    );
    assert!(
        run_training.contains("if max_non_loop <= 0:"),
        "single-sided loop cap must not use bilateral min (M155)"
    );
    assert!(
        run_training.contains("Single-sided loop corpus (max_non_loop unset/0)"),
        "loop balance branch must be documented in source (M155)"
    );
}

#[test]
fn media_conversion_training_ingest_balance_skew_m156() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 156),
        "contract must document M156"
    );
    assert!(
        contract.contains("training_ingest_balance_skew"),
        "M156 must name the skew audit tag"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("def warn_static_balance_skew"),
        "skew audit helper must exist (M156)"
    );
    assert!(
        run_training.contains("[WARN] training_ingest_balance_skew"),
        "skew audit must be visible on stderr (M156)"
    );
    assert!(
        run_training.contains("warn_static_balance_skew("),
        "balance path must invoke skew audit (M156)"
    );
}

#[test]
fn media_conversion_static_low_tier_any_logic_m157() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 157),
        "contract must document M157"
    );

    let tier_rs = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/training_tier_audit.rs",
    ))
    .expect("training_tier_audit.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tier_rs.contains("LOW_TIER_LOGIC: TierRuleLogic = TierRuleLogic::Any"),
        "low tier must use ANY (M157)"
    );
    assert!(
        tier_rs.contains("dead zone must not veto dimension-only lows (M157)"),
        "ANY path must document dead-zone exception (M157)"
    );

    let rules = fs::read_to_string(root.join("crates/dev/src/config/training_rules.json"))
        .expect("training_rules.json must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let low_section = rules
        .split("\"low_quality\"")
        .nth(1)
        .expect("training_rules must define low_quality"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        low_section.contains("\"logic\": \"ANY\""),
        "low_quality tier logic must be ANY in training_rules.json (M157)"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("\"low_quality\": {\n        \"logic\": \"ANY\""),
        "RUST_STATIC_TIER_CONTRACT must pin low ANY (M157)"
    );
}

#[test]
fn media_conversion_discipline_layer_closure_m158() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 158),
        "contract must document M158"
    );

    let discipline_seal = read_hardening_doc(&root, "MEDIA_CONVERSION_DISCIPLINE_SEAL.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        discipline_seal.contains("media_conversion_discipline_layer_closure_m158"),
        "discipline seal must name closure test (M158)"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in ["delivery_audit_optional_u32", "delivery_audit_optional_u64"] {
        assert!(
            gate.contains(sym),
            "gate must export {sym} for audit-only scalars (M158)"
        );
    }

    let numeric_hits = delivery_numeric_forgery_offenders(&root);
    assert!(
        numeric_hits.is_empty(),
        "M158 discipline: zero unallowlisted numeric-forgery lines:\n{}",
        numeric_hits.join("\n")
    );

    let extended_hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M68_EXTENDED,
    );
    let extended_hits: Vec<String> = extended_hits
        .into_iter()
        .filter(|line| !line.contains("std::env::var(env_key).map_or(true, |raw|"))
        .collect();
    assert!(
        extended_hits.is_empty(),
        "M158 discipline: M68 extended scan must be clear:\n{}",
        extended_hits.join("\n")
    );

    let substrate_hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M69_SUBSTRATE,
    );
    assert!(
        substrate_hits.is_empty(),
        "M158 discipline: M69 substrate scan must be clear:\n{}",
        substrate_hits.join("\n")
    );

    let gate_log_anomaly = gate.matches("log_anomaly!(").count();
    assert_eq!(
        gate_log_anomaly, 1,
        "M158 discipline: exactly one log_anomaly! site in gate (found {gate_log_anomaly})"
    );

    let poison_hits = delivery_poison_recovery_offenders(&root);
    assert!(
        poison_hits.is_empty(),
        "M158/M167 discipline: production must not use raw PoisonError recovery outside gate:\n{}",
        poison_hits.join("\n")
    );

    let cwd_hits = delivery_raw_current_dir_offenders(&root);
    assert!(
        cwd_hits.is_empty(),
        "M158/M168 discipline: production must not call std::env::current_dir() outside gate:\n{}",
        cwd_hits.join("\n")
    );

    let temp_hits = delivery_raw_temp_dir_offenders(&root);
    assert!(
        temp_hits.is_empty(),
        "M158/M169 discipline: production must not call std::env::temp_dir() outside gate:\n{}",
        temp_hits.join("\n")
    );

    let terminal_mutex_hits = delivery_terminal_mutex_offenders(&root);
    assert!(
        terminal_mutex_hits.is_empty(),
        "M158/M169 discipline: production must not silently skip TERMINAL_LOCK on poison:\n{}",
        terminal_mutex_hits.join("\n")
    );

    let unscoped_tempfile_hits = delivery_unscoped_tempfile_offenders(&root);
    assert!(
        unscoped_tempfile_hits.is_empty(),
        "M158/M169/M170 discipline: production must create temp files/dirs via gate scratch \
         SSOT:\n{}",
        unscoped_tempfile_hits.join("\n")
    );

    let mfb_tmp_hits = delivery_raw_get_mfb_tmp_offenders(&root);
    assert!(
        mfb_tmp_hits.is_empty(),
        "M158/M171 discipline: production must not call get_mfb_tmp_dir() outside \
         gate/process_lock:\n{}",
        mfb_tmp_hits.join("\n")
    );

    let tempfile_in_hits = delivery_raw_tempfile_in_offenders(&root);
    assert!(
        tempfile_in_hits.is_empty(),
        "M158/M171 discipline: production must not call .tempfile_in() outside gate:\n{}",
        tempfile_in_hits.join("\n")
    );

    let parent_unwrap_hits = delivery_raw_parent_unwrap_offenders(&root);
    assert!(
        parent_unwrap_hits.is_empty(),
        "M158/M172 discipline: production must not use parent().unwrap_or* outside gate:\n{}",
        parent_unwrap_hits.join("\n")
    );

    let path_dot_hits = delivery_raw_path_dot_offenders(&root);
    assert!(
        path_dot_hits.is_empty(),
        "M158/M172 discipline: production must not use Path::new(\".\") outside gate:\n{}",
        path_dot_hits.join("\n")
    );

    let silent_mkdir_hits = delivery_silent_create_dir_offenders(&root);
    assert!(
        silent_mkdir_hits.is_empty(),
        "M158/M172 discipline: production must not silently discard create_dir_all:\n{}",
        silent_mkdir_hits.join("\n")
    );

    let silent_fs_hits = delivery_silent_fs_offenders(&root);
    assert!(
        silent_fs_hits.is_empty(),
        "M158/M173 discipline: production must not silently discard fs remove/rename/copy:\n{}",
        silent_fs_hits.join("\n")
    );

    let strip_prefix_hits = delivery_path_strip_prefix_fallback_offenders(&root);
    assert!(
        strip_prefix_hits.is_empty(),
        "M158/M173 discipline: Path strip_prefix fallbacks must use strip_prefix_or_self:\n{}",
        strip_prefix_hits.join("\n")
    );

    let remove_unwrap_hits = delivery_remove_file_unwrap_offenders(&root);
    assert!(
        remove_unwrap_hits.is_empty(),
        "M158/M174 discipline: remove_file must use delivery_remove_file_or_audit:\n{}",
        remove_unwrap_hits.join("\n")
    );

    let file_stem_hits = delivery_raw_file_stem_fallback_offenders(&root);
    assert!(
        file_stem_hits.is_empty(),
        "M158/M174 discipline: file_stem must not use raw unwrap_or fallbacks:\n{}",
        file_stem_hits.join("\n")
    );

    let raw_remove_hits = delivery_raw_remove_file_offenders(&root);
    assert!(
        raw_remove_hits.is_empty(),
        "M158/M175 discipline: remove_file must use delivery_remove_file_or_audit:\n{}",
        raw_remove_hits.join("\n")
    );

    let extension_map_or_hits = delivery_extension_map_or_offenders(&root);
    assert!(
        extension_map_or_hits.is_empty(),
        "M158/M176 discipline: extension fallbacks must use gate helpers:\n{}",
        extension_map_or_hits.join("\n")
    );

    let stderr_line_hits = delivery_stderr_line_unwrap_offenders(&root);
    assert!(
        stderr_line_hits.is_empty(),
        "M158/M176 discipline: stderr lines must not use unwrap_or fallbacks:\n{}",
        stderr_line_hits.join("\n")
    );

    let file_name_map_or_hits = delivery_file_name_map_or_offenders(&root);
    assert!(
        file_name_map_or_hits.is_empty(),
        "M158/M177 discipline: file_name fallbacks must use gate helpers:\n{}",
        file_name_map_or_hits.join("\n")
    );

    let file_stem_ok_or_hits = delivery_file_stem_ok_or_offenders(&root);
    assert!(
        file_stem_ok_or_hits.is_empty(),
        "M158/M178 discipline: strict stem resolution must use gate helpers:\n{}",
        file_stem_ok_or_hits.join("\n")
    );

    let gpu_best_size_hits = delivery_gpu_best_size_unwrap_offenders(&root);
    assert!(
        gpu_best_size_hits.is_empty(),
        "M158/M179 discipline: GPU phase best_size must use gate helpers:\n{}",
        gpu_best_size_hits.join("\n")
    );

    let precheck_nb_direct_hits = delivery_precheck_nb_frames_direct_offenders(&root);
    assert!(
        precheck_nb_direct_hits.is_empty(),
        "M158/M179 discipline: precheck nb_frames must use \
         explore_precheck_nb_frames_resolved:\n{}",
        precheck_nb_direct_hits.join("\n")
    );

    let read_error_unwrap_hits = delivery_read_error_unwrap_offenders(&root);
    assert!(
        read_error_unwrap_hits.is_empty(),
        "M158/M180 discipline: probe decode errors must use gate helpers:\n{}",
        read_error_unwrap_hits.join("\n")
    );

    let jpeg_slice_unwrap_hits = delivery_jpeg_inline_slice_unwrap_offenders(&root);
    assert!(
        jpeg_slice_unwrap_hits.is_empty(),
        "M158/M180 discipline: JPEG slice fallbacks must use probe_jpeg_buffer_slice:\n{}",
        jpeg_slice_unwrap_hits.join("\n")
    );

    let checkpoint_start_hits = delivery_checkpoint_start_unwrap_offenders(&root);
    assert!(
        checkpoint_start_hits.is_empty(),
        "M158/M181 discipline: checkpoint lock start time must use gate helper:\n{}",
        checkpoint_start_hits.join("\n")
    );

    let date_field_hits = delivery_date_field_unwrap_offenders(&root);
    assert!(
        date_field_hits.is_empty(),
        "M158/M181 discipline: date-analysis fields must use delivery_exiftool_field_or_empty:\n{}",
        date_field_hits.join("\n")
    );

    let content_type_hits = delivery_content_type_unwrap_offenders(&root);
    assert!(
        content_type_hits.is_empty(),
        "M158/M182 discipline: content_type must use quality_content_type_for_crf_or_unknown:\n{}",
        content_type_hits.join("\n")
    );

    let jpeg_qt_hits = delivery_jpeg_qt_inline_audit_offenders(&root);
    assert!(
        jpeg_qt_hits.is_empty(),
        "M158/M182 discipline: JPEG QT must use delivery_jpeg_qt_cell_u16_or_one:\n{}",
        jpeg_qt_hits.join("\n")
    );

    let tool_inline_hits = delivery_tool_path_inline_offenders(&root);
    assert!(
        tool_inline_hits.is_empty(),
        "M158/M183 discipline: tool path must use gate SSOT:\n{}",
        tool_inline_hits.join("\n")
    );

    let vqd_crf_hits = delivery_vqd_crf_inline_offenders(&root);
    assert!(
        vqd_crf_hits.is_empty(),
        "M158/M183 discipline: video CRF must use gate SSOT:\n{}",
        vqd_crf_hits.join("\n")
    );

    let path_env_hits = delivery_path_env_inline_offenders(&root);
    assert!(
        path_env_hits.is_empty(),
        "M158/M184 discipline: PATH env must use delivery_path_env_or_empty:\n{}",
        path_env_hits.join("\n")
    );

    let memory_mb_hits = delivery_memory_mb_inline_offenders(&root);
    assert!(
        memory_mb_hits.is_empty(),
        "M158/M184 discipline: RAM probe must use delivery_system_memory_mb_or_zero:\n{}",
        memory_mb_hits.join("\n")
    );

    let rsync_hits = delivery_rsync_inline_offenders(&root);
    assert!(
        rsync_hits.is_empty(),
        "M158/M184 discipline: rsync path must use delivery_rsync_executable_or_default:\n{}",
        rsync_hits.join("\n")
    );

    let batch_hits = delivery_batch_perceived_speed_unwrap_offenders(&root);
    assert!(
        batch_hits.is_empty(),
        "M158/M186 discipline: perceived-speed batch caching must not use unwrap_or_else:\n{}",
        batch_hits.join("\n")
    );

    let analyzer_hits = delivery_image_analyzer_probe_unwrap_offenders(&root);
    assert!(
        analyzer_hits.is_empty(),
        "M158/M186 discipline: image_analyzer probe paths must not inline unwrap_or_else \
         fallbacks:\n{}",
        analyzer_hits.join("\n")
    );

    let db_hits = delivery_db_inline_unwrap_offenders(&root);
    assert!(
        db_hits.is_empty(),
        "M158/M187 discipline: database percentile/metadata fallbacks must use gate helpers:\n{}",
        db_hits.join("\n")
    );

    let runtime_ui_hits = delivery_runtime_ui_stream_uoe_offenders(&root);
    assert!(
        runtime_ui_hits.is_empty(),
        "M158/M188 discipline: runtime/ui/stream size must not use non-panic unwrap_or_else:\n{}",
        runtime_ui_hits.join("\n")
    );

    let explore_jxl_hits = delivery_explore_jxl_uoe_offenders(&root);
    assert!(
        explore_jxl_hits.is_empty(),
        "M158/M189 discipline: explore frame-count / jxl margin must use gate helpers:\n{}",
        explore_jxl_hits.join("\n")
    );

    let metrics_margin_hits = delivery_metrics_margin_uoe_offenders(&root);
    assert!(
        metrics_margin_hits.is_empty(),
        "M158/M190 discipline: image metrics / metadata margin must not use non-panic \
         unwrap_or_else:\n{}",
        metrics_margin_hits.join("\n")
    );

    let runtime_explore_hits = delivery_runtime_explore_uoe_offenders(&root);
    assert!(
        runtime_explore_hits.is_empty(),
        "M158/M191 discipline: runtime/explore critical paths must not use non-panic \
         unwrap_or_else:\n{}",
        runtime_explore_hits.join("\n")
    );

    let gpu_explore_mapor_hits = delivery_gpu_explore_mapor_offenders(&root);
    assert!(
        gpu_explore_mapor_hits.is_empty(),
        "M158/M192 discipline: GPU/explore/pipeline map_or fallbacks must use gate SSOT:\n{}",
        gpu_explore_mapor_hits.join("\n")
    );

    let probe_gpu_mapor_hits = delivery_probe_gpu_mapor_offenders(&root);
    assert!(
        probe_gpu_mapor_hits.is_empty(),
        "M158/M193 discipline: probe/GPU coarse map_or fallbacks must use gate SSOT:\n{}",
        probe_gpu_mapor_hits.join("\n")
    );

    let batch_db_conversion_hits = delivery_batch_db_conversion_mapor_offenders(&root);
    assert!(
        batch_db_conversion_hits.is_empty(),
        "M158/M194 discipline: batch/DB/conversion map_or fallbacks must use gate SSOT:\n{}",
        batch_db_conversion_hits.join("\n")
    );

    let analyzer_loop_hdr_hits = delivery_analyzer_loop_hdr_mapor_offenders(&root);
    assert!(
        analyzer_loop_hdr_hits.is_empty(),
        "M158/M195 discipline: analyzer/loop/HDR map_or fallbacks must use gate SSOT:\n{}",
        analyzer_loop_hdr_hits.join("\n")
    );

    let io_gpu_vector_db_hits = delivery_io_gpu_vector_db_mapor_offenders(&root);
    assert!(
        io_gpu_vector_db_hits.is_empty(),
        "M158/M196 discipline: IO/GPU/vector/quality-db map_or fallbacks must use gate SSOT:\n{}",
        io_gpu_vector_db_hits.join("\n")
    );

    let builders_progress_copier_hits = delivery_builders_progress_copier_mapor_offenders(&root);
    assert!(
        builders_progress_copier_hits.is_empty(),
        "M158/M197 discipline: builders/progress/copier map_or fallbacks must use gate SSOT:\n{}",
        builders_progress_copier_hits.join("\n")
    );

    let api_explore_ffi_hits = delivery_api_explore_ffi_mapor_offenders(&root);
    assert!(
        api_explore_ffi_hits.is_empty(),
        "M158/M198 discipline: API/explore/FFI map_or fallbacks must use gate SSOT:\n{}",
        api_explore_ffi_hits.join("\n")
    );

    let runtime_unwrap_or_hits = delivery_runtime_unwrap_or_offenders(&root);
    assert!(
        runtime_unwrap_or_hits.is_empty(),
        "M158/M199 discipline: runtime unwrap_or fallbacks must use gate SSOT:\n{}",
        runtime_unwrap_or_hits.join("\n")
    );

    let database_training_unwrap_or_hits = delivery_database_training_unwrap_or_offenders(&root);
    assert!(
        database_training_unwrap_or_hits.is_empty(),
        "M158/M200 discipline: database/training unwrap_or fallbacks must use gate SSOT:\n{}",
        database_training_unwrap_or_hits.join("\n")
    );
}

#[test]
fn media_conversion_training_corpus_tier_m159() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 159),
        "contract must document M159"
    );

    let tier_rs = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/training_tier_audit.rs",
    ))
    .expect("training_tier_audit.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tier_rs.contains("HIGH_TIER_LOGIC: TierRuleLogic = TierRuleLogic::Any"),
        "high tier ANY (M159)"
    );
    assert!(
        tier_rs.contains("HIGH_PIXEL_MIN_DIM_GE: u32 = 1080"),
        "social-high min dim (M159)"
    );
    assert!(
        tier_rs.contains("LOW_PIXEL_MAX_DIM_LE: u32 = 512"),
        "low max dim 512 (M159)"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("def warn_corpus_tier_coverage"),
        "corpus coverage audit (M159)"
    );
    assert!(
        run_training.contains("training_corpus_tier_coverage"),
        "visible corpus coverage tag (M159)"
    );

    let rules = fs::read_to_string(root.join("crates/dev/src/config/training_rules.json"))
        .expect("training_rules.json must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let high_section = rules
        .split("\"high_quality\"")
        .nth(1)
        .expect("high_quality section"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        high_section.contains("\"logic\": \"ANY\""),
        "high_quality logic ANY in JSON (M159)"
    );
    assert!(
        high_section.contains("\"value\": 1080"),
        "high pixel_min_dim 1080 (M159, matches Rust HIGH_PIXEL_MIN_DIM_GE)"
    );
    let low_section = rules
        .split("\"low_quality\"")
        .nth(1)
        .expect("low_quality section"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        low_section.contains("\"value\": 512"),
        "low pixel_max_dim 512 (M159, matches Rust LOW_PIXEL_MAX_DIM_LE)"
    );
}

#[test]
fn media_conversion_unified_log_layout_m160() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 160),
        "contract must document M160"
    );
    assert!(
        contract.contains("LOGGING_LAYOUT.md"),
        "M160 must reference LOGGING_LAYOUT.md"
    );

    let log_paths = fs::read_to_string(root.join("crates/dev/scripts/mfb_log_paths.py"))
        .expect("mfb_log_paths.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "persistent_log_dir",
        "ensure_unified_log_dir",
        "unified_log_dir",
        "coerce_log_dir",
        "is_forbidden_log_path",
        "find_mfb_workspace_root",
        "archive_training_session_bundle",
        "archive_drag_drop_session_bundle",
        "append_jsonl_audit_record",
        "training_lane_slug",
        "TRAINING_LOG_LANES",
        "TrainingBundle_",
    ] {
        assert!(
            log_paths.contains(sym),
            "mfb_log_paths must define {sym} (M160)"
        );
    }
    assert!(
        !log_paths.contains("workspace_logs_dir_from_cwd"),
        "mfb_log_paths must not resolve <repo>/logs (M160)"
    );
    assert!(
        !log_paths.contains("_repo_root"),
        "mfb_log_paths must not fall back to script repo root (M160)"
    );

    let logging = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/logging.rs"))
        .expect("logging.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logging.contains("pub fn unified_log_dir()"),
        "Rust must expose unified_log_dir (M160)"
    );
    assert!(
        logging.contains("fn persistent_log_dir()"),
        "Rust must use persistent_log_dir (M160)"
    );
    assert!(
        logging.contains("fn coerce_log_dir("),
        "Rust must coerce forbidden workspace log paths (M160)"
    );
    assert!(
        logging.contains("fn is_forbidden_workspace_log_path"),
        "Rust must detect forbidden workspace log paths (M160)"
    );
    assert!(
        !logging.contains("workspace_logs_dir_from_cwd"),
        "logging.rs must not resolve workspace logs (M160)"
    );

    let entry_guard = fs::read_to_string(root.join("crates/dev/scripts/mfb_entry_guard.py"))
        .expect("mfb_entry_guard.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        entry_guard.contains("ensure_unified_log_dir"),
        "guard_main must pin unified logs (M160)"
    );
    assert!(
        entry_guard.contains("coerce_log_dir"),
        "detach_to_background must coerce log dir (M160)"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("training_tier_audit.jsonl"),
        "tier audit must live under unified log root (M160)"
    );
    assert!(
        run_training.contains("archive_training_session_bundle"),
        "run_training must archive session logs into TrainingBundle (M160)"
    );
    assert!(
        run_training.contains("MFB_TRAINING_LANE"),
        "run_training must pin parallel lane slug for manifests (M160)"
    );
    assert!(
        log_paths.contains("training_lane_slug"),
        "mfb_log_paths must expose training_lane_slug (M160)"
    );
    let launcher = root.join("crates/dev/scripts/start_training_three.py");
    assert!(
        launcher.is_file(),
        "start_training_three.py must exist for parallel lane launch"
    );
    assert!(
        !run_training.contains("target/training_logs"),
        "run_training must not use target/training_logs (M160)"
    );
    assert!(
        !run_training.contains("target/training_tier_audit"),
        "run_training must not use target/training_tier_audit (M160)"
    );
    assert!(
        run_training.contains("pin_training_log_dir"),
        "run_training must pin MFB_LOG_DIR at session start (M160)"
    );

    let drag_drop = fs::read_to_string(root.join("crates/dev/src/bin/drag_and_drop_processor.rs"))
        .expect("drag_and_drop_processor.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        drag_drop.contains("ensure_unified_log_dir"),
        "drag-and-drop must use ensure_unified_log_dir (M160)"
    );
    assert!(
        drag_drop.contains("archive_drag_drop_session_bundle"),
        "drag-and-drop must archive session bundle with manifest (M160)"
    );
    assert!(
        logging.contains("log_file_includes_progress"),
        "Rust logging must gate mfb::progress via MFB_LOG_PROGRESS (M160)"
    );
    assert!(
        !drag_drop.contains("PROJECT_ROOT / \"logs\"") && !drag_drop.contains(".join(\"logs\")"),
        "drag-and-drop must not use in-repo logs (M160)"
    );

    let layout = read_hardening_doc(&root, "LOGGING_LAYOUT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        layout.contains("~/.modern_format_boost/logs"),
        "LOGGING_LAYOUT must document home log root (M160)"
    );
}

#[test]
fn media_conversion_contract_m1_m160_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 160),
        "contract must document M160"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 160, 129);
}

#[test]
fn media_conversion_training_audio_silence_ssot_m161() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 161),
        "contract must document M161"
    );
    assert!(
        contract.contains("detect_audio_silence"),
        "M161 must name penetration helper"
    );
    assert!(
        contract.contains("validate_video_section"),
        "M161 must document schema-only video rules"
    );

    let penetration = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_penetration.rs",
    ))
    .expect("media_penetration.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        penetration.contains("fn detect_audio_silence"),
        "audio silence must use media_penetration (M161)"
    );
    assert!(
        penetration.contains("volumedetect"),
        "audio silence must decode via volumedetect (M161)"
    );
    assert!(
        penetration.contains("AUDIO_SILENCE_THRESHOLD_DB"),
        "audio silence threshold must be SSOT constant (M161)"
    );

    let video_detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        video_detection.contains("detect_audio_silence(path)"),
        "video_detection must downgrade fake audio via penetration (M161)"
    );

    let loop_intent = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        loop_intent.contains("has_confirmed_silent_or_no_audio"),
        "loop tree must expose confirmed silent/no-audio (M161)"
    );
    assert!(
        loop_intent.contains("fn apply_penetration"),
        "loop evaluator must run penetration before tree (M161)"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("def validate_video_section"),
        "run_training must validate video rule schema (M161)"
    );
    assert!(
        !run_training.contains("def evaluate_video_section"),
        "run_training must not execute video contrast rules at collect (M161)"
    );
    let rules = fs::read_to_string(root.join("crates/dev/src/config/training_rules.json"))
        .expect("training_rules.json must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        rules.contains("\"no_audio AND duration_lt\""),
        "training_rules documents no_audio contrast (M161)"
    );
}

#[test]
fn media_conversion_training_loop_lanes_m162() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 162),
        "contract must document M162"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "rule_is_supported_non_loop_media_file",
        "rule_is_supported_loop_intent_media_file",
        "explicit_loop_balance_bucket",
    ] {
        assert!(
            run_training.contains(sym),
            "run_training must define {sym} (M162)"
        );
    }

    let rules = fs::read_to_string(root.join("crates/dev/src/config/training_rules.json"))
        .expect("training_rules.json must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        rules.contains("is_supported_loop_intent_media_file"),
        "training_rules must reference loop_intent media gate (M162)"
    );
    assert!(
        rules.contains("contrast_fast_silent_loop"),
        "training_rules must document fast silent loop contrast (M162)"
    );

    let launcher = fs::read_to_string(root.join("crates/dev/scripts/start_training_four.py"))
        .expect("start_training_four.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for lane in ["static_high", "static_low", "loop_high", "loop_low"] {
        assert!(
            launcher.contains(lane),
            "four-lane launcher must reference {lane} (M162)"
        );
    }

    let layout = read_hardening_doc(&root, "LOGGING_LAYOUT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        layout.contains("loop_low/"),
        "LOGGING_LAYOUT must document loop_low lane (M162)"
    );
}

#[test]
fn media_conversion_contract_m1_m162_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 162),
        "contract must document M162"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 162, 131);
}

#[test]
fn media_conversion_training_loop_collect_static_raster_m163() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 163),
        "contract must document M163"
    );

    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "passes_loop_raster_animation_gate",
        "try_probe_loop_intent_for_collect",
        "emit_loop_collect_rejection",
        "loop_probe_rejected",
        "loop_static_raster",
    ] {
        assert!(
            run_training.contains(sym),
            "run_training must define or reference {sym} (M163)"
        );
    }
    assert!(
        run_training.contains("if label == \"animated_loop\":"),
        "collect_samples must gate animated_loop label (M163)"
    );
    assert!(
        run_training.contains("loop intent balance probe failed; refusing uncertain fallback"),
        "balance probe failure must fail closed by default (M163)"
    );
    assert!(
        run_training.contains("if fail_closed_training_enabled():"),
        "debug-only uncertain fallback must stay gated by fail_closed_training_enabled (M163)"
    );
    let body = run_training
        .split("def collect_samples(")
        .nth(1)
        .and_then(|rest| rest.split("def resolve_local_source_path").next())
        .expect("collect_samples body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        body.contains("passes_loop_raster_animation_gate"),
        "collect_samples animated_loop path must use raster gate (M163)"
    );
    assert!(
        body.contains("try_probe_loop_intent_for_collect"),
        "collect_samples animated_loop path must require loop probe (M163)"
    );
}

#[test]
fn media_conversion_training_session_audit_m211() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 211),
        "contract must document M211"
    );
    let layout = read_hardening_doc(&root, "LOGGING_LAYOUT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        layout.contains("training_session_audit.jsonl"),
        "LOGGING_LAYOUT must document training_session_audit.jsonl (M211)"
    );
    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("TrainingSessionRecorder"),
        "run_training must use TrainingSessionRecorder (M211)"
    );
    assert!(
        run_training.contains("training_session_heartbeat"),
        "run_training must emit scan heartbeats (M211)"
    );
    let audit_mod =
        fs::read_to_string(root.join("crates/dev/scripts/mfb_training_session_audit.py"))
            .expect("mfb_training_session_audit.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        audit_mod.contains("TRAINING-EXIT") || audit_mod.contains("session_exit"),
        "audit module must record session_exit (M211)"
    );
}

#[test]
fn media_conversion_tool_and_ffi_paths_m164() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 164),
        "contract must document M164"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    {
        let sym = "ffi_ingest_path_list_or_delimited";
        assert!(gate.contains(sym), "gate must export {sym} (M164)");
    }
    let ingest_body = gate_fn_body(&gate, "ffi_ingest_path_list_or_delimited");
    assert!(
        ingest_body.contains("delivery_strict_batch_audit"),
        "ffi_ingest_path_list_or_delimited must strict-gate JSON/delimiter fallbacks (M164)"
    );

    let common = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/common_utils.rs",
    ))
    .expect("common_utils.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        common.contains("pub fn resolve_tool_path_or_audit"),
        "common_utils must export resolve_tool_path_or_audit (M164)"
    );
    assert!(
        common.contains("pub fn resolve_tool_path_or_audit")
            && common.contains("delivery_tool_path_or_bare_name"),
        "resolve_tool_path_or_audit must delegate to gate SSOT (M164/M183)"
    );
    assert!(
        gate.contains("delivery_tool_path_or_bare_name"),
        "gate must export delivery_tool_path_or_bare_name (M183)"
    );
    let tool_body = gate_fn_body(&gate, "delivery_tool_path_or_bare_name");
    assert!(
        tool_body.contains("delivery_strict_batch_audit")
            && tool_body.contains("tool_path_unresolved"),
        "delivery_tool_path_or_bare_name must strict-gate bare-name fallback (M183)"
    );

    let forbidden_patterns = [
        "resolve_tool_path(name).unwrap_or_else(|| std::path::PathBuf::from(name))",
        "resolve_tool_path(command_name).unwrap_or_else(|| std::path::PathBuf::from(command_name))",
    ];
    for rel in [
        "crates/foundation/src/tools.rs",
        "crates/foundation/src/builder_base.rs",
        "crates/foundation/src/image_builders.rs",
        "crates/foundation/src/common_utils.rs",
        "crates/foundation/src/c_api.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for forbidden in forbidden_patterns {
            if rel.ends_with("common_utils.rs") && forbidden.contains("resolve_tool_path(name)") {
                // Allowed only inside `resolve_tool_path_or_audit` (SSOT).
                let ssot_only = prod.matches(forbidden).count()
                    <= prod.matches("pub fn resolve_tool_path_or_audit").count();
                assert!(
                    ssot_only,
                    "{rel} must not use silent tool-path fallback outside \
                     resolve_tool_path_or_audit (M164)"
                );
                continue;
            }
            assert!(
                !prod.contains(forbidden),
                "{rel} must not use silent tool-path fallback (M164): {forbidden}"
            );
        }
    }
    for (rel, needle) in [
        (
            "crates/foundation/src/tools.rs",
            "resolve_tool_path_or_audit",
        ),
        (
            "crates/foundation/src/builder_base.rs",
            "resolve_tool_path_or_audit",
        ),
        (
            "crates/foundation/src/image_builders.rs",
            "resolve_tool_path_or_audit",
        ),
        (
            "crates/foundation/src/c_api.rs",
            "ffi_ingest_path_list_or_delimited",
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains(needle),
            "{rel} must route through {needle} (M164)"
        );
    }
    let c_api = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/c_api.rs"))
        .expect("c_api.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_c_api = production_scope(&c_api);
    assert!(
        !prod_c_api.contains("serde_json::from_str::<Vec<String>>"),
        "c_api must not parse ingest paths inline (M164)"
    );
}

#[test]
fn media_conversion_delivery_batch_mutex_m165() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 165),
        "contract must document M165"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "mutex_into_inner_or_recover",
        "rwlock_read_guard_or_recover",
        "rwlock_write_guard_or_recover",
        "delivery_batch_output_bytes_or_input",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M165)");
    }
    let output_body = gate_fn_body(&gate, "delivery_batch_output_bytes_or_input");
    assert!(
        output_body.contains("delivery_strict_batch_audit"),
        "delivery_batch_output_bytes_or_input must strict-gate (M165)"
    );

    let forbidden = [
        "output_size().unwrap_or_else(|| result.input_size())",
        ".unwrap_or_else(std::sync::PoisonError::into_inner)",
        "resolve_tool_path(constants::TOOL_MAGICK)",
    ];
    for rel in [
        "crates/foundation/src/cli_runner.rs",
        "crates/foundation/src/batch.rs",
        "crates/foundation/src/builder_base.rs",
        "crates/foundation/src/image_builders.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for pattern in forbidden {
            assert!(
                !prod.contains(pattern),
                "{rel} must not retain silent fallback {pattern} (M165)"
            );
        }
    }
    let cli = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/cli_runner.rs",
    ))
    .expect("cli_runner.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_cli = production_scope(&cli);
    for needle in [
        "mutex_guard_or_recover",
        "mutex_into_inner_or_recover",
        "delivery_batch_output_bytes_or_input",
    ] {
        assert!(
            prod_cli.contains(needle),
            "cli_runner must route batch paths through {needle} (M165)"
        );
    }
    let builders = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/builder_base.rs",
    ))
    .expect("builder_base.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_builders = production_scope(&builders);
    assert!(
        prod_builders.contains("rwlock_read_guard_or_recover")
            && prod_builders.contains("rwlock_write_guard_or_recover"),
        "builder_base must use rwlock recovery helpers (M165)"
    );
}

#[test]
fn media_conversion_gpu_checkpoint_mutex_m166() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 166),
        "contract must document M166"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("gpu_concurrency_cap_cache"),
        "gate must use mutex_guard_or_recover for GPU concurrency cap cache (M166)"
    );
    let cap_body = gate_fn_body(&gate, "gpu_concurrency_max_or_default");
    assert!(
        cap_body.contains("mutex_guard_or_recover(\"gpu_concurrency_cap_cache\""),
        "gpu_concurrency_max_or_default must recover cap cache mutex via gate (M166)"
    );

    let poison = ".unwrap_or_else(std::sync::PoisonError::into_inner)";
    for rel in [
        "crates/foundation/src/gpu_accel.rs",
        "crates/foundation/src/checkpoint.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            !prod.contains(poison),
            "{rel} production must not use raw PoisonError recovery (M166)"
        );
        assert!(
            prod.contains("mutex_guard_or_recover"),
            "{rel} must route mutex locks through gate (M166)"
        );
    }
    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/gpu_accel.rs",
    ))
    .expect("gpu_accel.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_gpu = production_scope(&gpu);
    for branch in [
        "gpu_progress_lines",
        "gpu_concurrency_acquire",
        "gpu_concurrency_cvar",
        "gpu_concurrency_release",
        "gpu_accel_cache",
    ] {
        assert!(
            prod_gpu.contains(branch),
            "gpu_accel must name mutex branch {branch} (M166)"
        );
    }
    let checkpoint = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/checkpoint.rs",
    ))
    .expect("checkpoint.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_checkpoint = production_scope(&checkpoint);
    assert!(
        prod_checkpoint.matches("checkpoint_completed").count() >= 5,
        "checkpoint must use checkpoint_completed mutex branches (M166)"
    );
}

#[test]
fn media_conversion_discipline_poison_logging_m167() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 167),
        "contract must document M167"
    );

    let poison_hits = delivery_poison_recovery_offenders(&root);
    assert!(
        poison_hits.is_empty(),
        "M167 discipline: production must not use raw PoisonError recovery outside gate:\n{}",
        poison_hits.join("\n")
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_cwd_or_audit",
        "tracing_registry_env_filter_or_config",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M167)");
    }
    let filter_body = gate_fn_body(&gate, "tracing_registry_env_filter_or_config");
    assert!(
        filter_body.contains("delivery_logging_path_audit"),
        "tracing_registry_env_filter_or_config must audit invalid RUST_LOG (M167)"
    );

    let logging = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/logging.rs"))
        .expect("logging.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_logging = production_scope(&logging);
    assert!(
        prod_logging.contains("delivery_cwd_or_audit")
            && prod_logging.contains("tracing_registry_env_filter_or_config"),
        "logging production must use M167 gate helpers"
    );
    assert!(
        !prod_logging.contains("EnvFilter::try_from_default_env"),
        "logging must not build EnvFilter inline (M167)"
    );
    assert!(
        !prod_logging.contains(concat!(
            "std::env::current_dir()\n            .",
            "ok",
            "()"
        )),
        "logging must not silently ignore cwd for workspace detection (M167)"
    );

    let path_validator = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/path_validator.rs",
    ))
    .expect("path_validator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_pv = production_scope(&path_validator);
    assert!(
        prod_pv.contains("delivery_cwd_or_audit"),
        "path_validator must use delivery_cwd_or_audit for relative outputs (M167)"
    );
    assert!(
        !prod_pv.contains("std::env::current_dir()\n            .map_err"),
        "path_validator must not call current_dir directly (M167)"
    );
}

#[test]
fn media_conversion_conversion_cwd_m168() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 168),
        "contract must document M168"
    );

    let cwd_hits = delivery_raw_current_dir_offenders(&root);
    assert!(
        cwd_hits.is_empty(),
        "M168 discipline: production must not call std::env::current_dir() outside gate:\n{}",
        cwd_hits.join("\n")
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_join_relative_to_cwd_or_err",
        "delivery_absolute_output_path_or_dot",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M168)");
    }

    let conversion = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/conversion.rs",
    ))
    .expect("conversion.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_conversion = production_scope(&conversion);
    assert!(
        prod_conversion.contains("delivery_join_relative_to_cwd_or_err"),
        "conversion must use delivery_join_relative_to_cwd_or_err (M168)"
    );
    assert!(
        !prod_conversion.contains("std::env::current_dir()"),
        "conversion production must not call current_dir directly (M168)"
    );

    let img = fs::read_to_string(root.join("crates/img/src/conversion_api.rs"))
        .expect("img conversion_api.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_img = production_scope(&img);
    assert!(
        prod_img.contains("delivery_absolute_output_path_or_dot"),
        "img must use delivery_absolute_output_path_or_dot (M168)"
    );
    assert!(
        !prod_img.contains("std::env::current_dir()"),
        "img production must not call current_dir directly (M168)"
    );

    let common = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/common_utils.rs",
    ))
    .expect("common_utils.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_common = production_scope(&common);
    assert!(
        prod_common.contains("delivery_cwd_or_audit"),
        "common_utils cache paths must use delivery_cwd_or_audit (M168)"
    );
    assert!(
        !prod_common.contains("std::env::current_dir()"),
        "common_utils production must not call current_dir directly (M168)"
    );
}

#[test]
fn media_conversion_terminal_temp_lock_m169() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 169),
        "contract must document M169"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_terminal_lock_guard",
        "delivery_scratch_temp_dir_or_system_temp",
        "delivery_temp_dir_in_scratch_or_err",
        "delivery_system_temp_dir_ssot",
    ] {
        assert!(
            gate.contains(sym),
            "gate must export or define {sym} (M169)"
        );
    }

    let temp_hits = delivery_raw_temp_dir_offenders(&root);
    assert!(
        temp_hits.is_empty(),
        "M169 discipline: production must not call std::env::temp_dir() outside gate:\n{}",
        temp_hits.join("\n")
    );

    let terminal_mutex_hits = delivery_terminal_mutex_offenders(&root);
    assert!(
        terminal_mutex_hits.is_empty(),
        "M169 discipline: production must not silently skip TERMINAL_LOCK on poison:\n{}",
        terminal_mutex_hits.join("\n")
    );

    let unscoped_tempfile_hits = delivery_unscoped_tempfile_offenders(&root);
    assert!(
        unscoped_tempfile_hits.is_empty(),
        "M169 discipline: production must create temp files/dirs via gate scratch SSOT:\n{}",
        unscoped_tempfile_hits.join("\n")
    );

    let progress = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/progress.rs",
    ))
    .expect("progress.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_progress = production_scope(&progress);
    assert!(
        prod_progress.contains("delivery_terminal_lock_guard"),
        "progress must use delivery_terminal_lock_guard (M169)"
    );
    assert!(
        !prod_progress.contains(concat!(
            "if let ",
            "Ok",
            "(_guard) = crate::ctrlc_guard::TERMINAL_LOCK.lock()"
        )),
        "progress must not silently skip TERMINAL_LOCK on poison (M169)"
    );

    let path_safety = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/path_safety.rs",
    ))
    .expect("path_safety.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_path_safety = production_scope(&path_safety);
    assert!(
        prod_path_safety.contains("delivery_scratch_temp_dir_or_system_temp"),
        "path_safety must use delivery_scratch_temp_dir_or_system_temp (M169)"
    );

    let process_lock = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/process_lock.rs",
    ))
    .expect("process_lock.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_process_lock = production_scope(&process_lock);
    assert!(
        prod_process_lock.contains("mutex_guard_or_recover"),
        "process_lock held-dir registry must use mutex_guard_or_recover (M169)"
    );

    let conversion_api = fs::read_to_string(root.join("crates/vid/src/conversion_api.rs"))
        .expect("conversion_api.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_conversion = production_scope(&conversion_api);
    assert!(
        prod_conversion.contains("delivery_temp_dir_in_scratch_or_err"),
        "vid HDR temp dirs must use delivery_temp_dir_in_scratch_or_err (M169)"
    );
    assert!(
        !prod_conversion.contains("tempfile::TempDir::new()"),
        "vid conversion_api must not use tempfile::TempDir::new() (M169)"
    );

    let animated = fs::read_to_string(root.join("crates/vid/src/animated_image.rs"))
        .expect("animated_image.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_animated = production_scope(&animated);
    assert!(
        prod_animated.contains("delivery_temp_dir_in_scratch_or_err"),
        "animated_image frame dirs must use delivery_temp_dir_in_scratch_or_err (M169)"
    );
}

#[test]
fn media_conversion_named_tempfile_scratch_m170() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 170),
        "contract must document M170"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("delivery_named_tempfile_in_scratch_or_err"),
        "gate must export delivery_named_tempfile_in_scratch_or_err (M170)"
    );

    let unscoped_tempfile_hits = delivery_unscoped_tempfile_offenders(&root);
    assert!(
        unscoped_tempfile_hits.is_empty(),
        "M170 discipline: production must create NamedTempFile via gate scratch SSOT:\n{}",
        unscoped_tempfile_hits.join("\n")
    );

    for (path, needle) in [
        (
            "crates/foundation/src/jxl_utils.rs",
            "delivery_named_tempfile_in_scratch_or_err",
        ),
        (
            "crates/foundation/src/vmaf_standalone.rs",
            "delivery_named_tempfile_in_scratch_or_err",
        ),
        (
            "crates/img/src/lossless_converter.rs",
            "delivery_named_tempfile_in_scratch_or_err",
        ),
        (
            "crates/vid/src/animated_image.rs",
            "delivery_named_tempfile_in_scratch_or_err",
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        assert!(
            prod.contains(needle),
            "{path} production must use {needle} (M170)"
        );
        assert!(
            !prod.contains(".tempfile()"),
            "{path} production must not call unscoped .tempfile() (M170)"
        );
    }
}

#[test]
fn media_conversion_output_adjacent_temp_m171() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 171),
        "contract must document M171"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("delivery_named_tempfile_in_parent_or_err"),
        "gate must export delivery_named_tempfile_in_parent_or_err (M171)"
    );

    let tempfile_in_hits = delivery_raw_tempfile_in_offenders(&root);
    assert!(
        tempfile_in_hits.is_empty(),
        "M171 discipline: production must not call .tempfile_in() outside gate:\n{}",
        tempfile_in_hits.join("\n")
    );

    let mfb_tmp_hits = delivery_raw_get_mfb_tmp_offenders(&root);
    assert!(
        mfb_tmp_hits.is_empty(),
        "M171 discipline: production must not call get_mfb_tmp_dir() outside \
         gate/process_lock:\n{}",
        mfb_tmp_hits.join("\n")
    );

    let hdr = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/hdr.rs"))
        .expect("hdr.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_hdr = production_scope(&hdr);
    assert!(
        prod_hdr.contains("delivery_named_tempfile_in_parent_or_err"),
        "hdr sidecar staging must use delivery_named_tempfile_in_parent_or_err (M171)"
    );
    assert!(
        !prod_hdr.contains(".tempfile_in("),
        "hdr production must not call .tempfile_in() directly (M171)"
    );

    for entry in ["crates/img/src/main.rs", "crates/vid/src/main.rs"] {
        let main_rs = fs::read_to_string(root.join(entry)).expect("main.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            main_rs.contains("init_ghost_mode"),
            "{entry} must initialize ghost mode at startup (M171)"
        );
    }
}

#[test]
fn media_conversion_path_parent_extreme_m172() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 172),
        "contract must document M172"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "path_parent_or_dot",
        "path_relative_parent_or_self",
        "delivery_create_dir_all_or_audit",
        "delivery_ensure_output_parent_or_audit",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M172)");
    }

    let path_parent_body = gate_fn_body(&gate, "path_parent_or_dot");
    assert!(
        path_parent_body.contains("delivery_metadata_batch_audit"),
        "path_parent_or_dot must audit missing parent (M172)"
    );
    let rel_parent_body = gate_fn_body(&gate, "path_relative_parent_or_self");
    assert!(
        rel_parent_body.contains("delivery_metadata_batch_audit"),
        "path_relative_parent_or_self must audit relative root (M172)"
    );

    let parent_unwrap_hits = delivery_raw_parent_unwrap_offenders(&root);
    assert!(
        parent_unwrap_hits.is_empty(),
        "M172 discipline: production must not use parent().unwrap_or* outside gate:\n{}",
        parent_unwrap_hits.join("\n")
    );
    let path_dot_hits = delivery_raw_path_dot_offenders(&root);
    assert!(
        path_dot_hits.is_empty(),
        "M172 discipline: production must not use Path::new(\".\") outside gate:\n{}",
        path_dot_hits.join("\n")
    );
    let silent_mkdir_hits = delivery_silent_create_dir_offenders(&root);
    assert!(
        silent_mkdir_hits.is_empty(),
        "M172 discipline: production must not silently discard create_dir_all:\n{}",
        silent_mkdir_hits.join("\n")
    );

    for (path, needle) in [
        ("crates/foundation/src/conversion.rs", "path_parent_or_dot"),
        ("crates/foundation/src/xmp_merger.rs", "path_parent_or_dot"),
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            "delivery_ensure_output_parent_or_audit",
        ),
        (
            "crates/foundation/src/logging.rs",
            "delivery_create_dir_all_or_audit",
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        assert!(
            prod.contains(needle),
            "{path} production must use {needle} (M172)"
        );
    }
}

#[test]
fn media_conversion_fs_strip_prefix_m173() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 173),
        "contract must document M173"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_remove_file_or_audit",
        "delivery_rename_or_audit",
        "strip_prefix_or_self",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M173)");
    }

    let silent_fs_hits = delivery_silent_fs_offenders(&root);
    assert!(
        silent_fs_hits.is_empty(),
        "M173 discipline: production must not silently discard fs remove/rename/copy:\n{}",
        silent_fs_hits.join("\n")
    );

    let strip_prefix_hits = delivery_path_strip_prefix_fallback_offenders(&root);
    assert!(
        strip_prefix_hits.is_empty(),
        "M173 discipline: Path strip_prefix fallbacks must use strip_prefix_or_self:\n{}",
        strip_prefix_hits.join("\n")
    );

    for (path, needle) in [
        (
            "crates/foundation/src/hdr.rs",
            "delivery_remove_file_or_audit",
        ),
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            "delivery_rename_or_audit",
        ),
        (
            "crates/foundation/src/common_utils.rs",
            "strip_prefix_or_self",
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        assert!(
            prod.contains(needle),
            "{path} production must use {needle} (M173)"
        );
    }
}

#[test]
fn media_conversion_path_stem_remove_m174() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 174),
        "contract must document M174"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("path_robust_move_staging_path"),
        "gate must export path_robust_move_staging_path (M174)"
    );

    let remove_unwrap_hits = delivery_remove_file_unwrap_offenders(&root);
    assert!(
        remove_unwrap_hits.is_empty(),
        "M174 discipline: remove_file must use delivery_remove_file_or_audit:\n{}",
        remove_unwrap_hits.join("\n")
    );

    for (path, needles) in [
        (
            "crates/foundation/src/io_utils.rs",
            &[
                "path_robust_move_staging_path",
                "delivery_remove_file_or_audit",
            ][..],
        ),
        (
            "crates/foundation/src/live_photo.rs",
            &[
                "path_file_stem_lossy_or_empty",
                "path_extension_lowercase_or_empty_unchecked",
            ][..],
        ),
        (
            "crates/foundation/src/loop_intent.rs",
            &["delivery_remove_file_or_audit"][..],
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{path} production must use {needle} (M174)"
            );
        }
    }
}

#[test]
fn media_conversion_remove_file_ssot_m175() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 175),
        "contract must document M175"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("path_file_stem_lossy_or_empty"),
        "gate must export path_file_stem_lossy_or_empty (M175)"
    );

    let raw_remove_hits = delivery_raw_remove_file_offenders(&root);
    assert!(
        raw_remove_hits.is_empty(),
        "M175 discipline: inline remove_file error handling must use gate helper:\n{}",
        raw_remove_hits.join("\n")
    );

    for (path, needle) in [
        (
            "crates/foundation/src/gpu_accel.rs",
            "delivery_remove_file_or_audit",
        ),
        (
            "crates/img/src/conversion_api.rs",
            "delivery_remove_file_or_audit",
        ),
        (
            "crates/vid/src/conversion_api.rs",
            "delivery_remove_file_or_audit",
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        assert!(
            prod.contains(needle),
            "{path} production must use {needle} (M175)"
        );
        assert!(
            !prod.contains("if let Err(e) = std::fs::remove_file")
                && !prod.contains("if let Err(err) = std::fs::remove_file"),
            "{path} must not inline remove_file error handling (M175)"
        );
    }

    let live = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/live_photo.rs",
    ))
    .expect("live_photo.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_live = production_scope(&live);
    assert!(
        prod_live.contains("path_file_stem_lossy_or_empty"),
        "live_photo must use non-auditing stem helper (M175)"
    );
    assert!(
        !prod_live.contains("path_file_stem_or_empty"),
        "live_photo must not audit on every probe (M175)"
    );
}

#[test]
fn media_conversion_extension_stderr_m176() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 176),
        "contract must document M176"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "path_extension_uppercase_or_unknown",
        "encode_stderr_last_line_or_unknown",
        "stderr_first_line_label",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M176)");
    }

    let extension_hits = delivery_extension_map_or_offenders(&root);
    assert!(
        extension_hits.is_empty(),
        "M176 discipline: extension fallbacks must use gate helpers:\n{}",
        extension_hits.join("\n")
    );

    let stderr_hits = delivery_stderr_line_unwrap_offenders(&root);
    assert!(
        stderr_hits.is_empty(),
        "M176 discipline: stderr lines must not use unwrap_or fallbacks:\n{}",
        stderr_hits.join("\n")
    );

    let iqd = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_quality_detector.rs",
    ))
    .expect("image_quality_detector.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_iqd = production_scope(&iqd);
    assert!(
        prod_iqd.contains("path_extension_uppercase_or_unknown"),
        "image_quality_detector must use path_extension_uppercase_or_unknown (M176)"
    );
}

#[test]
fn media_conversion_file_name_hot_path_m177() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 177),
        "contract must document M177"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "path_file_name_for_log",
        "path_file_name_or_empty",
        "path_file_name_utf8_or_none",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M177)");
    }

    let file_name_hits = delivery_file_name_map_or_offenders(&root);
    assert!(
        file_name_hits.is_empty(),
        "M177 discipline: file_name fallbacks must use gate helpers:\n{}",
        file_name_hits.join("\n")
    );

    for (path, needle) in [
        (
            "crates/foundation/src/cli_runner.rs",
            "path_file_name_for_log",
        ),
        (
            "crates/foundation/src/file_copier.rs",
            "path_file_name_utf8_or_none",
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        assert!(
            prod.contains(needle),
            "{path} production must use {needle} (M177)"
        );
        assert!(
            !prod.contains("file_name().map_or"),
            "{path} must not use file_name().map_or (M177)"
        );
    }
}

#[test]
fn media_conversion_delivery_stem_strict_m178() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 178),
        "contract must document M178"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "path_file_stem_os_or_delivery_err",
        "path_file_stem_utf8_or_delivery_err",
        "path_file_stem_or_empty",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M178)");
    }

    let stem_ok_or_hits = delivery_file_stem_ok_or_offenders(&root);
    assert!(
        stem_ok_or_hits.is_empty(),
        "M178 discipline: strict stem resolution must use gate helpers:\n{}",
        stem_ok_or_hits.join("\n")
    );

    let img = fs::read_to_string(root.join("crates/img/src/conversion_api.rs"))
        .expect("conversion_api.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_img = production_scope(&img);
    assert!(
        prod_img.contains("path_file_stem_os_or_delivery_err"),
        "img conversion_api must use path_file_stem_os_or_delivery_err for output joins (M178)"
    );
    assert!(
        !prod_img.contains("file_stem().ok_or"),
        "img conversion_api must not use file_stem().ok_or (M178)"
    );

    let or_empty_body = gate_fn_body(&gate, "path_file_stem_or_empty");
    assert!(
        or_empty_body.contains("to_string_lossy"),
        "path_file_stem_or_empty must preserve non-UTF-8 stems via lossy (M178 regression)"
    );
    assert!(
        !or_empty_body.contains("to_str()"),
        "path_file_stem_or_empty must not drop non-UTF-8 stems (M178 regression)"
    );
}

#[test]
fn media_conversion_delivery_numeric_ssot_m179() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 179),
        "contract must document M179"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_gpu_phase_best_size_optional",
        "delivery_gpu_phase_best_size_required",
        "delivery_gpu_phase_best_size_or_zero",
        "delivery_gpu_binary_search_crf_from_mid",
        "explore_precheck_nb_frames_resolved",
        "explore_quick_calibrate_mapper_or_default",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M179)");
    }

    let gpu_best_size_hits = delivery_gpu_best_size_unwrap_offenders(&root);
    assert!(
        gpu_best_size_hits.is_empty(),
        "M179 discipline: GPU phase best_size must use gate helpers:\n{}",
        gpu_best_size_hits.join("\n")
    );

    let precheck_nb_direct_hits = delivery_precheck_nb_frames_direct_offenders(&root);
    assert!(
        precheck_nb_direct_hits.is_empty(),
        "M179 discipline: precheck nb_frames must use explore_precheck_nb_frames_resolved:\n{}",
        precheck_nb_direct_hits.join("\n")
    );

    for (path, needles) in [
        (
            "crates/foundation/src/gpu_accel.rs",
            &[
                "delivery_gpu_phase_best_size_required",
                "delivery_gpu_binary_search_crf_from_mid",
            ][..],
        ),
        (
            "crates/foundation/src/video_explorer/precheck.rs",
            &["explore_precheck_nb_frames_resolved"][..],
        ),
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            &["dynamic_mapping::quick_calibrate"][..],
        ),
        (
            "crates/foundation/src/gpu_accel.rs",
            &["explore_encode_size_improvement_pct_optional"][..],
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        for needle in needles {
            assert!(
                prod.contains(needle),
                "{path} production must use {needle} (M179)"
            );
        }
        if path.ends_with("gpu_coarse_search.rs") {
            assert!(
                !prod.contains("explore_quick_calibrate_mapper_or_default"),
                "{path} must propagate quick_calibrate Err, not forged size-only mapper (M179)"
            );
        }
    }

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/gpu_accel.rs",
    ))
    .expect("gpu_accel.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_gpu = production_scope(&gpu);
    assert!(
        !prod_gpu.contains("best_size.unwrap_or_else"),
        "gpu_accel must not inline best_size unwrap_or_else (M179)"
    );
}

#[test]
fn media_conversion_probe_decode_jpeg_slice_m180() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 180),
        "contract must document M180"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "probe_image_decode_failure_or_unknown",
        "probe_rational_from_f64_or_zero",
        "probe_jpeg_buffer_slice",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M180)");
    }

    let read_error_hits = delivery_read_error_unwrap_offenders(&root);
    assert!(
        read_error_hits.is_empty(),
        "M180 discipline: read_error must use gate helper:\n{}",
        read_error_hits.join("\n")
    );

    let jpeg_slice_hits = delivery_jpeg_inline_slice_unwrap_offenders(&root);
    assert!(
        jpeg_slice_hits.is_empty(),
        "M180 discipline: JPEG slices must use probe_jpeg_buffer_slice:\n{}",
        jpeg_slice_hits.join("\n")
    );

    let detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_detection = production_scope(&detection);
    assert!(
        prod_detection.contains("probe_image_decode_failure_or_unknown"),
        "image_detection must use probe_image_decode_failure_or_unknown (M180)"
    );
    assert!(
        prod_detection.contains("probe_rational_from_f64_optional"),
        "image_detection must use probe_rational_from_f64_optional (M180)"
    );
    assert!(
        !prod_detection.contains("read_error.unwrap_or_else"),
        "image_detection must not inline read_error unwrap_or_else (M180)"
    );

    let jpeg = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_jpeg_analysis.rs",
    ))
    .expect("image_jpeg_analysis.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_jpeg = production_scope(&jpeg);
    assert!(
        prod_jpeg.matches("probe_jpeg_buffer_slice").count() >= 4,
        "image_jpeg_analysis must route JPEG slice fallbacks via probe_jpeg_buffer_slice (M180)"
    );
}

#[test]
fn media_conversion_runtime_checkpoint_fields_m181() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 181),
        "contract must document M181"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_exiftool_field_or_empty",
        "delivery_checkpoint_lock_start_time_or_now",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M181)");
    }

    let checkpoint_hits = delivery_checkpoint_start_unwrap_offenders(&root);
    assert!(
        checkpoint_hits.is_empty(),
        "M181 discipline: checkpoint start time must use gate helper:\n{}",
        checkpoint_hits.join("\n")
    );

    let date_hits = delivery_date_field_unwrap_offenders(&root);
    assert!(
        date_hits.is_empty(),
        "M181 discipline: date fields must use gate helper:\n{}",
        date_hits.join("\n")
    );

    for (path, needle) in [
        (
            "crates/foundation/src/date_analysis.rs",
            "delivery_exiftool_field_or_empty",
        ),
        (
            "crates/foundation/src/checkpoint.rs",
            "delivery_checkpoint_lock_start_time_or_now",
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&src);
        assert!(
            prod.contains(needle),
            "{path} production must use {needle} (M181)"
        );
    }
}

#[test]
fn media_conversion_quality_jpeg_numeric_m182() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 182),
        "contract must document M182"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "quality_content_type_for_crf_or_unknown",
        "delivery_jpeg_qt_cell_u16_or_one",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M182)");
    }

    let content_type_hits = delivery_content_type_unwrap_offenders(&root);
    assert!(
        content_type_hits.is_empty(),
        "M182 discipline: content_type must use gate helper:\n{}",
        content_type_hits.join("\n")
    );

    let jpeg_qt_hits = delivery_jpeg_qt_inline_audit_offenders(&root);
    assert!(
        jpeg_qt_hits.is_empty(),
        "M182 discipline: JPEG QT must use gate helper:\n{}",
        jpeg_qt_hits.join("\n")
    );

    let qm = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_matcher.rs",
    ))
    .expect("quality_matcher.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_qm = production_scope(&qm);
    assert!(
        prod_qm.contains("quality_content_type_for_crf_or_unknown"),
        "quality_matcher must use quality_content_type_for_crf_or_unknown (M182)"
    );
    assert!(
        !prod_qm.contains("content_type.unwrap_or_else"),
        "quality_matcher must not inline content_type unwrap_or_else (M182)"
    );

    let jpeg = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_jpeg_analysis.rs",
    ))
    .expect("image_jpeg_analysis.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_jpeg = production_scope(&jpeg);
    assert!(
        prod_jpeg.contains("delivery_jpeg_qt_cell_u16_or_one"),
        "image_jpeg_analysis must use delivery_jpeg_qt_cell_u16_or_one (M182)"
    );
}

#[test]
fn media_conversion_tool_training_crf_m183() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 183),
        "contract must document M183"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_training_source_path_or_input",
        "delivery_tool_path_or_bare_name",
        "probe_video_crf_from_params_or_estimate",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M183)");
    }

    let common = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/common_utils.rs",
    ))
    .expect("common_utils.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_common = production_scope(&common);
    assert!(
        prod_common.contains("delivery_training_source_path_or_input")
            && prod_common.contains("delivery_tool_path_or_bare_name"),
        "common_utils must delegate training/tool paths to gate (M183)"
    );

    let vqd = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_quality_detector.rs",
    ))
    .expect("video_quality_detector.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&vqd).contains("probe_video_crf_from_params_or_estimate"),
        "video_quality_detector must use probe_video_crf_from_params_or_estimate (M183)"
    );
}

#[test]
fn media_conversion_runtime_infra_m184() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 184),
        "contract must document M184"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_path_env_or_empty",
        "delivery_system_memory_mb_or_zero",
        "delivery_rsync_executable_or_default",
        "delivery_runtime_permille_u32_or_max",
        "delivery_spinner_frame_index_or_zero",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M184)");
    }

    for (path, needle) in [
        (
            "crates/foundation/src/process_lock.rs",
            "delivery_path_env_or_empty",
        ),
        (
            "crates/foundation/src/x265_params.rs",
            "delivery_system_memory_mb_or_zero",
        ),
        (
            "crates/foundation/src/thread_manager.rs",
            "delivery_rsync_executable_or_default",
        ),
        (
            "crates/foundation/src/modern_ui.rs",
            "delivery_runtime_permille_u32_or_max",
        ),
    ] {
        let src =
            fs::read_to_string(join_legacy_aware(&root, path)).expect("source must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            production_scope(&src).contains(needle),
            "{path} must use {needle} (M184)"
        );
    }
}

#[test]
fn media_conversion_ffprobe_nb_frames_sort_m185() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 185),
        "contract must document M185"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("probe_ffprobe_stream_nb_frames_sort_or_zero"),
        "gate must export probe_ffprobe_stream_nb_frames_sort_or_zero (M185)"
    );

    let ffprobe = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
        .expect("ffprobe.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&ffprobe).contains("probe_ffprobe_stream_nb_frames_sort_or_zero"),
        "ffprobe must use probe_ffprobe_stream_nb_frames_sort_or_zero (M185)"
    );
}

#[test]
fn media_conversion_batch_analyzer_probe_m186() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 186),
        "contract must document M186"
    );

    let batch_hits = delivery_batch_perceived_speed_unwrap_offenders(&root);
    assert!(
        batch_hits.is_empty(),
        "M186 discipline: perceived-speed batch caching must not use unwrap_or_else:\n{}",
        batch_hits.join("\n")
    );

    let analyzer_hits = delivery_image_analyzer_probe_unwrap_offenders(&root);
    assert!(
        analyzer_hits.is_empty(),
        "M186 discipline: image_analyzer probe paths must not inline unwrap_or_else fallbacks:\n{}",
        analyzer_hits.join("\n")
    );
}

#[test]
fn media_conversion_db_percentile_metadata_m187() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 187),
        "contract must document M187"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in ["delivery_db_usize_or_zero", "delivery_db_json_or_default"] {
        assert!(gate.contains(sym), "gate must export {sym} (M187)");
    }

    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_db = production_scope(&db);
    assert!(
        prod_db.contains("delivery_db_usize_or_zero"),
        "database percentile path must use delivery_db_usize_or_zero (M187)"
    );
    assert!(
        prod_db.contains("delivery_db_json_or_default"),
        "database loop metadata parse must use delivery_db_json_or_default (M187)"
    );

    let db_hits = delivery_db_inline_unwrap_offenders(&root);
    assert!(
        db_hits.is_empty(),
        "M187 discipline: database percentile/metadata fallbacks must use gate helpers:\n{}",
        db_hits.join("\n")
    );
}

#[test]
fn media_conversion_runtime_ui_stream_m188() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 188),
        "contract must document M188"
    );

    let hits = delivery_runtime_ui_stream_uoe_offenders(&root);
    assert!(
        hits.is_empty(),
        "M188 discipline: runtime/ui/stream size must not use non-panic unwrap_or_else:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_explore_jxl_margin_m189() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 189),
        "contract must document M189"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_gif_frame_count_optional",
        "explore_webp_frame_count_optional",
        "explore_gif_frame_count_or_zero",
        "explore_webp_frame_count_or_zero",
        "delivery_jxl_margin_u64_or_one",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M189)");
    }

    let stream = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/stream_analysis.rs",
    ))
    .expect("stream_analysis.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_stream = production_scope(&stream);
    assert!(prod_stream.contains("explore_gif_frame_count_optional"));
    assert!(prod_stream.contains("explore_webp_frame_count_optional"));

    let jxl = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/jxl_explorer.rs",
    ))
    .expect("jxl_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_jxl = production_scope(&jxl);
    assert!(prod_jxl.contains("delivery_jxl_margin_u64_or_one"));

    let hits = delivery_explore_jxl_uoe_offenders(&root);
    assert!(
        hits.is_empty(),
        "M189 discipline: explore frame-count / jxl margin must use gate helpers:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_metrics_metadata_margin_m190() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 190),
        "contract must document M190"
    );

    let hits = delivery_metrics_margin_uoe_offenders(&root);
    assert!(
        hits.is_empty(),
        "M190 discipline: image metrics / metadata margin must not use non-panic \
         unwrap_or_else:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_runtime_explore_hardening_m191() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 191),
        "contract must document M191"
    );

    let hits = delivery_runtime_explore_uoe_offenders(&root);
    assert!(
        hits.is_empty(),
        "M191 discipline: runtime/explore critical paths must not use non-panic \
         unwrap_or_else:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_video_detection_or_else_m206() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 206),
        "contract must document M206"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_header_bytes_png_or_apng",
        "probe_webp_dimensions_from_bytes_or_path",
        "probe_bitstream_media_info_or_webp_canvas",
        "probe_ffprobe_bit_rate_or_derived_from_size",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M206 helper {helper}"
        );
    }

    let vd = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&vd).contains("try_animated_header_preflight"),
        "video_detection must use try_animated_header_preflight"
    );

    let hits = delivery_video_detection_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M206 discipline: video_detection must not use inline or_else fallbacks:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_stream_analysis_precheck_or_else_m207() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 207),
        "contract must document M207"
    );

    let hits = delivery_stream_analysis_precheck_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M207 discipline: stream_analysis/precheck must not use targeted inline or_else:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_image_heic_detection_or_else_m208() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 208),
        "contract must document M208"
    );

    let hits = delivery_image_heic_detection_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M208 discipline: image_heic_analysis/image_detection must not use targeted inline \
         or_else:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_logging_system_memory_or_else_m209() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 209),
        "contract must document M209"
    );

    let hits = delivery_logging_system_memory_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M209 discipline: logging/system_memory must not use inline or_else fallbacks:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_remaining_or_else_m210() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 210),
        "contract must document M210"
    );

    let hits = delivery_remaining_or_else_closures_m210_offenders(&root);
    assert!(
        hits.is_empty(),
        "M210 discipline: production code must not contain inline .or_else(|| ... )\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_quality_timing_or_else_m205() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 205),
        "contract must document M205"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_animated_frame_count_u32",
        "probe_animated_duration_secs",
        "probe_animated_fps",
        "probe_animated_average_frame_delay_ms",
        "probe_video_quality_duration_secs",
        "probe_video_quality_fps",
        "probe_video_quality_frame_count",
        "probe_video_quality_bitrate_mbps",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M205 helper {helper}"
        );
    }

    let hits = delivery_quality_timing_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M205 discipline: animated/video quality timing must not use inline or_else:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_ffprobe_hdr_or_else_m204() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 204),
        "contract must document M204"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_ffprobe_hdr_luma_raw_u64",
        "probe_ffprobe_format_loop_count_tag",
        "probe_ffprobe_hdr_side_data_chromaticity_u64",
        "probe_ffprobe_hdr_side_data_luminance_u64",
        "probe_ffprobe_cll_max_content_u64",
        "probe_ffprobe_cll_max_average_u64",
        "probe_ffprobe_stream_bit_depth_u8_from_fields",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M204 helper {helper}"
        );
    }

    let ffprobe = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
        .expect("ffprobe.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !production_scope(&ffprobe).contains(".or_else("),
        "ffprobe production scope must not contain inline or_else after M204"
    );

    let hits = delivery_ffprobe_hdr_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M204 discipline: ffprobe/ffprobe_json must not use inline HDR or_else fallbacks:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_ffprobe_loop_or_else_m203() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 203),
        "contract must document M203"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_ffprobe_bit_depth_string_fields",
        "probe_ffprobe_fps_avg_or_r_frame_rate",
        "probe_ffprobe_stream_u64_field",
        "probe_ffprobe_zero_dimension_recovery",
        "probe_ffprobe_encoder_settings_from_tags",
        "probe_ffprobe_hdr_coord_raw_u64",
        "loop_duration_p50_or_capped_p75",
        "loop_encoder_software_label",
        "loop_inference_unit_probability_or_tree_fallback",
        "loop_inference_resolution_path_or_tree",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M203 helper {helper}"
        );
    }

    let loop_src = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&loop_src).contains("loop_meta_duration_tier_or_from_secs"),
        "LoopMeta::tier must use loop_meta_duration_tier_or_from_secs"
    );

    let hits = delivery_ffprobe_loop_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M203 discipline: ffprobe/loop_intent must not use inline or_else fallbacks:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_conversion_cli_or_else_m202() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 202),
        "contract must document M202"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "conversion_fallback_output_path_display",
        "probe_identify_output_magick_then_system",
        "delivery_cli_base_dir_or_input_when_output",
        "delivery_pipeline_pixel_count_u64_or_none",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M202 helper {helper}"
        );
    }

    let conv = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/conversion.rs",
    ))
    .expect("conversion.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_conv = production_scope(&conv);
    assert!(
        prod_conv.contains("probe_identify_output_magick_then_system"),
        "media_info_without_ffprobe must use probe_identify_output_magick_then_system"
    );

    let hits = delivery_conversion_cli_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M202 discipline: conversion/batch/vid CLI must not use inline or_else fallbacks:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_database_or_else_m201() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 201),
        "contract must document M201"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "delivery_db_diag_cell_or_unknown",
        "delivery_db_duration_p90_or_feature_stats",
        "delivery_db_loop_aspect_ratio_or_derived",
        "delivery_db_knn_neighbor_count_i32",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M201 helper {helper}"
        );
    }

    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_db = production_scope(&db);
    assert!(
        prod_db.contains("loop_sample_row_or_reprobe_from_source"),
        "database loop training row recovery must use loop_sample_row_or_reprobe_from_source"
    );

    let hits = delivery_database_or_else_offenders(&root);
    assert!(
        hits.is_empty(),
        "M201 discipline: database/diagnostics must not use inline or_else/map_or fallbacks:\n{}",
        hits.join("\n")
    );
}

#[test]
fn database_inference_log_hash_failure_skips_insert_contract() {
    let root = workspace_root();
    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_db = production_scope(&db);
    let start = prod_db
        .find("pub fn log_inference_record")
        .expect("log_inference_record must exist"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let end = prod_db[start..]
        .find("\n// ── Level 1")
        .map_or(prod_db.len(), |offset| start + offset);
    let body = &prod_db[start..end];

    assert!(
        prod_db.contains("fn inference_log_file_hash_or_skip"),
        "database inference logging must centralize source-path BLAKE3 failure policy"
    );
    assert!(
        body.contains("let file_hash = match inference_log_file_hash_or_skip(path)")
            && body.contains("None => return,"),
        "source-path BLAKE3 failure must audit and skip inference_log insert, not write NULL \
         file_hash"
    );
    assert!(
        !body.contains(concat!(".", "ok", "()\n    });")),
        "log_inference_record must not convert source-path hash errors into None via dot-ok"
    );
    let silent_doc = ["Fails", "silently"].join(" ");
    assert!(
        !body.contains(&silent_doc),
        "database inference logging docs must not describe unlogged failure as acceptable"
    );
}

#[test]
fn media_conversion_database_training_unwrap_or_m200() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 200),
        "contract must document M200"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "delivery_training_pg_connstr_or_default",
        "delivery_subprocess_log_tail_or_empty",
        "delivery_path_basename_for_log_or_unknown",
        "delivery_system_avail_bytes_from_u128",
        "delivery_db_u64_to_usize_or_zero_with_notice",
        "loop_collection_secs_or_baseline_policy",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M200 helper {helper}"
        );
    }

    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_db = production_scope(&db);
    assert!(
        prod_db.contains("loop_collection_secs_or_baseline_policy"),
        "database KnnDistributionProfile::default must use loop_collection_secs_or_baseline_policy"
    );

    let hits = delivery_database_training_unwrap_or_offenders(&root);
    assert!(
        hits.is_empty(),
        "M200 discipline: database/training unwrap_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_runtime_unwrap_or_m199() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 199),
        "contract must document M199"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("delivery_quality_model_python_command_or_default"),
        "gate must export M199 helper delivery_quality_model_python_command_or_default"
    );

    let qrm = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_regression_model.rs",
    ))
    .expect("quality_regression_model.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&qrm).contains("delivery_quality_model_python_command_or_default"),
        "quality_regression_model must use gate helper for python command"
    );

    let hits = delivery_runtime_unwrap_or_offenders(&root);
    assert!(
        hits.is_empty(),
        "M199 discipline: runtime unwrap_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_api_explore_ffi_mapor_m198() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 198),
        "contract must document M198"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("probe_ffmpeg_stderr_tail_line_or_unknown"),
        "gate must export M198 helper probe_ffmpeg_stderr_tail_line_or_unknown"
    );

    let ffmpeg = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/ffmpeg_process.rs",
    ))
    .expect("ffmpeg_process.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&ffmpeg).contains("probe_ffmpeg_stderr_tail_line_or_unknown"),
        "ffmpeg_process must use gate helper probe_ffmpeg_stderr_tail_line_or_unknown"
    );

    let hits = delivery_api_explore_ffi_mapor_offenders(&root);
    assert!(
        hits.is_empty(),
        "M198 discipline: API/explore/FFI map_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_builders_progress_copier_mapor_m197() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 197),
        "contract must document M197"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("delivery_imagemagick_cli_path_or_default"),
        "gate must export M197 helper delivery_imagemagick_cli_path_or_default"
    );

    let hits = delivery_builders_progress_copier_mapor_offenders(&root);
    assert!(
        hits.is_empty(),
        "M197 discipline: builders/progress/copier map_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_io_gpu_vector_db_mapor_m196() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 196),
        "contract must document M196"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_io_fixed_slice_or_none",
        "delivery_gpu_probe_failure_reason_or_default",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M196 helper {helper}"
        );
    }

    let hits = delivery_io_gpu_vector_db_mapor_offenders(&root);
    assert!(
        hits.is_empty(),
        "M196 discipline: IO/GPU/vector/quality-db map_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_analyzer_loop_hdr_mapor_m195() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 195),
        "contract must document M195"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_detection_canvas_or_zero",
        "probe_hdr_heic_input_label",
        "probe_hdr_sidecar_extension_or_bin",
        "x265_params_base_owned_or_empty",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M195 helper {helper}"
        );
    }

    let hits = delivery_analyzer_loop_hdr_mapor_offenders(&root);
    assert!(
        hits.is_empty(),
        "M195 discipline: analyzer/loop/HDR map_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_batch_db_conversion_mapor_m194() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 194),
        "contract must document M194"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "delivery_path_modified_unix_secs_or_zero",
        "delivery_batch_relative_depth_or_zero",
        "delivery_video_frame_count_or_estimate",
        "delivery_video_sort_work_or_none",
        "conversion_size_increase_diff_tag",
        "conversion_message_with_quality_label",
        "conversion_result_core_msg",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M194 helper {helper}"
        );
    }

    let hits = delivery_batch_db_conversion_mapor_offenders(&root);
    assert!(
        hits.is_empty(),
        "M194 discipline: batch/DB/conversion map_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_probe_gpu_mapor_m193() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 193),
        "contract must document M193"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_path_file_name_for_log",
        "explore_fusion_ssim_floor",
        "explore_quality_check_failed_line",
        "ui_explore_ssim_ref_or_none",
        "explore_search_anchor_crf_or_baseline",
        "probe_classifier_content_name_or_unknown",
        "probe_palette_color_diversity_ratio",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M193 helper {helper}"
        );
    }

    let hits = delivery_probe_gpu_mapor_offenders(&root);
    assert!(
        hits.is_empty(),
        "M193 discipline: probe/GPU coarse map_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_gpu_explore_mapor_m192() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 192),
        "contract must document M192"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for helper in [
        "probe_path_can_be_animated_or_label",
        "explore_gpu_search_summary_from_best_crf",
        "explore_perceptual_gate_failure_reason_or_default",
        "ui_ssim_colon_label_or_unknown",
        "explore_ms_ssim_skip_when_duration_unknown",
        "delivery_cli_extension_display_or_none",
        "delivery_cli_output_size_label_or_unknown",
        "img_static_lossless_quality_arg_or_default",
    ] {
        assert!(
            gate.contains(helper),
            "gate must export M192 helper {helper}"
        );
    }

    let hits = delivery_gpu_explore_mapor_offenders(&root);
    assert!(
        hits.is_empty(),
        "M192 discipline: GPU/explore/pipeline map_or fallbacks must use gate SSOT:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_contract_m1_m185_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 185),
        "contract must document M185"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 185, 154);
}

#[test]
fn media_conversion_contract_m1_m187_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 187),
        "contract must document M187"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 187, 157);
}

#[test]
fn media_conversion_contract_m1_m188_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 188),
        "contract must document M188"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 188, 158);
}

#[test]
fn media_conversion_contract_m1_m189_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 189),
        "contract must document M189"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 189, 160);
}

#[test]
fn media_conversion_contract_m1_m190_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 190),
        "contract must document M190"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 190, 162);
}

#[test]
fn media_conversion_contract_m1_m191_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 191),
        "contract must document M191"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 191, 164);
}

#[test]
fn media_conversion_contract_m1_m192_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 192),
        "contract must document M192"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 192, 166);
}

#[test]
fn media_conversion_contract_m1_m193_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 193),
        "contract must document M193"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 193, 168);
}

#[test]
fn media_conversion_contract_m1_m194_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 194),
        "contract must document M194"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 194, 170);
}

#[test]
fn media_conversion_contract_m1_m200_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 200),
        "contract must document M200"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 200, 177);
}

#[test]
fn media_conversion_contract_m1_m201_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 201),
        "contract must document M201"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 201, 178);
}

#[test]
fn media_conversion_contract_m1_m202_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 202),
        "contract must document M202"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 202, 179);
}

#[test]
fn media_conversion_contract_m1_m203_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 203),
        "contract must document M203"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 203, 180);
}

#[test]
fn media_conversion_contract_m1_m204_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 204),
        "contract must document M204"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 204, 181);
}

#[test]
fn media_conversion_contract_m1_m205_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 205),
        "contract must document M205"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 205, 182);
}

#[test]
fn media_conversion_contract_m1_m206_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 206),
        "contract must document M206"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 206, 183);
}

#[test]
fn media_conversion_contract_m1_m207_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 207),
        "contract must document M207"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 207, 184);
}

#[test]
fn media_conversion_contract_m1_m208_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 208),
        "contract must document M208"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 208, 184);
}

#[test]
fn media_conversion_contract_m1_m209_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 209),
        "contract must document M209"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 209, 185);
}

#[test]
fn media_conversion_contract_m1_m210_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 210),
        "contract must document M210"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 210, 186);
}

#[test]
fn media_conversion_contract_m1_m211_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 211),
        "contract must document M211"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 211, 187);
}

#[test]
fn media_conversion_contract_m1_m213_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 213),
        "contract must document M213"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 213, 189);
}

#[test]
fn media_conversion_contract_m1_m214_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 214),
        "contract must document M214"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 214, 190);
}

#[test]
fn media_conversion_contract_m1_m199_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 199),
        "contract must document M199"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 199, 176);
}

#[test]
fn media_conversion_contract_m1_m198_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 198),
        "contract must document M198"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 198, 175);
}

#[test]
fn media_conversion_contract_m1_m197_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 197),
        "contract must document M197"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 197, 174);
}

#[test]
fn media_conversion_contract_m1_m196_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 196),
        "contract must document M196"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 196, 173);
}

#[test]
fn media_conversion_contract_m1_m195_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 195),
        "contract must document M195"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 195, 172);
}

#[test]
fn media_conversion_contract_m1_m186_design_complete() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 186),
        "contract must document M186"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert_media_conversion_contract_registry(&contract, &tests, 1, 186, 155);
}

#[test]
fn media_conversion_gpu_accel_numeric_ssot_m108() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 108),
        "contract must document M108"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "gpu_quality_compression_ratio_or_neutral",
        "explore_gpu_quality_ceiling_crf_or_last_tested",
        "explore_encode_size_improvement_pct",
        "gpu_compression_potential_adjustment_or_zero",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M108)");
    }
    let ratio_body = gate_fn_body(&gate, "gpu_quality_compression_ratio_or_neutral");
    assert!(
        ratio_body.contains("delivery_gpu_batch_audit"),
        "gpu_quality_compression_ratio_or_neutral must audit via delivery_gpu_batch_audit (M108)"
    );
    let ceiling_body = gate_fn_body(&gate, "explore_gpu_quality_ceiling_crf_or_last_tested");
    assert!(
        ceiling_body.contains("delivery_gpu_batch_audit"),
        "explore_gpu_quality_ceiling_crf_or_last_tested must audit via delivery_gpu_batch_audit \
         (M108)"
    );

    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/gpu_accel.rs",
    ))
    .expect("gpu_accel.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&gpu);
    for needle in [
        "gpu_quality_compression_ratio_or_neutral",
        "explore_gpu_boundary_crf_from_search_optional",
        "explore_encode_size_improvement_pct_optional",
        "gpu_compression_potential_adjustment_optional",
    ] {
        assert!(
            prod.contains(needle),
            "gpu_accel production must use gate helper {needle} (M108)"
        );
    }
    for ban in [
        "input_size is 0 during quality score calculation",
        "quality ceiling CRF missing; using last tested",
        "state.best_size.map_or(100.0",
        "explore_encode_size_improvement_pct(",
        "compression_potential.map_or(0.0",
    ] {
        assert!(
            !prod.contains(ban),
            "gpu_accel must not use silent fallback {ban} (M108)"
        );
    }
}

#[test]
fn media_conversion_safety_cwd_m107() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 107),
        "contract must document M107"
    );

    let safety = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/safety.rs"))
        .expect("safety.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&safety);
    assert!(
        !prod.contains("PathBuf::from(\"/\")"),
        "safety path normalization must not silently fall back to / (M107)"
    );
    assert!(
        !prod.contains("current_dir()\n            .unwrap_or_else"),
        "safety must not use inline current_dir unwrap_or_else (M107)"
    );
    assert!(
        prod.contains("delivery_safety_relative_base_or_root"),
        "safety must resolve relative paths via delivery_safety_relative_base_or_root (M107)"
    );
    assert!(
        !prod.contains("delivery_run_logs_dir_or_dot"),
        "safety must not use run-log cwd helper (/. semantics differ) (M107)"
    );
}

#[test]
fn media_conversion_canonicalize_ssot_m106() {
    const FORBIDDEN: &[&str] = &[
        ".canonicalize().unwrap_or_else(|_|",
        ".unwrap_or_else(|_| path.to_path_buf())",
    ];
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 106),
        "contract must document M106"
    );
    let hits = offending_lines(&root, &production_rust_files(&root), FORBIDDEN);
    assert!(
        hits.is_empty(),
        "production img/vid/foundation must not silently canonicalize paths (M106):\n{}",
        hits.join("\n")
    );

    let common = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/common_utils.rs",
    ))
    .expect("common_utils.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&common);
    assert!(
        prod.contains("canonicalize_for_tool_input"),
        "common_utils training map keys must use canonicalize_for_tool_input (M106)"
    );
}

#[test]
fn media_conversion_fallback_emitter_crate_private_m101() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 101),
        "contract must document M101"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("pub(crate) fn delivery_fallback_audit"),
        "delivery_fallback_audit must be crate-private (M101)"
    );
    assert!(
        gate.contains("pub(crate) fn delivery_path_audit")
            && gate.contains("pub(crate) fn delivery_batch_audit"),
        "path/batch emitters remain crate-private (M100/M101)"
    );
}

#[test]
fn media_conversion_dynamic_calibration_audit_m81() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 81),
        "contract must document M81"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_calibration_degraded_audit",
        "explore_calibration_duration_optional",
        "explore_calibration_probe_size_optional",
        "explore_calibration_probe_size_or_zero",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M81)");
        if sym == "explore_calibration_degraded_audit" {
            let body = gate_fn_body(&gate, sym);
            assert!(
                body.contains("explore_precheck_batch_audit"),
                "{sym} must delegate to precheck batch (M81/M97)"
            );
        }
    }

    let mapping_path = join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/dynamic_mapping.rs",
    );
    let mapping = fs::read_to_string(&mapping_path).expect("dynamic_mapping.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&mapping);
    for needle in [
        "explore_calibration_degraded_audit",
        "explore_calibration_duration_optional",
        "explore_calibration_probe_size_optional",
    ] {
        assert!(
            prod.contains(needle),
            "dynamic_mapping must use gate helper {needle} (M81)"
        );
    }

    let hits = offending_lines(&root, &[mapping_path], MC_FORBIDDEN_M81_DYNAMIC_MAPPING);
    assert!(
        hits.is_empty(),
        "M81 dynamic_mapping must not use always-on precheck audits or inline duration \
         fallback:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_quality_heuristic_audit_m85() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("quality_heuristic_fallback_audit"),
        "gate must export quality_heuristic_fallback_audit (M85)"
    );
    let body = gate_fn_body(&gate, "quality_heuristic_fallback_audit");
    assert!(
        body.contains("probe_quality_batch_audit"),
        "quality_heuristic_fallback_audit must delegate to strict-gated probe quality audit \
         (M85/M96)"
    );
    let qm = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_matcher.rs",
    ))
    .expect("quality_matcher.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&qm);
    assert!(
        prod.contains("quality_content_type_for_crf_or_unknown"),
        "quality_matcher must route missing content_type heuristics through gate (M85/M104/M182)"
    );
    let hits = offending_lines(
        &root,
        &[join_legacy_aware(
            &root,
            "crates/foundation/src/quality_matcher.rs",
        )],
        MC_FORBIDDEN_M85_QUALITY,
    );
    assert!(
        hits.is_empty(),
        "M85 quality_matcher must not use always-on delivery_fallback for content_type:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_explore_size_target_reason_m86() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(gate.contains("explore_size_target_failure_reason_or_default"));
    let explorer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&explorer).contains("explore_size_target_failure_reason_or_default"),
        "video_explorer must use gate size-target reason helper (M86)"
    );
    assert!(
        !production_scope(&explorer).contains(
            "delivery_fallback_audit(\n                    \"explore_size_target_reason\""
        ),
        "video_explorer must not always-on audit empty size-target reason (M86)"
    );
}

#[test]
fn media_conversion_delivery_path_layout_m87() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let layout = gate_fn_body(&gate, "delivery_path_layout_fallback_audit");
    assert!(
        layout.contains("delivery_strict_batch_audit"),
        "delivery_path_layout_fallback_audit must delegate to strict SSOT (M87/M97)"
    );
    let conversion = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/conversion.rs",
    ))
    .expect("conversion.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&conversion);
    for needle in [
        "output_stem_for_delivery",
        "path_parent_or_dot",
        "strip_prefix_or_self",
        "delivery_path_layout_fallback_audit",
    ] {
        assert!(
            prod.contains(needle),
            "conversion.rs must use {needle} (M87)"
        );
    }
    let hits = offending_lines(
        &root,
        &[join_legacy_aware(
            &root,
            "crates/foundation/src/conversion.rs",
        )],
        MC_FORBIDDEN_M87_CONVERSION,
    );
    assert!(
        hits.is_empty(),
        "M87 conversion must not use inline path fallbacks:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_probe_detection_recovery_m88() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let body = gate_fn_body(&gate, "probe_detection_recovery_audit");
    assert!(
        body.contains("probe_layer_batch_audit"),
        "probe_detection_recovery_audit must delegate to probe_layer_batch_audit (M88/M97)"
    );
    assert!(
        !body.contains("strict_media_conversion_delivery_enabled"),
        "probe_detection_recovery_audit must not double-gate strict (M97)"
    );
    let vd = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_detection.rs",
    ))
    .expect("video_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&vd).contains("probe_detection_recovery_audit"),
        "video_detection animated promote must use probe_detection_recovery_audit (M88)"
    );
    assert!(
        !production_scope(&vd).contains(
            "delivery_fallback_audit(\n            \"animated_container_ffprobe_recovery\""
        ),
        "video_detection must not always-on audit animated promote (M88)"
    );
}

#[test]
fn media_conversion_video_explorer_audit_m89() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 89),
        "contract must document M89"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_gpu_coarse_degraded_audit",
        "explore_delivery_explore_outcome_audit",
        "explore_gpu_coarse_explore_audit",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M89)");
    }
    let boundary = gate_fn_body(&gate, "explore_boundary_crf_or_refined");
    assert!(
        !boundary.contains("delivery_fallback_audit"),
        "explore_boundary_crf_or_refined must not always-on audit missing refine (M89)"
    );
    let progress_ssim = gate_fn_body(&gate, "explore_progress_ssim_token");
    assert!(
        progress_ssim.contains("ui_ssim_inline_or_empty"),
        "explore_progress_ssim_token must delegate to ui_ssim_inline_or_empty (M113/M89)"
    );
    for fn_name in [
        "explore_quality_gate_audit",
        "explore_quality_skip_summary_audit",
        "explore_gpu_coarse_degraded_audit",
        "explore_crf_cache_key_rejected_audit",
        "explore_ssim_measurement_fallback_audit",
    ] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            body.contains("delivery_strict_path_audit")
                || body.contains("delivery_strict_batch_audit")
                || body.contains("strict_media_conversion_delivery_enabled"),
            "{fn_name} must be strict-gated or delegate to delivery_strict_* (M89/M96)"
        );
    }
    let fail_reason = gate_fn_body(&gate, "explore_quality_fail_reason");
    assert!(
        fail_reason.contains("explore_delivery_explore_outcome_audit"),
        "explore_quality_fail_reason must use strict explore outcome audit (M89)"
    );

    let explorer_path = join_legacy_aware(&root, "crates/foundation/src/video_explorer.rs");
    let explorer = fs::read_to_string(&explorer_path).expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_explorer = production_scope(&explorer);
    assert!(
        prod_explorer.contains("explore_gpu_coarse_degraded_audit"),
        "video_explorer production must use explore_gpu_coarse_degraded_audit (M89)"
    );
    let hits = offending_lines(&root, &[explorer_path], MC_FORBIDDEN_M89_VIDEO_EXPLORER);
    assert!(
        hits.is_empty(),
        "video_explorer production must not call always-on explore_gpu_coarse_audit (M89):\n{}",
        hits.join("\n")
    );
    assert!(
        !prod_explorer.contains("delivery_fallback_audit"),
        "video_explorer production must not call delivery_fallback_audit (M89)"
    );
}

#[test]
fn media_conversion_delivery_api_audit_m90() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 90),
        "contract must document M90"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "delivery_api_path_fallback_audit",
        "delivery_api_batch_fallback_audit",
        "delivery_jxl_path_fallback_audit",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M90)");
        let body = gate_fn_body(&gate, sym);
        assert!(
            body.contains("strict_media_conversion_delivery_enabled")
                || body.contains("delivery_strict_path_audit")
                || body.contains("delivery_strict_batch_audit"),
            "{sym} must be strict-gated or delegate to delivery_strict_* (M90/M96)"
        );
    }

    let paths = [
        root.join("crates/vid/src/conversion_api.rs"),
        root.join("crates/img/src/conversion_api.rs"),
        root.join("crates/img/src/main.rs"),
        root.join("crates/img/src/lossless_converter.rs"),
    ];
    for path in &paths {
        let content =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains("media_conversion_gate::"),
            "{} must route delivery fallbacks through gate (M90)",
            path.display()
        );
        let hits = offending_lines(
            &root,
            std::slice::from_ref(path),
            MC_FORBIDDEN_M90_DELIVERY_API,
        );
        assert!(
            hits.is_empty(),
            "{} production must not call delivery_fallback_audit directly (M90):\n{}",
            path.display(),
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_animated_delivery_audit_m91() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 91),
        "contract must document M91"
    );

    let paths = [
        root.join("crates/vid/src/animated_image.rs"),
        root.join("crates/vid/src/conversion_api.rs"),
    ];
    for path in &paths {
        let content =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains("delivery_api_path_fallback_audit"),
            "{} must use strict-gated path fallback audit (M91)",
            path.display()
        );
        let hits = offending_lines(&root, std::slice::from_ref(path), MC_FORBIDDEN_M91_ANIMATED);
        assert!(
            hits.is_empty(),
            "{} production must not call always-on delivery_path_audit (M91):\n{}",
            path.display(),
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_gate_path_labels_m92() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 92),
        "contract must document M92"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("delivery_jxl_batch_fallback_audit"),
        "gate must export delivery_jxl_batch_fallback_audit (M92)"
    );
    let batch_body = gate_fn_body(&gate, "delivery_jxl_batch_fallback_audit");
    assert!(
        batch_body.contains("delivery_strict_batch_audit"),
        "delivery_jxl_batch_fallback_audit must delegate to strict SSOT (M92/M97)"
    );
    for (fn_name, strict_expected) in [
        ("processed_path_key", true),
        ("delivery_frame_count_label(", true),
        ("path_file_name_for_log", true),
        ("color_info_for_cjxl_prep", false),
        ("ffprobe_pix_fmt_or_empty", false),
    ] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            !body.contains("delivery_fallback_audit(\n                \"color_info_probe\"")
                && !body.contains("delivery_fallback_audit(\n                \"ffprobe_pix_fmt\""),
            "{fn_name} must not always-on audit color_info/ffprobe_pix_fmt (M92)"
        );
        if strict_expected {
            assert!(
                body.contains("strict_media_conversion_delivery_enabled")
                    || body.contains("delivery_strict_path_audit")
                    || body.contains("delivery_strict_batch_audit"),
                "{fn_name} must be strict-gated via SSOT (M92/M98)"
            );
        } else {
            assert!(
                !body.contains("delivery_fallback_audit"),
                "{fn_name} must be policy-silent (M92)"
            );
        }
    }

    for rel in [
        "crates/foundation/src/jxl_explorer.rs",
        "crates/foundation/src/jxl_utils.rs",
    ] {
        let path = join_legacy_aware(&root, rel);
        let hits = offending_lines(&root, &[path], MC_FORBIDDEN_M92_JXL_BATCH);
        assert!(
            hits.is_empty(),
            "{rel} production must not call always-on delivery_jxl_batch_audit (M92):\n{}",
            hits.join("\n")
        );
    }
}

#[test]
fn media_conversion_probe_substrate_audit_m93() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 93),
        "contract must document M93"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for (fn_name, sym) in [
        ("probe_layer_audit", "delivery_strict_path_audit"),
        ("probe_layer_batch_audit", "delivery_strict_batch_audit"),
    ] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            body.contains(sym),
            "{fn_name} must delegate to {sym} (M93/M98)"
        );
    }
    let gif_fps = gate_fn_body(&gate, "gif_encode_fps_from_probe");
    assert!(
        !gif_fps.contains("gif_fps_avg_frame_rate") && !gif_fps.contains("gif_fps_r_frame_rate"),
        "gif_encode_fps_from_probe must not audit avg/r_frame_rate ladder (M93)"
    );
    assert!(
        gif_fps.contains("delivery_api_path_fallback_audit"),
        "gif_encode_fps_from_probe must strict-audit unavailable FPS only (M93)"
    );
    let warm = gate_fn_body(&gate, "warm_start_crf_or_predicted");
    assert!(
        !warm.contains("delivery_fallback_audit"),
        "warm_start_crf_or_predicted must be policy-silent (M93)"
    );
    let ssim = gate_fn_body(&gate, "conversion_ssim_message_token");
    assert!(
        ssim.contains("delivery_progress_batch_audit"),
        "conversion_ssim_message_token must audit missing SSIM via delivery_progress_batch_audit \
         (M113/M93)"
    );
}

#[test]
fn media_conversion_pipeline_audit_m94() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 94),
        "contract must document M94"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for fn_name in [
        "delivery_pipeline_path_audit",
        "delivery_pipeline_batch_audit",
        "hdr_metadata_fallback_audit",
        "apple_compat_fallback_audit",
        "delivery_cleanup_audit",
    ] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            body.contains("delivery_strict_path_audit")
                || body.contains("delivery_strict_batch_audit")
                || body.contains("strict_media_conversion_delivery_enabled"),
            "{fn_name} must be strict-gated or delegate to delivery_strict_* (M94/M96)"
        );
    }
    let cli = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/cli_runner.rs",
    ))
    .expect("cli_runner.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        production_scope(&cli).contains("delivery_pipeline_path_audit"),
        "cli_runner must route pipeline fallbacks through gate (M94)"
    );
}

#[test]
fn media_conversion_delivery_substrate_m95() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 95),
        "contract must document M95"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("fn delivery_substrate_path_audit")
            && gate.contains("fn delivery_substrate_batch_audit"),
        "gate must define delivery_substrate_* helpers (M95)"
    );
    for fn_name in [
        "delivery_encode_path_audit",
        "delivery_gpu_batch_audit",
        "delivery_runtime_batch_audit",
        "delivery_checkpoint_path_audit",
    ] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            body.contains("delivery_substrate_"),
            "{fn_name} must delegate to delivery_substrate_* (M95)"
        );
    }
    let gpu_conc = gate_fn_body(&gate, "gpu_concurrency_max_or_default");
    assert!(
        !gpu_conc.contains("delivery_gpu_batch_audit"),
        "gpu_concurrency_max_or_default must be policy-silent (M95)"
    );
    let gpu_ext = gate_fn_body(&gate, "gpu_output_extension_segment");
    assert!(
        !gpu_ext.contains("delivery_gpu_batch_audit"),
        "gpu_output_extension_segment must be policy-silent (M95)"
    );

    let conversion_path = join_legacy_aware(&root, "crates/foundation/src/conversion.rs");
    let hits = offending_lines(&root, &[conversion_path], MC_FORBIDDEN_M95_CONVERSION);
    assert!(
        hits.is_empty(),
        "conversion.rs production must not call delivery_path_audit/delivery_batch_audit directly \
         (M95):\n{}",
        hits.join("\n")
    );
    let ffprobe = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/ffprobe.rs"))
        .expect("ffprobe.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !production_scope(&ffprobe).contains("delivery_batch_audit("),
        "ffprobe production must not call always-on delivery_batch_audit (M95)"
    );
}

#[test]
fn media_conversion_strict_ssot_m97() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 97),
        "contract must document M97"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for fn_name in [
        "delivery_jxl_path_audit",
        "delivery_jxl_batch_audit",
        "delivery_jxl_path_fallback_audit",
        "delivery_jxl_batch_fallback_audit",
        "delivery_path_layout_fallback_audit",
    ] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            body.contains("delivery_strict_path_audit")
                || body.contains("delivery_strict_batch_audit"),
            "{fn_name} must delegate to strict SSOT (M97)"
        );
        assert!(
            !body.contains("delivery_path_audit(") && !body.contains("delivery_batch_audit("),
            "{fn_name} must not call always-on delivery_path/batch audit (M97)"
        );
    }
    let recovery = gate_fn_body(&gate, "probe_detection_recovery_audit");
    assert!(
        recovery.contains("probe_layer_batch_audit"),
        "probe_detection_recovery_audit must delegate to probe_layer (M97)"
    );
    assert!(
        !recovery.contains("strict_media_conversion_delivery_enabled"),
        "probe_detection_recovery_audit must not double-gate (M97)"
    );
    let metric = gate_fn_body(&gate, "explore_metric_parse_reject_audit");
    assert!(
        metric.contains("explore_precheck_batch_audit"),
        "explore_metric_parse_reject_audit must delegate to precheck batch (M97)"
    );
    assert!(
        !metric.contains("strict_media_conversion_delivery_enabled"),
        "explore_metric_parse_reject_audit must not double-gate (M97)"
    );
    let cal = gate_fn_body(&gate, "explore_calibration_degraded_audit");
    assert!(
        cal.contains("explore_precheck_batch_audit"),
        "explore_calibration_degraded_audit must delegate to precheck batch (M97)"
    );
    assert!(
        !cal.contains("strict_media_conversion_delivery_enabled"),
        "explore_calibration_degraded_audit must not double-gate (M97)"
    );
}

#[test]
fn media_conversion_production_emitter_seal_m100() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 100),
        "contract must document M100"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("pub(crate) fn delivery_fallback_audit")
            && gate.contains("pub(crate) fn delivery_path_audit")
            && gate.contains("pub(crate) fn delivery_batch_audit"),
        "all emitters must be crate-private (M100/M101)"
    );

    let hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M100_DELIVERY_EMITTERS,
    );
    let hits: Vec<String> = hits
        .into_iter()
        .filter(|line| !line.starts_with("crates/foundation/src/convert/media_conversion_gate.rs:"))
        .collect();
    assert!(
        hits.is_empty(),
        "img/vid/foundation production must not call delivery emitters outside gate (M100):\n{}",
        hits.join("\n")
    );

    let gate_log = gate.matches("log_anomaly!").count();
    assert_eq!(
        gate_log, 1,
        "media_conversion_gate must have exactly one log_anomaly site (M100 seal)"
    );
}

#[test]
fn media_conversion_strict_ssot_entry_m99() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 99),
        "contract must document M99"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in ["delivery_strict_path_audit", "delivery_strict_batch_audit"] {
        let body = gate_fn_body(&gate, sym);
        assert!(
            body.contains("strict_media_conversion_delivery_enabled"),
            "{sym} must gate on strict delivery (M99)"
        );
    }
    assert!(
        gate.contains("Call only from [`delivery_strict_path_audit`]"),
        "delivery_path_audit must document emitter-only contract (M99)"
    );
}

#[test]
fn media_conversion_gate_helpers_ssot_m98() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 98),
        "contract must document M98"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    let probe_path = gate_fn_body(&gate, "probe_layer_audit");
    assert!(
        probe_path.contains("delivery_strict_path_audit"),
        "probe_layer_audit must delegate to delivery_strict_path_audit (M98)"
    );
    let probe_batch = gate_fn_body(&gate, "probe_layer_batch_audit");
    assert!(
        probe_batch.contains("delivery_strict_batch_audit"),
        "probe_layer_batch_audit must delegate to delivery_strict_batch_audit (M98)"
    );

    for fn_name in ["processed_path_key", "mutex_guard_or_recover"] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            !body.contains("delivery_fallback_audit("),
            "{fn_name} must not call delivery_fallback_audit directly (M98)"
        );
        assert!(
            body.contains("delivery_strict_batch_audit"),
            "{fn_name} must use delivery_strict_batch_audit (M98)"
        );
    }
    let frame_body = gate
        .split("pub fn delivery_frame_count_label(")
        .nth(1)
        .and_then(|tail| tail.split("\n\n/// ").next())
        .unwrap_or_else(|| panic!("gate must define delivery_frame_count_label (M98)")); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        !frame_body.contains("delivery_fallback_audit("),
        "delivery_frame_count_label must not call delivery_fallback_audit directly (M98)"
    );
    assert!(
        frame_body.contains("delivery_strict_batch_audit"),
        "delivery_frame_count_label must use delivery_strict_batch_audit (M98)"
    );

    let path_audit = gate_fn_body(&gate, "delivery_path_audit");
    let batch_audit = gate_fn_body(&gate, "delivery_batch_audit");
    assert!(
        path_audit.contains("delivery_fallback_audit("),
        "delivery_path_audit remains the path-scoped emitter (M98)"
    );
    assert!(
        batch_audit.contains("delivery_fallback_audit("),
        "delivery_batch_audit remains the batch emitter (M98)"
    );

    let strict_path_hits = gate.matches("delivery_path_audit(").count();
    let strict_batch_hits = gate.matches("delivery_batch_audit(").count();
    assert_eq!(
        strict_path_hits, 2,
        "delivery_path_audit must only appear in strict SSOT + definition (M98)"
    );
    assert_eq!(
        strict_batch_hits, 2,
        "delivery_batch_audit must only appear in strict SSOT + definition (M98)"
    );

    for sym in ["delivery_strict_path_audit", "delivery_strict_batch_audit"] {
        let body = gate_fn_body(&gate, sym);
        assert!(
            body.contains("strict_media_conversion_delivery_enabled"),
            "{sym} must retain strict gate (M99 anti-regression)"
        );
        assert!(
            body.contains("delivery_path_audit") || body.contains("delivery_batch_audit"),
            "{sym} must delegate to path/batch emitters only (M99)"
        );
    }
}

#[test]
fn media_conversion_strict_ssot_m96() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 96),
        "contract must document M96"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in ["delivery_strict_path_audit", "delivery_strict_batch_audit"] {
        assert!(gate.contains(sym), "gate must export {sym} (M96)");
    }
    for fn_name in [
        "delivery_api_path_fallback_audit",
        "delivery_api_batch_fallback_audit",
        "delivery_pipeline_path_audit",
        "explore_precheck_batch_audit",
        "explore_gpu_coarse_batch_audit",
        "delivery_progress_eta_unknown_audit",
    ] {
        let body = gate_fn_body(&gate, fn_name);
        assert!(
            body.contains("delivery_strict_path_audit")
                || body.contains("delivery_strict_batch_audit"),
            "{fn_name} must delegate to strict SSOT helper (M96)"
        );
    }
}

#[test]
fn media_conversion_precheck_stream_audit_m84() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 84),
        "contract must document M84"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_precheck_degraded_audit",
        "explore_precheck_nb_frames_or_zero",
        "explore_delivery_explore_outcome_audit",
        "explore_progress_time_millis_optional",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M84)");
    }
    let degraded = gate_fn_body(&gate, "explore_precheck_degraded_audit");
    assert!(
        degraded.contains("explore_precheck_batch_audit"),
        "explore_precheck_degraded_audit must delegate to explore_precheck_batch_audit (M84/M96)"
    );

    let paths = [
        join_legacy_aware(&root, "crates/foundation/src/video_explorer/precheck.rs"),
        join_legacy_aware(
            &root,
            "crates/foundation/src/video_explorer/stream_analysis.rs",
        ),
        join_legacy_aware(&root, "crates/foundation/src/video_explorer.rs"),
    ];
    for path in &paths {
        let content =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains("explore_precheck_degraded_audit")
                || prod.contains("explore_precheck_nb_frames_or_zero")
                || prod.contains("explore_delivery_explore_outcome_audit")
                || prod.contains("explore_progress_time_millis_or_zero"),
            "{} must route explore aux through M84 gate helpers",
            path.display()
        );
    }

    let hits = offending_lines(&root, &paths[..2], MC_FORBIDDEN_M84_PRECHECK_STREAM);
    assert!(
        hits.is_empty(),
        "M84 precheck/stream must not use always-on batch audits or duration-ladder spam:\n{}",
        hits.join("\n")
    );

    let explorer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod_explorer = production_scope(&explorer);
    assert!(
        prod_explorer.contains("explore_delivery_explore_outcome_audit"),
        "video_explorer explore outcomes must use strict-gated audit (M84)"
    );
    assert!(
        !prod_explorer.contains("delivery_batch_audit(\n            \"explore_highly_compressed\""),
        "video_explorer must not always-on audit highly_compressed outcome (M84)"
    );
}

#[test]
fn media_conversion_gpu_coarse_explore_audit_m83() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 83),
        "contract must document M83"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_gpu_coarse_explore_audit",
        "explore_gpu_coarse_audio_bitrate_or_default",
        "explore_gpu_coarse_audio_bitrate_optional",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M83)");
    }
    let explore = gate_fn_body(&gate, "explore_gpu_coarse_explore_audit");
    assert!(
        explore.contains("explore_gpu_coarse_fallback_audit"),
        "explore_gpu_coarse_explore_audit must delegate to fallback audit (M83/M96)"
    );
    let audio = gate_fn_body(&gate, "explore_gpu_coarse_audio_bitrate_optional");
    assert!(
        audio.contains("explore_gpu_coarse_fallback_audit"),
        "audio bitrate optional must audit absent bit_rate (M83)"
    );

    let gpu_path = join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    );
    let gpu = fs::read_to_string(&gpu_path).expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&gpu);
    assert!(
        prod.contains("explore_gpu_coarse_explore_audit"),
        "gpu_coarse_search must route explore diagnostics through strict helper (M83)"
    );
    assert!(
        prod.contains("explore_gpu_coarse_audio_bitrate_optional"),
        "gpu_coarse_search must use gate optional audio bitrate (M83)"
    );
    assert!(
        !prod.contains("explore_gpu_coarse_audio_bitrate_or_default"),
        "gpu_coarse_search must not use forged audio bitrate default helper (M83)"
    );

    let hits = offending_lines(&root, &[gpu_path], MC_FORBIDDEN_M83_GPU_COARSE);
    assert!(
        hits.is_empty(),
        "M83 gpu_coarse_search must not call batch audit directly or inline audio default:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_progress_eta_mutex_m82() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 82),
        "contract must document M82"
    );

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let eta = gate_fn_body(&gate, "delivery_progress_eta_unknown_audit");
    assert!(
        eta.contains("delivery_strict_batch_audit"),
        "delivery_progress_eta_unknown_audit must delegate to delivery_strict_batch_audit \
         (M82/M96)"
    );

    let progress = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/progress.rs",
    ))
    .expect("progress.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let prod = production_scope(&progress);
    assert!(
        prod.contains("progress_active_line_read") && prod.contains("mutex_guard_or_recover"),
        "active_progress_line must use mutex_guard_or_recover (M82)"
    );
    assert!(
        !prod.contains(
            "delivery_progress_batch_audit(\n                \"progress_active_line_read\""
        ),
        "active_progress_line must not emit delivery_progress_batch_audit on poison (M82)"
    );
}

#[test]
fn media_conversion_precision_metric_sealing_m74() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 74),
        "contract must document M74"
    );

    for (rel, needle) in [
        (
            "crates/foundation/src/explore_strategy.rs",
            "parse_explore_psnr_metric_token",
        ),
        (
            "crates/foundation/src/video_explorer.rs",
            "parse_explore_psnr_metric_token",
        ),
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            "parse_explore_ssim_metric_token",
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains(needle),
            "{rel} must use central parser {needle} (M74)"
        );
    }

    let parse_hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M74_EXPLORE_PARSE,
    );
    assert!(
        parse_hits.is_empty(),
        "M74 explore parse residues must be cleared:\n{}",
        parse_hits.join("\n")
    );
}

#[test]
fn media_conversion_precision_metric_sealing_m73() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 73),
        "contract must document M73"
    );

    let precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precision.rs",
    ))
    .expect("precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "parse_explore_ssim_metric_token",
        "parse_explore_psnr_metric_token",
        "parse_explore_ms_ssim_score_token",
    ] {
        assert!(
            precision.contains(sym),
            "precision.rs must define central parser {sym} (M73)"
        );
    }

    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "explore_gpu_sample_duration_optional",
        "explore_adaptive_vmaf_y_floor_optional",
        "explore_adaptive_psnr_uv_floor_optional",
        "probe_ffprobe_duration_text_or_none",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} (M73)");
    }

    for (rel, needle) in [
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            "explore_gpu_sample_duration_optional",
        ),
        (
            "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            "explore_adaptive_vmaf_y_floor_optional",
        ),
        (
            "crates/foundation/src/stream_size.rs",
            "probe_ffprobe_duration_text_or_none",
        ),
        (
            "crates/foundation/src/gpu_accel.rs",
            "parse_explore_ssim_metric_token",
        ),
        (
            "crates/foundation/src/explore_strategy.rs",
            "parse_explore_ssim_metric_token",
        ),
        (
            "crates/foundation/src/video_explorer.rs",
            "parse_explore_ssim_metric_token",
        ),
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        assert!(
            prod.contains(needle),
            "{rel} must route through {needle} (M73)"
        );
    }

    let parse_hits = offending_lines(
        &root,
        &production_rust_files(&root),
        MC_FORBIDDEN_M73_CENTRAL_PARSE,
    );
    assert!(
        parse_hits.is_empty(),
        "M73 must use central parsers / gate defaults only:\n{}",
        parse_hits.join("\n")
    );
}

fn gate_fn_body<'a>(gate: &'a str, fn_name: &str) -> &'a str {
    let start = gate
        .find(&format!("pub(crate) fn {fn_name}"))
        .or_else(|| gate.find(&format!("pub fn {fn_name}")))
        .unwrap_or_else(|| panic!("gate must define {fn_name}")); // audited: contract test assertion path; panic/expect is test-only failure signal
    let tail = &gate[start..];
    let end = tail
        .find("\n\n/// ")
        .or_else(|| tail.find("\n\n#[must_use]"))
        .or_else(|| tail.find("\n\npub fn "))
        .unwrap_or(tail.len());
    &tail[..end]
}

#[test]
fn media_conversion_gate_audit_policy_phase_i() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_HARDENING_AUDIT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract.contains("phase I"),
        "hardening audit must document phase I audit policy"
    );
    let jxl_prev = gate_fn_body(&gate, "jxl_previous_candidate_size_or_fallback");
    assert!(
        jxl_prev.contains("delivery_jxl_batch_fallback_audit"),
        "JXL first-probe baseline must audit via delivery_jxl_batch_fallback_audit (CR-97)"
    );
    assert!(
        !jxl_prev.contains("delivery_jxl_batch_audit("),
        "JXL first-probe baseline must not spam delivery_jxl_batch_audit"
    );
    let promote = gate_fn_body(&gate, "probe_animated_promoted_frame_count_or_min_two");
    assert!(
        !promote.contains("probe_layer_audit") && !promote.contains("delivery_fallback_audit"),
        "animated promote min-2 is routing policy, not a degrading fallback"
    );
    let profile = gate_fn_body(&gate, "algorithm_feature_distribution_required");
    assert!(
        profile.contains("ok_or_else"),
        "loop profile feature slots must fail closed via algorithm_feature_distribution_required"
    );
    for fn_name in [
        "loop_gif_logical_screen_optional",
        "probe_webp_header_frame_count_optional",
        "explore_metric_parse_reject_audit",
        "probe_ffprobe_duration_text_or_none",
        "stream_size_probe_failure_audit",
        "explore_gpu_coarse_fallback_audit",
        "explore_ssim_metric_degraded_audit",
        "explore_calibration_degraded_audit",
        "delivery_progress_eta_unknown_audit",
        "explore_gpu_coarse_explore_audit",
        "explore_precheck_degraded_audit",
        "explore_precheck_nb_frames_or_zero",
        "explore_delivery_explore_outcome_audit",
        "explore_progress_time_millis_optional",
        "explore_size_target_failure_reason_or_default",
        "quality_heuristic_fallback_audit",
        "delivery_path_layout_fallback_audit",
        "probe_detection_recovery_audit",
        "probe_layer_audit",
        "delivery_pipeline_path_audit",
        "hdr_metadata_fallback_audit",
    ] {
        let body = gate_fn_body(&gate, fn_name);
        if fn_name == "probe_ffprobe_duration_text_or_none" {
            assert!(
                !body.contains("delivery_encode_batch_audit"),
                "{fn_name} must not duplicate encode audits (callers audit on None, M75)"
            );
        } else if fn_name == "stream_size_probe_failure_audit"
            || fn_name == "explore_gpu_coarse_fallback_audit"
            || fn_name == "explore_calibration_degraded_audit"
            || fn_name == "delivery_progress_eta_unknown_audit"
            || fn_name == "explore_gpu_coarse_explore_audit"
            || fn_name == "explore_precheck_degraded_audit"
            || fn_name == "explore_delivery_explore_outcome_audit"
            || fn_name == "quality_heuristic_fallback_audit"
            || fn_name == "delivery_path_layout_fallback_audit"
            || fn_name == "probe_detection_recovery_audit"
            || fn_name == "probe_layer_audit"
            || fn_name == "delivery_pipeline_path_audit"
            || fn_name == "hdr_metadata_fallback_audit"
        {
            assert!(
                body.contains("strict_media_conversion_delivery_enabled")
                    || body.contains("delivery_strict_path_audit")
                    || body.contains("delivery_strict_batch_audit")
                    || body.contains("explore_gpu_coarse_batch_audit")
                    || body.contains("explore_gpu_coarse_fallback_audit")
                    || body.contains("explore_precheck_batch_audit")
                    || body.contains("probe_quality_batch_audit")
                    || body.contains("probe_layer_batch_audit")
                    || body.contains("delivery_encode_path_audit"),
                "{fn_name} must gate audits behind strict delivery (M76–M98 SSOT)"
            );
        } else if fn_name == "explore_ssim_metric_degraded_audit" {
            assert!(
                body.contains("explore_precheck_degraded_audit"),
                "{fn_name} must delegate to strict-gated precheck degraded audit (M79/M84)"
            );
        } else if fn_name == "explore_precheck_nb_frames_or_zero" {
            assert!(
                body.contains("explore_precheck_degraded_audit"),
                "{fn_name} must route nb_frames fallback through strict-gated degraded audit (M84)"
            );
        } else if fn_name == "explore_size_target_failure_reason_or_default" {
            assert!(
                body.contains("explore_delivery_explore_outcome_audit"),
                "{fn_name} must route empty reason through strict-gated outcome audit (M86)"
            );
        } else if fn_name == "explore_progress_time_millis_optional" {
            assert!(
                body.contains("explore_delivery_explore_outcome_audit"),
                "{fn_name} must route overflow through strict-gated outcome audit (M84)"
            );
        } else if fn_name == "explore_metric_parse_reject_audit" {
            assert!(
                body.contains("explore_precheck_batch_audit"),
                "{fn_name} must delegate metric reject audits to precheck batch (M75/M97)"
            );
        } else if fn_name == "delivery_output_file_len_or_estimate" {
            assert!(
                body.contains("delivery_strict_path_audit"),
                "{fn_name} must delegate output-size fallback to strict path audit (M97)"
            );
        } else if fn_name == "stream_size_duration_fallback_audit"
            || fn_name == "stream_size_probe_failure_audit"
        {
            assert!(
                body.contains("delivery_encode_path_audit"),
                "{fn_name} must delegate to encode path audit (M76/M98)"
            );
        } else {
            assert!(
                body.contains("strict_media_conversion_delivery_enabled")
                    || body.contains("delivery_strict_path_audit")
                    || body.contains("delivery_strict_batch_audit")
                    || body.contains("probe_layer_audit")
                    || body.contains("probe_layer_batch_audit")
                    || body.contains("delivery_db_batch_audit")
                    || body.contains("delivery_intent_batch_audit")
                    || body.contains("delivery_runtime_batch_audit")
                    || body.contains("delivery_gpu_batch_audit")
                    || body.contains("delivery_jxl_batch_audit"),
                "{fn_name} must route audits through strict SSOT delegates (M98)"
            );
        }
    }
    let precision = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/precision.rs",
    ))
    .expect("precision.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        precision.contains("seal_ms_ssim_composite_average"),
        "M75 must tolerate composite MS-SSIM float noise without rejecting whole YUV bundle"
    );
}

#[test]
fn media_conversion_core_error_surfaces_m53() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("ui_user_facing_warning"),
        "gate must export ui_user_facing_warning for M53"
    );
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 53),
        "contract must document M53"
    );
    for (rel, needle) in [
        (
            "crates/foundation/src/unified_error.rs",
            "ui_user_facing_error",
        ),
        ("crates/foundation/src/app_error.rs", "ui_user_facing_error"),
        (
            "crates/foundation/src/quality_matcher.rs",
            "ui_user_facing_error",
        ),
    ] {
        let path = join_legacy_aware(&root, rel);
        let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains(needle),
            "{rel} must route core user errors through gate ({needle}, M53)"
        );
        let hits = offending_lines(&root, &[path], MC_FORBIDDEN_M53_SYMBOLS);
        assert!(
            hits.is_empty(),
            "{rel} must not inline symbols::pick for user errors after M53:\n{}",
            hits.join("\n")
        );
    }
    let unified = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/unified_error.rs",
    ))
    .expect("unified_error.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let app = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/app_error.rs",
    ))
    .expect("app_error.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        unified.contains("ui_user_facing_warning") && app.contains("ui_user_facing_warning"),
        "core error surfaces must route warnings through gate (M53)"
    );
}

#[test]
fn media_conversion_safety_and_explore_icons_m54() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "ui_safety_system_dir_blocked",
        "ui_safety_home_root_blocked",
        "ui_safety_photos_library_blocked",
        "ui_explore_crf_compress_ok_mark",
        "ui_explore_crf_too_large_mark",
        "ui_explore_crf_target_mark",
    ] {
        assert!(gate.contains(sym), "gate must export {sym} for M54");
    }
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 54),
        "contract must document M54"
    );
    let safety = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/safety.rs"))
        .expect("safety.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        safety.contains("ui_safety_system_dir_blocked")
            && safety.contains("ui_safety_home_root_blocked")
            && safety.contains("ui_safety_photos_library_blocked"),
        "safety.rs must route blocks through gate (M54)"
    );
    assert!(
        !safety.contains("modern_ui::symbols::pick"),
        "safety.rs must not call symbols::pick directly after M54"
    );
    assert!(
        !safety.contains("🚨 DANGEROUS OPERATION BLOCKED"),
        "safety.rs must not embed raw crit emoji literals after M54"
    );
    let explore = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        explore.contains("ui_explore_crf_compress_ok_mark")
            && explore.contains("ui_explore_crf_too_large_mark")
            && explore.contains("ui_explore_crf_target_mark"),
        "video_explorer CRF bisect log_detail must use gate marks (M54)"
    );
    let hits = offending_lines(
        &root,
        &[join_legacy_aware(
            &root,
            "crates/foundation/src/video_explorer.rs",
        )],
        &["LABEL_PHASE_1", "pick(\"❌\", \"[ERROR]\")"],
    );
    let phase1_err = hits
        .iter()
        .filter(|line| line.contains("LABEL_PHASE_1") && line.contains("pick(\"❌\""))
        .count();
    assert_eq!(
        phase1_err,
        0,
        "video_explorer phase-1 CRF lines must not inline error pick after M54:\n{}",
        hits.join("\n")
    );
}

#[test]
fn media_conversion_static_log_severity_icons_m55() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("ui_error_severity_colored_label"),
        "gate must export ui_error_severity_colored_label for M55"
    );
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 55),
        "contract must document M55"
    );
    let logs = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/static_logs.rs",
    ))
    .expect("static_logs.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logs.contains("ui_error_severity_colored_label"),
        "ErrorSeverity::label_colored must delegate to gate (M55)"
    );
    let start = logs
        .find("pub fn label_colored")
        .expect("label_colored must exist"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let end = logs[start..]
        .find("\n    ///")
        .or_else(|| logs[start..].find("\n}\n"))
        .map_or(start + 400, |off| start + off);
    let label_fn = &logs[start..end];
    assert!(
        !label_fn.contains("modern_ui::symbols::pick"),
        "label_colored must not call symbols::pick directly after M55"
    );
    let logging = read_hardening_doc(&root, "LOGGING_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logging_contract_documents_row(&logging, 8),
        "logging contract must document L8"
    );
}

#[test]
fn media_conversion_static_logs_icon_pick_m56() {
    let root = workspace_root();
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gate.contains("pub fn ui_icon_pick"),
        "gate must export ui_icon_pick for M56"
    );
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 56),
        "contract must document M56"
    );
    let logs = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/static_logs.rs",
    ))
    .expect("static_logs.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logs.contains("media_conversion_gate::ui_icon_pick"),
        "static_logs must use gate icon pick (M56)"
    );
    assert!(
        !logs.contains("symbols::pick"),
        "static_logs must not call symbols::pick after M56"
    );
    let logging = read_hardening_doc(&root, "LOGGING_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logging_contract_documents_row(&logging, 9),
        "logging contract must document L9"
    );
}

#[test]
fn media_conversion_video_explorer_icon_pick_m57() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 57),
        "contract must document M57"
    );
    let explore = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer.rs",
    ))
    .expect("video_explorer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        explore.contains("media_conversion_gate::ui_icon_pick"),
        "video_explorer must route explore icons through gate (M57)"
    );
    assert!(
        !explore.contains("modern_ui::symbols::pick"),
        "video_explorer must not call modern_ui::symbols::pick after M57"
    );
    let logging = read_hardening_doc(&root, "LOGGING_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logging_contract_documents_row(&logging, 10),
        "logging contract must document L10"
    );
}

#[test]
fn media_conversion_gpu_coarse_icon_pick_m58() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 58),
        "contract must document M58"
    );
    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gpu.contains("media_conversion_gate::ui_icon_pick"),
        "gpu_coarse_search must route icons through gate (M58)"
    );
    assert!(
        !gpu.contains("symbols::pick"),
        "gpu_coarse_search must not call symbols::pick after M58"
    );
}

#[test]
fn media_conversion_explore_strategy_icon_pick_m59() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 59),
        "contract must document M59"
    );
    let strategy = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/explore_strategy.rs",
    ))
    .expect("explore_strategy.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        strategy.contains("media_conversion_gate::ui_icon_pick"),
        "explore_strategy must route icons through gate (M59)"
    );
    assert!(
        !strategy.contains("modern_ui::symbols::pick"),
        "explore_strategy must not call modern_ui::symbols::pick after M59"
    );
}

#[test]
fn media_conversion_delivery_quality_tooling_icons_m60() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 60),
        "contract must document M60"
    );
    for rel in [
        "crates/foundation/src/ffmpeg_process.rs",
        "crates/foundation/src/msssim_progress.rs",
        "crates/foundation/src/msssim_parallel.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::ui_icon_pick"),
            "{rel} must route user-visible icons through gate (M60)"
        );
        assert!(
            !content.contains("modern_ui::symbols::pick"),
            "{rel} must not call modern_ui::symbols::pick after M60"
        );
    }
}

#[test]
fn media_conversion_progress_mode_icon_pick_m61() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 61),
        "contract must document M61"
    );
    let progress = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/progress_mode.rs",
    ))
    .expect("progress_mode.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        progress.contains("media_conversion_gate::ui_icon_pick"),
        "progress_mode must route icons through gate (M61)"
    );
    assert!(
        !progress.contains("symbols::pick"),
        "progress_mode must not call symbols::pick after M61"
    );
    let jxl = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/jxl_utils.rs",
    ))
    .expect("jxl_utils.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        jxl.contains("media_conversion_gate::ui_icon_pick")
            && !jxl.contains("modern_ui::symbols::pick"),
        "jxl_utils delivery stderr must use gate ui_icon_pick (M61)"
    );
}

#[test]
fn media_conversion_progress_and_logging_icons_m62() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 62),
        "contract must document M62"
    );
    for rel in [
        "crates/foundation/src/progress.rs",
        "crates/foundation/src/logging.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::ui_icon_pick"),
            "{rel} must route progress/logging icons through gate (M62)"
        );
        assert!(
            !content.contains("symbols::pick") && !content.contains("modern_ui::symbols::pick"),
            "{rel} must not call symbols::pick after M62"
        );
    }
}

#[test]
fn media_conversion_report_icons_m63() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 63),
        "contract must document M63"
    );
    let report = fs::read_to_string(join_legacy_aware(&root, "crates/foundation/src/report.rs"))
        .expect("report.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        report.contains("media_conversion_gate::ui_icon_pick"),
        "report.rs must route summary icons through gate (M63)"
    );
    assert!(
        !report.contains("symbols::pick"),
        "report.rs must not call symbols::pick after M63"
    );
}

#[test]
fn media_conversion_quality_and_db_audit_icons_m64() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 64),
        "contract must document M64"
    );
    let qm = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/quality_matcher.rs",
    ))
    .expect("quality_matcher.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        qm.contains("media_conversion_gate::ui_icon_pick(\"💡\", \"[HINT]\")"),
        "quality_matcher HINT lines must use gate ui_icon_pick (M64)"
    );
    assert!(
        !qm.contains("modern_ui::symbols::pick"),
        "quality_matcher must not call modern_ui::symbols::pick after M64"
    );
    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        db.contains("media_conversion_gate::ui_icon_pick"),
        "database audit stderr must use gate ui_icon_pick (M64)"
    );
}

#[test]
fn media_conversion_delivery_io_icons_m65() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 65),
        "contract must document M65"
    );
    for rel in [
        "crates/foundation/src/file_copier.rs",
        "crates/foundation/src/image_analyzer.rs",
        "crates/foundation/src/video_detection.rs",
        "crates/foundation/src/cli_runner.rs",
        "crates/foundation/src/stream_size.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("media_conversion_gate::ui_icon_pick"),
            "{rel} must route delivery I/O icons through gate (M65)"
        );
        assert!(
            !content.contains("modern_ui::symbols::pick"),
            "{rel} must not call modern_ui::symbols::pick after M65"
        );
    }
}

#[test]
fn media_conversion_gpu_accel_icon_pick_m66() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        contract_documents_milestone(&contract, 66),
        "contract must document M66"
    );
    let gpu = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/gpu_accel.rs",
    ))
    .expect("gpu_accel.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        gpu.contains("media_conversion_gate::ui_icon_pick"),
        "gpu_accel must route icons through gate (M66)"
    );
    assert!(
        !gpu.contains("modern_ui::symbols::pick") && !gpu.contains("symbols::pick"),
        "gpu_accel must not call symbols::pick after M66"
    );
}

#[test]
fn database_loop_stderr_and_duration_use_gate_u12() {
    let root = workspace_root();
    let ui_contract = read_hardening_doc(&root, "UI_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ui_contract.contains("| U12 |"),
        "UI contract must document U12"
    );
    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        db.contains("ui_duration_secs_label_or_unknown"),
        "database loop ingest must use duration gate helper (U12)"
    );
    assert!(
        db.contains("ui_stderr::line") || db.contains("symbols::pick"),
        "database audit stderr must use ui_stderr or symbols::pick (U12)"
    );
    assert!(
        !db.contains("\"📊 LoopIntent DB Check"),
        "database must not use raw stats emoji in log format literals (U12)"
    );
}

#[test]
fn probe_detection_stderr_use_gate_u13() {
    let root = workspace_root();
    let ui_contract = read_hardening_doc(&root, "UI_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ui_contract.contains("| U13 |"),
        "UI contract must document U13"
    );
    let analyzer = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let detection = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        analyzer.contains("ui_probe_stats_stderr")
            && detection.contains("ui_penetration_warning_stderr"),
        "probe/detection stderr must use gate helpers (U13)"
    );
}

#[test]
fn quality_report_headers_use_gate_u14() {
    let root = workspace_root();
    let ui_contract = read_hardening_doc(&root, "UI_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ui_contract.contains("| U14 |"),
        "UI contract must document U14"
    );
    let image_q = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_quality_detector.rs",
    ))
    .expect("image_quality_detector.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        image_q.contains("ui_visual_artifact_audit_title"),
        "quality report headers must use gate (U14)"
    );
}

#[test]
fn path_validator_security_errors_use_gate_u15() {
    let root = workspace_root();
    let ui_contract = read_hardening_doc(&root, "UI_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ui_contract.contains("| U15 |"),
        "UI contract must document U15"
    );
    let path_validator = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/path_validator.rs",
    ))
    .expect("path_validator.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        path_validator.contains("ui_user_facing_error"),
        "path_validator Display must use ui_user_facing_error (U15)"
    );
}

#[test]
fn algorithm_inference_snapshot_audit_overlay_i10() {
    let root = workspace_root();
    let algo = read_hardening_doc(&root, "ALGORITHM_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        algo.contains("| I10 |"),
        "algorithm contract must document I10"
    );
    let db = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        db.contains("json_inference_optional_f64_or_null"),
        "inference_log snapshot must use policy-silent optional f64 helper (M117)"
    );
    assert!(
        db.contains("json_finite_f64_or_null") && db.contains("runtime_final_probability"),
        "audit-only overlay must still use audited json_finite_f64_or_null (I10)"
    );
    let hits = offending_lines(
        &root,
        &[join_legacy_aware(
            &root,
            "crates/foundation/src/database.rs",
        )],
        &[
            "map_or(serde_json::Value::Null",
            ".map_or(Value::Null",
            "map_or(serde_json::json!(null)",
        ],
    );
    assert!(
        hits.is_empty(),
        "database inference_log overlays must not use inline map_or(Value::Null) after I10:\n{}",
        hits.join("\n")
    );
}

#[test]
fn logging_layer_contract_doc_exists() {
    let root = workspace_root();
    let content = read_hardening_doc(&root, "LOGGING_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in ["## Core invariants", "L1", "L6", "ui_user_facing_error"] {
        assert!(
            content.contains(required),
            "LOGGING_LAYER_CONTRACT.md must document `{required}`"
        );
    }
}

#[test]
fn database_layer_contract_doc_exists() {
    let root = workspace_root();
    let content = read_hardening_doc(&root, "DATABASE_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in ["## Core invariants", "D1", "D5", "json_finite_f64_or_null"] {
        assert!(
            content.contains(required),
            "DATABASE_LAYER_CONTRACT.md must document `{required}`"
        );
    }
}

#[test]
fn ui_tracing_path_and_result_box_use_gate_u11() {
    let root = workspace_root();
    let ui_contract = read_hardening_doc(&root, "UI_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ui_contract.contains("| U11 |"),
        "UI contract must document U11"
    );
    let modern = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/modern_ui.rs",
    ))
    .expect("modern_ui.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        modern.contains("path_tracing_log_file_name_or_app_log")
            && modern.contains("ui_result_box_width_or_title_default"),
        "modern_ui must route tracing/result-box fallbacks through gate (U11)"
    );
}

#[test]
fn media_conversion_delivery_heatmap_no_regressions() {
    let root = workspace_root();
    let baseline_path =
        root.join("crates/dev/src/fixtures/media_conversion_delivery_heatmap_baseline.json");
    let baseline_raw = fs::read_to_string(&baseline_path)
        .unwrap_or_else(|e| panic!("read {}: {e:?}", baseline_path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    let baseline: serde_json::Value = serde_json::from_str(&baseline_raw)
        .unwrap_or_else(|e| panic!("parse heatmap baseline: {e:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
    let offenders = delivery_numeric_forgery_offenders(&root);
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let gate_log_anomaly = gate.matches("log_anomaly!").count();
    assert_eq!(
        offenders.len(),
        baseline_usize_field(&baseline, "numeric_forgery_offenders"),
        "numeric forgery offender count regressed; update baseline after audited gate fixes:\n{}",
        offenders.join("\n")
    );
    assert_eq!(
        gate_log_anomaly,
        baseline_usize_field(&baseline, "gate_log_anomaly_count"),
        "gate log_anomaly count changed; update heatmap baseline if intentional"
    );
    assert_eq!(
        baseline["contract_invariants"]
            .as_u64()
            .expect("baseline contract_invariants"), /* audited: contract test assertion path; panic/expect is test-only failure signal */
        66,
        "heatmap baseline must track M66 gpu_accel icon picks (66 invariants)"
    );
    let deep = root.join("crates/dev/src/fixtures/media_conversion_deep_audit.json");
    if deep.is_file() {
        let audit: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&deep).unwrap_or_else(|e| panic!("read deep audit: {e:?}")), // audited: contract test assertion path; panic/expect is test-only failure signal
        )
        .unwrap_or_else(|e| panic!("parse deep audit: {e:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert_eq!(
            audit["extended_scan"]["map_or_high_improvement"]
                .as_u64()
                .expect("deep audit map_or_high_improvement should be present"), /* audited: contract test assertion path; panic/expect is test-only failure signal */
            0,
            "deep audit must show no map_or(100.*) blind spots after M40"
        );
        assert_eq!(
            audit["extended_scan"]["unwrap_or_else_estimate"]
                .as_u64()
                .expect("deep audit unwrap_or_else_estimate should be present"), /* audited: contract test assertion path; panic/expect is test-only failure signal */
            0,
            "deep audit must show no unwrap_or_else estimate chains after M40"
        );
        assert_eq!(
            audit["extended_scan"]["mutex_poison_inline"]
                .as_u64()
                .expect("deep audit mutex_poison_inline should be present"), /* audited: contract test assertion path; panic/expect is test-only failure signal */
            0,
            "deep audit must show no inline mutex poison recovery after M44"
        );
        assert_eq!(
            audit["extended_scan"]["temp_dir_inline"]
                .as_u64()
                .expect("deep audit temp_dir_inline should be present"), /* audited: contract
                                                                          * test assertion path;
                                                                          * panic/expect is
                                                                          * test-only failure
                                                                          * signal */
            0,
            "deep audit must route temp_dir log fallbacks through gate after M45"
        );
        for (key, rel) in [
            ("mutex_lock_ok_silent", "crates/foundation/src/progress.rs"),
            ("ui_na_inline", "crates/foundation/src/unified_progress.rs"),
            ("ui_na_inline", "crates/foundation/src/video_explorer.rs"),
            (
                "ui_na_inline",
                "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
            ),
            ("ui_na_inline", "crates/foundation/src/image_analyzer.rs"),
            (
                "ui_na_inline",
                "crates/foundation/src/image_quality_detector.rs",
            ),
            (
                "ui_na_inline",
                "crates/foundation/src/video_quality_detector.rs",
            ),
            (
                "ui_na_inline",
                "crates/foundation/src/image_jpeg_analysis.rs",
            ),
            (
                "ui_na_inline",
                "crates/foundation/src/quality_verifier_enhanced.rs",
            ),
            ("ui_na_closure", "crates/foundation/src/quality_matcher.rs"),
            ("ui_na_closure", "crates/foundation/src/image_detection.rs"),
            (
                "mutex_lock_ok_silent",
                "crates/foundation/src/ctrlc_guard.rs",
            ),
        ] {
            let samples = audit["extended_samples"][key].as_array();
            let in_file = samples.is_some_and(|arr| {
                arr.iter()
                    .any(|entry| entry.get("file").and_then(|v| v.as_str()) == Some(rel))
            });
            assert!(
                !in_file,
                "deep audit {key} must be clear in {rel} after M49"
            );
        }
    }
}

#[test]
fn media_conversion_delivery_layer_sealed() {
    let root = workspace_root();
    let contract = read_hardening_doc(&root, "MEDIA_CONVERSION_LAYER_CONTRACT.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let seal = read_hardening_doc(&root, "MEDIA_CONVERSION_DELIVERY_SEAL.md"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        seal.contains("MEDIA_CONVERSION_DISCIPLINE_SEAL")
            && seal.contains("M1–M158")
            && seal.contains("M39")
            && seal.contains("M158")
            && seal.contains("media_conversion_discipline_layer_closure_m158")
            && seal.contains("delivery_fallback_audit")
            && seal.contains("pub(crate)"),
        "delivery seal doc must link discipline seal, M1–M158 registry, M39/M158, and emitter \
         visibility"
    );
    assert!(
        root.join("crates/dev/scripts/media_conversion_delivery_heatmap.py")
            .is_file(),
        "delivery heatmap script must exist for M39 audits"
    );
    assert!(
        !read_hardening_doc(&root, "MEDIA_CONVERSION_DISCIPLINE_SEAL.md").is_empty(),
        "discipline seal doc must exist (M158)"
    );
    assert!(
        contract_documents_milestone(&contract, 158),
        "delivery contract must include M158 discipline closure"
    );
    let tests = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        tests.contains("fn media_conversion_discipline_layer_closure_m158"),
        "discipline closure test must exist (M158)"
    );
    let scope = fs::read_to_string(root.join("crates/dev/scripts/media_scope.py"))
        .expect("media_scope.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        scope.contains("(M38)")
            && scope.contains("(M39)")
            && scope.contains("(M40)")
            && scope.contains("(M41)")
            && scope.contains("(M42)")
            && scope.contains("(M43)")
            && scope.contains("(M44)")
            && scope.contains("(M45)")
            && scope.contains("(M46)")
            && scope.contains("(M47)")
            && scope.contains("(M48)")
            && scope.contains("(M49)")
            && scope.contains("(M50)")
            && scope.contains("(M51)")
            && scope.contains("(M52)")
            && scope.contains("(M53)")
            && scope.contains("(M54)")
            && scope.contains("(M55)")
            && scope.contains("(M56)")
            && scope.contains("(M57)")
            && scope.contains("(M58)")
            && scope.contains("(M59)")
            && scope.contains("(M60)")
            && scope.contains("(M61)")
            && scope.contains("(M62)")
            && scope.contains("(M63)")
            && scope.contains("(M64)")
            && scope.contains("(M65)")
            && scope.contains("(M66)")
            && scope.contains("(M67)"),
        "media_scope.py must document M38–M67"
    );
    for id in 1..=67 {
        assert!(
            contract_documents_milestone(&contract, id),
            "contract must include invariant M{id}"
        );
    }
    for rel in ["crates/img/src", "crates/vid/src"] {
        let prefix = join_legacy_aware(&root, rel);
        let hits = production_rust_files(&root)
            .into_iter()
            .filter(|path| path.starts_with(&prefix))
            .filter(|path| match fs::read_to_string(path) {
                Ok(content) => content.contains("log_anomaly!"),
                Err(_err) => false,
            })
            .collect::<Vec<_>>();
        assert!(
            hits.is_empty(),
            "{rel} delivery sources must not call log_anomaly! directly: {hits:?}"
        );
    }
    let gate = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let gate_log_anomaly = gate.matches("log_anomaly!").count();
    assert_eq!(
        gate_log_anomaly, 1,
        "only delivery_fallback_audit may emit log_anomaly! in media_conversion_gate (found \
         {gate_log_anomaly})"
    );
}

#[test]
fn training_tier_ambiguous_policy_defaults_to_exclude() {
    let root = workspace_root();
    let tier_rs = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/training_tier_audit.rs",
    ))
    .expect("training_tier_audit.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let entry_guard = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/entry_guard.rs",
    ))
    .expect("entry_guard.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        entry_guard.contains("MFB_INVOKER_ENV")
            && entry_guard.contains("refusing shell-wrapped invocation")
            && entry_guard.contains("CONFIG_CONSUMERS.md"),
        "Rust entry_guard must enforce MFB_INVOKER and reject shell wrappers"
    );
    let mfb_entry_guard = fs::read_to_string(root.join("crates/dev/scripts/mfb_entry_guard.py"))
        .expect("mfb_entry_guard.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        mfb_entry_guard.contains("guard_main")
            && mfb_entry_guard.contains("run_delegated")
            && mfb_entry_guard.contains("SCRIPT_INVOKERS")
            && mfb_entry_guard.contains("shell_wrapper_in_ancestry"),
        "Python mfb_entry_guard must be the shared script entry gate"
    );
    let mfb_config = fs::read_to_string(root.join("crates/dev/scripts/mfb_config_load.py"))
        .expect("mfb_config_load.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        mfb_config.contains("_consumer") && mfb_config.contains("expected_consumer"),
        "JSON configs must be loaded via mfb_config_load with _consumer validation"
    );
    assert!(
        tier_rs.contains("COMMITTED_TIER_AMBIGUOUS_POLICY")
            && tier_rs.contains("verify_training_tier_for_ingest")
            && tier_rs.contains("MFB_TIER_AMBIGUOUS_POLICY")
            && tier_rs.contains(".zip(resolved_from_rules)"),
        "training tier layer must commit to exclude, verify ingest labels, and require \
         assigned==resolved for tier_consistent"
    );
    let rules = fs::read_to_string(root.join("crates/dev/src/config/training_rules.json"))
        .expect("training_rules.json must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        rules.contains("\"_consumer\": \"run_training.py\"")
            && rules.contains("\"tier_ambiguous_policy\": \"exclude\"")
            && rules.contains("\"logic\": \"ALL\"")
            && rules.contains("\"logic\": \"ANY\"")
            && rules.contains("\"value\": 2.8")
            && rules.contains("\"value\": 7.7")
            && rules.contains("\"value\": 1080")
            && rules.contains("\"value\": 512")
            && tier_rs.contains("HIGH_DIMENSION_ENTROPY_FLOOR")
            && tier_rs.contains("assert_non_animated_static_asset"),
        "committed training rules must declare run_training.py consumer + exclude + tier logic + \
         tightened entropy thresholds"
    );
    assert!(
        tier_rs.contains("HIGH_TIER_LOGIC") && tier_rs.contains("LOW_TIER_LOGIC"),
        "Rust tier evaluator must declare high/low combiners"
    );
    assert!(
        tier_rs.contains("LOW_TIER_LOGIC: TierRuleLogic = TierRuleLogic::Any"),
        "low tier must use ANY combiner (one rule sufficient)"
    );
    assert!(
        tier_rs.contains("HIGH_TIER_LOGIC: TierRuleLogic = TierRuleLogic::Any"),
        "high tier must use ANY combiner (M159 corpus)"
    );
    let run_training = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        run_training.contains("probe.resolved_tier")
            && run_training.contains("MFB_TIER_AMBIGUOUS_POLICY")
            && !run_training.contains("ambiguous→high="),
        "collect must follow Rust resolved_tier, not silent prefer-high"
    );
}

#[test]
fn image_analyzer_warn_detail_uses_symbol_pick() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("macro_rules! warn_detail")
            && content.contains("media_conversion_gate::ui_icon_pick")
            && content.contains("macro_rules! info_detail"),
        "image_analyzer detail macros must route through gate ui_icon_pick (M65/U7)"
    );
    assert!(
        !content.contains("log_detail!(\n                \"⚠️")
            && !content.contains("log_detail!(\n            \"⚠️")
            && !content.contains("log_detail!(\n        \"⚠️")
            && !content.contains("log_detail!(\n                    \"⚠️")
            && !content.contains("log_detail!(\n                    \"ℹ️"),
        "image_analyzer must not embed raw warning/info emoji in log_detail format literals"
    );
}

#[test]
fn database_audit_logs_use_symbol_pick() {
    let root = workspace_root();
    for rel in [
        "crates/foundation/src/database.rs",
        "crates/foundation/src/image_quality_db.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("symbols::pick")
                || content.contains("ui_stderr::line")
                || content.contains("media_conversion_gate::ui_icon_pick"),
            "{rel} audit/tracing user lines must use symbols::pick, ui_icon_pick, or \
             ui_stderr::line (U7/U12/M64)"
        );
        assert!(
            !content.contains("\"📊 LoopIntent DB Check")
                && !content.contains("\"🔬 Quality Regression Fusion")
                && !content.contains("|| \"❌ Failed to calculate adaptive neighbor count\""),
            "{rel} must not use raw emoji in user-facing log format literals"
        );
    }
    let verify = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/bin/verify_db_logging.rs",
    ))
    .expect("verify_db_logging.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        verify.contains("symbols::pick") && !verify.contains("\"✅ DB LOGGING VERIFICATION"),
        "verify_db_logging must not duplicate raw success emoji (log_success already picks)"
    );
}

#[test]
fn progress_mode_stats_and_skip_lines_use_symbol_pick() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/progress_mode.rs",
    ))
    .expect("progress_mode.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("fn stats_line_prefix")
            && content.contains("media_conversion_gate::ui_icon_pick"),
        "progress_mode stats prefix must use gate ui_icon_pick (M61/U7)"
    );
    assert!(
        content.contains("fn format_xmp_jxl_images_line")
            && content.contains("let ok_mark = crate::media_conversion_gate::ui_icon_pick"),
        "format_xmp_jxl_images_line must use gate ui_icon_pick for ok/fail marks (M61/U7)"
    );
    assert!(
        !content.contains("⏭️  {}  {}{}{} — {}"),
        "skip/ignore stderr must not use raw skip emoji format literal"
    );
}

#[test]
fn app_error_user_messages_use_symbol_pick() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/app_error.rs",
    ))
    .expect("app_error.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("fn user_err_msg") && content.contains("ui_user_facing_error"),
        "AppError user_message must route through gate helpers (U7/M53)"
    );
    assert!(
        !content.contains("format!(\"❌ File not found"),
        "app_error must not use raw error emoji literals"
    );
}

#[test]
fn stderr_adjacent_paths_use_ui_stderr_or_symbol_pick() {
    let root = workspace_root();
    let copier = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/smart_file_copier.rs",
    ))
    .expect("smart_file_copier.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        copier.contains("ui_stderr::line")
            && !copier.contains("emit_stderr(&format!(\"⚠️")
            && !copier.contains("emit_stderr(&format!(\"   📋"),
        "smart_file_copier verbose paths must use ui_stderr (U7)"
    );
    let logs = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/static_logs.rs",
    ))
    .expect("static_logs.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        logs.contains("fn plain_aware_detail") && logs.contains("get_symbol_by_label"),
        "static_logs must provide plain_aware_detail + label symbols (U8)"
    );
    let ffmpeg = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/ffmpeg_process.rs",
    ))
    .expect("ffmpeg_process.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        ffmpeg.contains("media_conversion_gate::ui_icon_pick")
            && !ffmpeg.contains("writeln!(f, \"❌ FFMPEG ERROR\")"),
        "FfmpegError display must use gate ui_icon_pick (M60/U7)"
    );
    let msssim = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/msssim_progress.rs",
    ))
    .expect("msssim_progress.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        msssim.contains("media_conversion_gate::ui_icon_pick")
            && !msssim.contains("format!(\"❌ Failed"),
        "msssim_progress errors must use gate ui_icon_pick (M60/U7)"
    );
    let meta = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/metadata/mod.rs",
    ))
    .expect("metadata/mod.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        meta.contains("ui_stderr::line") && !meta.contains("⚠️  XMP merge skipped"),
        "XMP skip must use ui_stderr (U7)"
    );
}

#[test]
fn jxl_utils_retry_status_uses_styled_helpers() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/jxl_utils.rs",
    ))
    .expect("jxl_utils.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("styled_ok_fail_label"),
        "JXL retry success/fail must use styled_ok_fail_label (U7)"
    );
    assert!(
        !content.contains(r#"style("✅")"#) && !content.contains(r#"style("❌")"#),
        "jxl_utils must not use raw style(✅/❌) for attempt status"
    );
}

#[test]
fn date_analysis_print_uses_ui_stderr_sections() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/date_analysis.rs",
    ))
    .expect("date_analysis.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("ui_stderr::section"),
        "date_analysis must use ui_stderr::section for headers (U7)"
    );
    assert!(
        !content.contains("emit_stderr(\"\\n📋"),
        "date_analysis must not use raw clipboard emoji in emit_stderr"
    );
}

#[test]
fn training_bins_use_ui_stderr_not_raw_emoji() {
    let root = workspace_root();
    for rel in [
        "crates/foundation/src/bin/train_knn.rs",
        "crates/foundation/src/bin/recompute_stats.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|e| panic!("{rel}: {e}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("ui_stderr::line"),
            "{rel} must use ui_stderr::line"
        );
        assert!(
            content.contains("configure_terminal_ux"),
            "{rel} must call configure_terminal_ux at startup"
        );
    }
}

#[test]
fn mfb_ui_tokens_defines_brand_blue_and_no_color() {
    let root = workspace_root();
    let content = fs::read_to_string(root.join("crates/dev/scripts/mfb_ui_tokens.py"))
        .expect("mfb_ui_tokens.py must exist"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for required in ["BRAND_BLUE", "#43a0ff", "def colors_enabled", "NO_COLOR"] {
        assert!(
            content.contains(required),
            "mfb_ui_tokens.py must define `{required}` (U10)"
        );
    }
}

#[test]
fn gpu_coarse_search_quality_check_formatter_is_diagnostic_only() {
    let root = workspace_root();
    let content = fs::read_to_string(join_legacy_aware(
        &root,
        "crates/foundation/src/video_explorer/gpu_coarse_search.rs",
    ))
    .expect("gpu_coarse_search.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("pipeline_acceptable"),
        "gpu_coarse_search must reference pipeline_acceptable for exploration decisions"
    );
    assert!(
        !content.contains("quality_passed.is_passed() ||"),
        "gpu_coarse_search must not use quality_passed || size_target OR gate for decisions"
    );
}

#[test]
fn python_production_scripts_declare_guard_main() {
    let root = workspace_root();
    let guard_py = fs::read_to_string(root.join("crates/dev/scripts/mfb_entry_guard.py"))
        .expect("mfb_entry_guard.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let marker = "PRODUCTION_GUARDED_SCRIPTS";
    let start = guard_py
        .find(marker)
        .expect("mfb_entry_guard must define PRODUCTION_GUARDED_SCRIPTS"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let block = &guard_py[start..start + 800];
    for script in [
        "run_training.py",
        "training_pipeline.py",
        "quality_regression_model.py",
    ] {
        assert!(
            block.contains(&format!("\"{script}\"")),
            "PRODUCTION_GUARDED_SCRIPTS must list {script}"
        );
        let content = fs::read_to_string(root.join("crates/dev/scripts").join(script))
            .unwrap_or_else(|err| panic!("read {script}: {err:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("guard_main("),
            "{script} must call guard_main() at entry"
        );
    }
    let drag_source = root.join("crates/dev/scripts/archive/drag_and_drop_processor.py");
    let content = fs::read_to_string(&drag_source)
        .unwrap_or_else(|err| panic!("read {}: {err:?}", drag_source.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        content.contains("guard_main("),
        "archived drag_and_drop_processor.py must retain guard_main() for parity reference"
    );
}

#[test]
fn py2bin_overlap_python_sources_stay_until_parity_is_proven() {
    let root = workspace_root();
    let overlaps = [
        "ci/build_libdispatch.py",
        "ci/clippy_strict.py",
        "ci/download_gnu_mpc.py",
        "ci/just_fix_gate.py",
        "run_training.py",
    ];

    for script in overlaps {
        assert!(
            root.join("crates/dev/scripts").join(script).is_file(),
            "py2bin source must remain until Rust parity is explicitly proven: {script}"
        );
    }
    assert!(
        root.join("crates/dev/scripts/archive/drag_and_drop_processor.py")
            .is_file(),
        "py2bin archived source must remain until Rust parity is explicitly proven: \
         drag_and_drop_processor.py"
    );

    let drag_rs = fs::read_to_string(root.join("crates/dev/src/bin/drag_and_drop_processor.rs"))
        .expect("drag_and_drop_processor.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        drag_rs.contains("drag_and_drop_processor.py")
            && drag_rs.contains("rich_panel")
            && drag_rs.contains("print_runtime_panel")
            && drag_rs.contains("print_summary_report"),
        "drag_and_drop_processor.rs must implement Rich-style panels while keeping py compat \
         reference"
    );

    let run_training_rs = fs::read_to_string(root.join("crates/dev/src/bin/run_training.rs"))
        .expect("run_training.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for marker in [
        "detach_current_process",
        "BackgroundPidGuard",
        "run_training.py",
    ] {
        assert!(
            run_training_rs.contains(marker),
            "run_training.rs must implement background detach with py compat reference: {marker}"
        );
    }
    assert!(
        !run_training_rs.contains("--background detach not yet implemented"),
        "run_training.rs must not keep stale background stub after detach is implemented"
    );
}

#[test]
fn training_pipeline_execute_paths_are_fail_closed() {
    let root = workspace_root();
    let content = fs::read_to_string(root.join("crates/dev/scripts/run_training.py"))
        .expect("run_training.py must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    for required in [
        "training_entry_guard::assert_train_quality_entry",
        "training_entry_guard::assert_train_knn_entry",
        "guard_main",
        "run_delegated",
        "run_rust_ingest",
        "invoke_script",
        "rust_ingest_env",
        "assert_pipeline_tool_entry",
        "assert_dev_tool_entry",
        "MFB_TRAINING_INVOKER",
        "refusing shell-wrapped",
        "if total_fail > 0:",
        "refusing to report clean pipeline success",
        "return 1",
        "if total_success == 0:",
        "Training ingest produced zero successful samples",
        "return 2",
        "if args.execute:",
        "if args.ingest:",
        "raise SystemExit(exit_code)",
        "raise SystemExit(2)",
    ] {
        assert!(
            content.contains(required),
            "run_training.py must fail closed for execute/ingest failures; missing {required}"
        );
    }
}

#[test]
fn processing_walks_do_not_silently_drop_directory_errors() {
    let root = workspace_root();
    for rel in [
        "crates/dev/src/bin/index_gallery.rs",
        "crates/vid/src/animated_image.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, rel))
            .unwrap_or_else(|err| panic!("read {rel}: {err:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        for forbidden in [
            ".filter_map(std::result::Result::ok)",
            ".filter_map(core::result::Result::ok)",
            concat!(
                ".filter_map(|e: core::result::Result<walkdir::DirEntry, walkdir::Error>| e.",
                "ok",
                "())"
            ),
        ] {
            assert!(
                !content.contains(forbidden),
                "{rel} must propagate or count walk/read_dir errors instead of dropping them with \
                 {forbidden}"
            );
        }
    }
}

fn assert_constraint_install_is_precise(schema_text: &str, table: &str, constraint: &str) {
    let start = schema_text
        .find(&format!("WHERE conname = '{constraint}'"))
        .unwrap_or_else(|| panic!("{constraint} guard missing")); // audited: contract test assertion path; panic/expect is test-only failure signal
    let guard = &schema_text[start..];
    let end = guard
        .find("END IF;")
        .unwrap_or_else(|| panic!("{constraint} guard is unterminated")); // audited: contract test assertion path; panic/expect is test-only failure signal
    let guard = &guard[..end];

    assert!(
        guard.contains(&format!("AND conrelid = '{table}'::regclass")),
        "{constraint} must be scoped to {table}, not just matched by name"
    );
    assert!(
        guard.contains("NOT VALID"),
        "{constraint} must install NOT VALID and rely on explicit VALIDATE CONSTRAINT for old rows"
    );
    assert!(
        schema_text.contains(&format!("VALIDATE CONSTRAINT {constraint}")),
        "{constraint} must be explicitly validated after installation"
    );
}

#[test]
fn ci_quality_workflow_runs_fail_loud_strict_clippy() {
    let root = workspace_root();
    let workflow = fs::read_to_string(root.join(".github/workflows/ci-quality.yml"))
        .expect("ci-quality workflow must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let clippy_script = fs::read_to_string(root.join("crates/dev/src/bin/clippy_strict.rs"))
        .expect("clippy_strict.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal

    let toolchain = fs::read_to_string(root.join("rust-toolchain.toml"))
        .expect("rust-toolchain.toml must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        toolchain.contains("nightly-"),
        "rust-toolchain.toml must pin a dated nightly channel"
    );
    assert!(
        workflow.contains("dtolnay/rust-toolchain@v1"),
        "ci-quality workflow must install Rust via dtolnay/rust-toolchain (reads pinned toolchain \
         file)"
    );
    let check_all = fs::read_to_string(root.join("crates/dev/src/bin/check_all.rs"))
        .expect("check_all.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        workflow.contains("--bin check_all") && workflow.contains("--ci"),
        "ci-quality health-check must delegate to check_all --ci (expensive SSOT)"
    );
    assert!(
        workflow.contains("cargo-llvm-cov"),
        "ci-quality workflow must install cargo-llvm-cov for check_all coverage"
    );
    assert!(
        check_all.contains("\"clippy_strict\""),
        "check_all must invoke ultra-strict clippy (not duplicate cargo clippy in workflow)"
    );
    assert!(
        check_all.contains("foundation/ci-static-build"),
        "check_all --ci must enable ci-static-build for libheif on runners"
    );
    assert!(
        check_all.contains("\"test\"") && check_all.contains("\"--workspace\""),
        "check_all --ci must run dev contract registry tests"
    );
    assert!(
        check_all.contains("\"test\"") && check_all.contains("\"--workspace\""),
        "check_all --ci must run headless GIF ffmpeg/runtime probe regression (M238)"
    );
    assert!(
        check_all.contains("\"test\"") && check_all.contains("\"--workspace\""),
        "check_all --ci must run WebP/APNG/HEIC/JXL/AVIF runtime probe regression (M241–M243)"
    );
    assert!(
        workflow.contains("--bin download_gnu_mpc"),
        "ci-quality must use mirror-aware Rust MPC download bin (M241)"
    );
    assert!(
        check_all.contains("\"doc\"") && check_all.contains("\"-D warnings\""),
        "check_all --ci must run foundation rustdoc with -D warnings"
    );
    assert!(
        check_all.contains("\"lcov.info\""),
        "check_all --ci must fail-closed when lcov.info is missing"
    );
    assert!(
        workflow.contains("test -s lcov.info"),
        "ci-quality must verify lcov.info before upload-artifact"
    );
    assert!(
        workflow.contains("timeout-minutes: 120"),
        "health-check must allow >60m for check_all --ci (llvm-cov + hack matrix)"
    );
    assert!(
        workflow.contains("NODE_OPTIONS"),
        "ci-quality health-check must suppress Node deprecation noise"
    );
    let justfile = fs::read_to_string(root.join("justfile")).expect("justfile must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        justfile.contains("fix:") && justfile.contains("check:") && justfile.contains("fix-gate:"),
        "justfile must define fix, check, and fix-gate recipes"
    );
    let just_fix_gate = fs::read_to_string(root.join("crates/dev/src/bin/just_fix_gate.rs"))
        .expect("just_fix_gate.rs must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    assert!(
        just_fix_gate.contains("\"just\"") && just_fix_gate.contains("\"check\""),
        "just_fix_gate.rs must delegate to just check"
    );
    assert!(
        workflow.contains("extractions/setup-just@v4"),
        "ci-quality must use setup-just@v4 (composite; avoids deprecated Node 20 runtime)"
    );
    assert!(
        !workflow.contains("extractions/setup-just@v2"),
        "ci-quality must not pin setup-just@v2 (Node 20 deprecation on runners)"
    );
    assert!(
        workflow.contains("just fmt-check"),
        "ci-quality health-check must run early just fmt-check"
    );
    assert!(
        workflow.contains("just fix-gate"),
        "ci-quality health-check must run just fix-gate before check_all --ci"
    );
    assert!(
        workflow.contains("just --list"),
        "ci-quality validation must verify justfile recipes"
    );
    assert!(
        clippy_script.contains("\"clippy\"")
            && clippy_script.contains("\"--workspace\"")
            && clippy_script.contains("\"--all-targets\"")
            && clippy_script.contains("\"--all-features\"")
            && clippy_script.contains("foundation/ci-static-build"),
        "clippy_strict.rs must run strict clippy with CI embedded libheif on GITHUB_ACTIONS"
    );
    assert!(
        clippy_script.contains("\"-D\"") && clippy_script.contains("\"warnings\""),
        "clippy_strict.rs must treat warnings as errors"
    );

    for forbidden in [
        "clippy crashed or failed",
        "exit 0",
        "cargo clippy --workspace --all-targets -- -D warnings",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "ci-quality workflow still contains softened clippy behavior: {forbidden}"
        );
    }
}

#[test]
fn release_packaging_does_not_swallow_copy_failures() {
    let root = workspace_root();
    let release = fs::read_to_string(root.join(".github/workflows/cd-stable.yml"))
        .expect("stable release workflow must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let offenders: Vec<_> = release
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("cp ") && line.contains("|| true"))
        .map(|(idx, line)| {
            format!(
                ".github/workflows/cd-stable.yml:{}: {}",
                idx + 1,
                line.trim()
            )
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "release packaging copy steps must fail loudly:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn obsolete_blocking_exit_guard_is_not_present() {
    let root = workspace_root();
    assert!(
        !root.join("scripts/terminal_exit_guard.py").exists(),
        "terminal_exit_guard.py reintroduces blocking GUI exit confirmation"
    );
    assert!(
        !root
            .join("crates/.modern_format_boost/crates/.modern_format_boost/.tmp_lib/libstdc++.tbd")
            .exists(),
        "tracked .tmp_lib stubs are CI scratch artifacts, not source"
    );
}

#[test]
fn audit_tests_are_real_harness_tests() {
    let root = workspace_root();
    for audit_file in [
        "crates/dev/src/tests/test_real_silent_fallbacks.rs",
        "crates/dev/src/tests/test_silent_numeric_fallbacks.rs",
    ] {
        let content = fs::read_to_string(join_legacy_aware(&root, audit_file))
            .unwrap_or_else(|err| panic!("read {audit_file}: {err:?}")); // audited: contract test assertion path; panic/expect is test-only failure signal
        assert!(
            content.contains("#[test]"),
            "{audit_file} must contain real Cargo test functions"
        );
        let old_always_passes_phrase = ["always", " passes"].concat();
        let old_report_only_phrase = ["check output", " for details"].concat();
        assert!(
            !content.contains(&old_always_passes_phrase)
                && !content.contains(&old_report_only_phrase),
            "{audit_file} must not be a report-only pseudo-test"
        );
    }
}

#[test]
fn dev_test_targets_are_not_zero_test_placeholders() {
    let root = workspace_root();
    let test_dir = root.join("crates/dev/src/tests");
    let mut offenders = Vec::new();

    let entries = fs::read_dir(&test_dir).expect("dev tests readable"); // audited: test
    for entry in entries {
        // audited: test
        let entry = entry.expect("dev test directory entry must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|ext| ext != std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let content = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        if !content.contains("#[test]") && !content.contains("proptest!") {
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            offenders.push(rel.display().to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "dev integration test targets must contain real tests or move to src/bin:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn dev_test_targets_do_not_install_success_printing_main_wrappers() {
    let root = workspace_root();
    let test_dir = root.join("crates/dev/src/tests");
    let main_signature = ["fn ", "main()"].concat();
    let mut offenders = Vec::new();

    let entries = fs::read_dir(&test_dir).expect("dev tests readable"); // audited: test
    for entry in entries {
        // audited: test
        let entry = entry.expect("dev test directory entry must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|ext| ext != std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let content = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        if content.contains(&main_signature) {
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            offenders.push(rel.display().to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "Cargo integration tests must not use success-printing main wrappers:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn dev_tests_do_not_replace_assertions_with_contract_comments() {
    let root = workspace_root();
    let test_dir = root.join("crates/dev/src/tests");
    let forbidden = [
        ["For now, this is a ", "structural test"].concat(),
        ["Pseudocode for ", "actual test"].concat(),
        ["This test documents ", "the requirement"].concat(),
        ["Expected ", "behavior:"].concat(),
    ];
    let mut offenders = Vec::new();

    let entries = fs::read_dir(&test_dir).expect("dev tests readable"); // audited: test
    for entry in entries {
        // audited: test
        let entry = entry.expect("dev test directory entry must be readable"); // audited: contract test assertion path; panic/expect is test-only failure signal
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|ext| ext != std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let content = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        for phrase in &forbidden {
            if content.contains(phrase) {
                let rel = path.strip_prefix(&root).unwrap_or(&path);
                offenders.push(format!("{}: {phrase}", rel.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "dev tests must assert behavior instead of carrying comment-only contracts:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn rust_probe_parse_residue_targets_are_absent_across_crates() {
    fn visit_rs_files(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        let entries = fs::read_dir(path)
            .unwrap_or_else(|err| panic!("read directory {}: {err:?}", path.display()));
        for entry_result in entries {
            let entry = entry_result.unwrap_or_else(|err| {
                panic!("read directory entry under {}: {err:?}", path.display())
            });
            let child = entry.path();
            if child.is_dir() {
                visit_rs_files(&child, files);
                continue;
            }
            if child.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                files.push(child);
            }
        }
    }

    fn line_has_forbidden_residue(line: &str) -> bool {
        let ok_gate = ["if let ", "Ok", "("].concat();
        let ok_chain_gate = ["&& let ", "Ok", "("].concat();
        let dropped_result = [".", "ok", "()"].concat();
        let ok_and = ["is_", "ok", "_and", "("].concat();
        let wildcard_err = ["Err", "(_)"].concat();
        line.contains(&ok_gate)
            || line.contains(&ok_chain_gate)
            || line.contains(&dropped_result)
            || line.contains(&ok_and)
            || (line.contains(&wildcard_err)
                && (line.contains("=> false")
                    || line.contains("=> None")
                    || line.contains("=> continue")))
    }

    let root = workspace_root();
    let crates_dir = root.join("crates");
    let mut files = Vec::new();
    visit_rs_files(&crates_dir, &mut files);

    let mut offenders = Vec::new();
    for file in files {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display()));
        for (line_no, line) in content.lines().enumerate() {
            if line_has_forbidden_residue(line) {
                let rel = file.strip_prefix(&root).unwrap_or(&file);
                offenders.push(format!("{}:{}:{line}", rel.display(), line_no + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "Rust source must not silently discard probe/read/parse errors via target residue \
         forms:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn production_media_cleanup_does_not_drop_safe_remove_file_results() {
    let root = workspace_root();
    let files = workspace_crate_production_rust_files(&root, &["img", "foundation", "vid"]);
    let forbidden = [
        "let _ = foundation::io_utils::safe_remove_file",
        "let _ = crate::io_utils::safe_remove_file",
        "let _ = crate::media_conversion_gate::delivery_rename_or_audit",
        "let _ = foundation::media_conversion_gate::delivery_rename_or_audit",
    ];
    let mut offenders = Vec::new();

    for file in files {
        let content = fs::read_to_string(&file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let production = production_scope(&content);
        for (line_no, line) in production.lines().enumerate() {
            if forbidden.iter().any(|pattern| line.contains(pattern)) {
                let rel = file.strip_prefix(&root).unwrap_or(&file);
                offenders.push(format!("{}:{}:{line}", rel.display(), line_no + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "Production media cleanup must use audited cleanup helpers instead of dropping \
         safe_remove_file results:\n{}",
        offenders.join("\n")
    );
}

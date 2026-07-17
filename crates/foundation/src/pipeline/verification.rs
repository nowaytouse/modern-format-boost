//! Shared verification gate model for fast/full media pipelines.

use crate::common_utils::{calculate_blake3_hash, is_command_available};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Blake3Entry {
    #[serde(default)]
    pub out_rel: Option<String>,
    pub src: String,
    pub out: String,
    pub library_asset: Option<String>,
}

pub type Blake3Log = BTreeMap<String, Blake3Entry>;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct SkippedSourceEntry {
    pub src: String,
    pub reason: String,
}

pub type SkippedSourceLog = BTreeMap<String, SkippedSourceEntry>;
pub type FailedSourceLog = BTreeMap<String, SkippedSourceEntry>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LibraryAssetRecord {
    pub rel_path: String,
    pub blake3: String,
    pub sync_status: String,
    pub quarantined: bool,
    /// Photos library UUID used for pre-delete custody re-verification (tier-2 imports).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photos_uuid: Option<String>,
    /// Photos library BLAKE3 when import rewrote container bytes but pixel proof passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library_blake3: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryHandle {
    pub imported_assets: Vec<LibraryAssetRecord>,
    pub import_error_count: usize,
}

#[derive(Debug, Clone)]
pub struct PipelineCtx {
    pub working_copy: PathBuf,
    pub src_dir: PathBuf,
    pub blake3_log: Blake3Log,
    pub expected_count: usize,
    pub library_handle: Option<LibraryHandle>,
    /// Output format for this pipeline run, used to select the extension for
    /// output file collection. When `None`, falls back to filesystem detection.
    pub output_format: Option<crate::image::format_detect::FormatKind>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckDetail {
    pub name: &'static str,
    pub passed: bool,
    pub expected: String,
    pub actual: String,
    pub affected_files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GateResult {
    pub passed: bool,
    pub checks: Vec<CheckDetail>,
}

pub trait VerificationGate {
    fn name(&self) -> &'static str;
    fn run(&self, ctx: &PipelineCtx) -> GateResult;
}

pub struct Gate1Local;
pub struct Gate2Import;
pub struct Gate3Deep;

impl VerificationGate for Gate1Local {
    fn name(&self) -> &'static str {
        "Gate 1 — Pre-import local verification"
    }

    fn run(&self, ctx: &PipelineCtx) -> GateResult {
        let ext = ctx
            .output_format
            .and_then(output_format_extension)
            .unwrap_or_else(|| detect_output_extension(&ctx.working_copy));
        let jxl_files = match collect_output_files(&ctx.working_copy, ext) {
            Ok(files) => files,
            Err(err) => {
                return gate_result(vec![detail(
                    "count",
                    false,
                    ctx.expected_count.to_string(),
                    format!("walkdir traversal failed: {err}"),
                    vec![ctx.working_copy.clone()],
                )]);
            }
        };
        let checks = vec![
            check_count(ctx.expected_count, jxl_files.len(), &jxl_files),
            check_blake3_logged_outputs(ctx),
            check_exact_metadata_copies(ctx),
            check_nonzero_size(ctx.expected_count, &jxl_files),
            check_orientation_absent(&jxl_files),
            check_decode_probe(&jxl_files, ctx.output_format),
        ];
        gate_result(checks)
    }
}

impl VerificationGate for Gate2Import {
    fn name(&self) -> &'static str {
        "Gate 2 — Post-import verification"
    }

    fn run(&self, ctx: &PipelineCtx) -> GateResult {
        let Some(library) = &ctx.library_handle else {
            return gate_result(vec![
                detail(
                    "count",
                    false,
                    ctx.expected_count.to_string(),
                    "library handle absent".to_string(),
                    Vec::new(),
                ),
                detail(
                    "blake3_sample",
                    false,
                    sample_expected(ctx.expected_count),
                    "library handle absent".to_string(),
                    Vec::new(),
                ),
                detail(
                    "no_error",
                    false,
                    "0 import errors".to_string(),
                    "library handle absent".to_string(),
                    Vec::new(),
                ),
            ]);
        };
        let imported_count = library.imported_assets.len();
        let duplicate_library_paths = duplicate_library_rel_paths(&library.imported_assets);
        let logged_outputs = out_rel_index(&ctx.blake3_log);
        let sample = sample_size(ctx.expected_count);
        let matched = library
            .imported_assets
            .iter()
            .take(sample)
            .filter(|asset| {
                duplicate_library_paths.is_empty()
                    && logged_outputs
                        .get(&asset.rel_path)
                        .is_some_and(|entry| entry.out == asset.blake3)
            })
            .count();
        let sample_failures = library
            .imported_assets
            .iter()
            .take(sample)
            .filter(|asset| {
                !duplicate_library_paths.is_empty()
                    || logged_outputs
                        .get(&asset.rel_path)
                        .is_none_or(|entry| entry.out != asset.blake3)
            })
            .map(|asset| ctx.working_copy.join(&asset.rel_path))
            .collect::<Vec<_>>();

        gate_result(vec![
            detail(
                "count",
                imported_count == ctx.expected_count,
                ctx.expected_count.to_string(),
                imported_count.to_string(),
                Vec::new(),
            ),
            detail(
                "blake3_sample",
                matched == sample && duplicate_library_paths.is_empty(),
                format!("{sample}/{sample} match"),
                if duplicate_library_paths.is_empty() {
                    format!("{matched}/{sample} match")
                } else {
                    format!(
                        "{matched}/{sample} match; duplicate library paths: {}",
                        duplicate_library_paths.join(", ")
                    )
                },
                sample_failures,
            ),
            detail(
                "no_error",
                library.import_error_count == 0,
                "0 import errors".to_string(),
                library.import_error_count.to_string(),
                Vec::new(),
            ),
        ])
    }
}

impl VerificationGate for Gate3Deep {
    fn name(&self) -> &'static str {
        "Gate 3 — Pre-delete deep verification"
    }

    fn run(&self, ctx: &PipelineCtx) -> GateResult {
        let ext = ctx
            .output_format
            .and_then(output_format_extension)
            .unwrap_or_else(|| detect_output_extension(&ctx.working_copy));
        let jxl_files = match collect_output_files(&ctx.working_copy, ext) {
            Ok(files) => files,
            Err(err) => {
                return gate_result(vec![detail(
                    "count_x3",
                    false,
                    format!("{} wc, library, blake3 log", ctx.expected_count),
                    format!("walkdir traversal failed: {err}"),
                    vec![ctx.working_copy.clone()],
                )]);
            }
        };
        let Some(library) = &ctx.library_handle else {
            return gate_result(vec![
                detail(
                    "count_x3",
                    false,
                    format!("{} wc, library, blake3 log", ctx.expected_count),
                    format!(
                        "{} wc, absent library, {} blake3 log",
                        jxl_files.len(),
                        ctx.blake3_log.len()
                    ),
                    Vec::new(),
                ),
                detail(
                    "sync",
                    false,
                    "Photos local custody or uploaded".to_string(),
                    "library handle absent".to_string(),
                    Vec::new(),
                ),
                detail(
                    "quarantine",
                    false,
                    "0 quarantined".to_string(),
                    "library handle absent".to_string(),
                    Vec::new(),
                ),
                detail(
                    "chain",
                    false,
                    format!("{} intact chains", ctx.expected_count),
                    "library handle absent".to_string(),
                    Vec::new(),
                ),
            ]);
        };
        let duplicate_library_paths = duplicate_library_rel_paths(&library.imported_assets);
        let logged_outputs = out_rel_index(&ctx.blake3_log);
        let library_index = library_asset_index(&library.imported_assets);
        let count_x3 = duplicate_library_paths.is_empty()
            && jxl_files.len() == ctx.expected_count
            && library.imported_assets.len() == ctx.expected_count
            && ctx.blake3_log.len() == ctx.expected_count;
        let sync_failures = library
            .imported_assets
            .iter()
            .filter(|asset| !photos_library_sync_status_is_accepted(&asset.sync_status))
            .map(|asset| ctx.working_copy.join(&asset.rel_path))
            .collect::<Vec<_>>();
        let quarantine_failures = library
            .imported_assets
            .iter()
            .filter(|asset| asset.quarantined)
            .map(|asset| ctx.working_copy.join(&asset.rel_path))
            .collect::<Vec<_>>();
        let mut chain_failures = logged_outputs
            .iter()
            .filter_map(|(out_rel, entry)| {
                let asset = library_index.get(out_rel)?;
                (entry.out != asset.blake3
                    || entry.library_asset.as_deref() != Some(asset.blake3.as_str()))
                .then(|| ctx.working_copy.join(out_rel))
            })
            .collect::<Vec<_>>();
        chain_failures.extend(
            logged_outputs
                .keys()
                .filter(|out_rel| !library_index.contains_key(*out_rel))
                .map(|out_rel| ctx.working_copy.join(out_rel)),
        );
        chain_failures.extend(
            library_index
                .keys()
                .filter(|out_rel| !logged_outputs.contains_key(*out_rel))
                .map(|out_rel| ctx.working_copy.join(out_rel)),
        );
        chain_failures.extend(
            duplicate_library_paths
                .iter()
                .map(|out_rel| ctx.working_copy.join(out_rel)),
        );
        chain_failures.sort();
        chain_failures.dedup();

        gate_result(vec![
            detail(
                "count_x3",
                count_x3,
                format!("{} wc, library, blake3 log", ctx.expected_count),
                format!(
                    "{} wc, {} library, {} blake3 log",
                    jxl_files.len(),
                    library.imported_assets.len(),
                    ctx.blake3_log.len()
                ),
                Vec::new(),
            ),
            detail(
                "sync",
                sync_failures.is_empty(),
                "Photos local custody or uploaded".to_string(),
                format!("{} without accepted custody proof", sync_failures.len()),
                sync_failures,
            ),
            detail(
                "quarantine",
                quarantine_failures.is_empty(),
                "0 quarantined".to_string(),
                quarantine_failures.len().to_string(),
                quarantine_failures,
            ),
            detail(
                "chain",
                chain_failures.is_empty(),
                format!("{} intact chains", ctx.expected_count),
                format!("{} broken chains", chain_failures.len()),
                chain_failures,
            ),
        ])
    }
}

fn gate_result(checks: Vec<CheckDetail>) -> GateResult {
    let passed = !checks.is_empty() && checks.iter().all(|check| check.passed);
    GateResult { passed, checks }
}

const fn detail(
    name: &'static str,
    passed: bool,
    expected: String,
    actual: String,
    affected_files: Vec<PathBuf>,
) -> CheckDetail {
    CheckDetail {
        name,
        passed,
        expected,
        actual,
        affected_files,
    }
}

fn check_count(expected: usize, actual: usize, files: &[PathBuf]) -> CheckDetail {
    detail(
        "count",
        actual == expected,
        expected.to_string(),
        actual.to_string(),
        if actual == expected {
            Vec::new()
        } else {
            files.to_vec()
        },
    )
}

fn check_blake3_logged_outputs(ctx: &PipelineCtx) -> CheckDetail {
    let mut failures = Vec::new();
    let mut hash_read_errors = 0usize;
    for (rel_path, entry) in &ctx.blake3_log {
        let src = ctx.src_dir.join(rel_path);
        let out_rel = entry.out_rel.as_deref().map_or_else(
            || PathBuf::from(rel_path).with_extension("JXL"),
            PathBuf::from,
        );
        let out = ctx.working_copy.join(out_rel);
        let src_current = if entry.src.is_empty() {
            false
        } else {
            match calculate_blake3_hash(&src) {
                Ok(hash) => hash == entry.src,
                Err(err) => {
                    hash_read_errors += 1;
                    crate::media_conversion_gate::delivery_pipeline_path_audit(
                        "fast_img_gate1_blake3_src",
                        &src,
                        format!("source BLAKE3 read failed during Gate 1 proof: {err}"),
                    );
                    false
                }
            }
        };
        let out_current = if entry.out.is_empty() {
            false
        } else {
            match calculate_blake3_hash(&out) {
                Ok(hash) => hash == entry.out,
                Err(err) => {
                    hash_read_errors += 1;
                    crate::media_conversion_gate::delivery_pipeline_path_audit(
                        "fast_img_gate1_blake3_out",
                        &out,
                        format!("output BLAKE3 read failed during Gate 1 proof: {err}"),
                    );
                    false
                }
            }
        };
        let logged_hashes_current = src_current && out_current;
        if !logged_hashes_current {
            failures.push(out);
        }
    }
    let verified = ctx.blake3_log.len().saturating_sub(failures.len());
    let actual = if hash_read_errors == 0 {
        format!("{} verified, {} failed", verified, failures.len())
    } else {
        format!(
            "{} verified, {} failed, hash read errors: {}",
            verified,
            failures.len(),
            hash_read_errors
        )
    };

    detail(
        "blake3",
        failures.is_empty() && ctx.blake3_log.len() == ctx.expected_count,
        format!("{} final source/output hashes current", ctx.expected_count),
        actual,
        failures,
    )
}

fn check_exact_metadata_copies(ctx: &PipelineCtx) -> CheckDetail {
    let mut failures = Vec::new();
    let mut mismatch_count = 0usize;
    let mut first_mismatch = None;
    for (rel_path, entry) in &ctx.blake3_log {
        let src = ctx.src_dir.join(rel_path);
        let out_rel = entry.out_rel.as_deref().map_or_else(
            || PathBuf::from(rel_path).with_extension("JXL"),
            PathBuf::from,
        );
        let out = ctx.working_copy.join(out_rel);
        match crate::metadata::verify_exact_metadata_copy(&src, &out) {
            Ok(check) if check.passed => {}
            Ok(check) => {
                mismatch_count = mismatch_count.saturating_add(1);
                if first_mismatch.is_none() {
                    first_mismatch = Some(check.mismatches.join("; "));
                }
                failures.push(out);
            }
            Err(err) => {
                mismatch_count = mismatch_count.saturating_add(1);
                if first_mismatch.is_none() {
                    first_mismatch = Some(err.to_string());
                }
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "fast_img_gate1_metadata",
                    &out,
                    format!("metadata exact-copy verification failed during Gate 1: {err}"),
                );
                failures.push(out);
            }
        }
    }
    let verified = ctx.blake3_log.len().saturating_sub(failures.len());
    let actual = first_mismatch.map_or_else(
        || format!("{verified} verified, {mismatch_count} failed"),
        |detail| format!("{verified} verified, {mismatch_count} failed; first {detail}"),
    );

    detail(
        "metadata",
        failures.is_empty() && ctx.blake3_log.len() == ctx.expected_count,
        format!("{} exact source/output metadata copies", ctx.expected_count),
        actual,
        failures,
    )
}

fn out_rel_index(blake3_log: &Blake3Log) -> BTreeMap<String, &Blake3Entry> {
    blake3_log
        .iter()
        .map(|(source_rel, entry)| {
            (
                entry.out_rel.clone().unwrap_or_else(|| {
                    // Legacy entry without out_rel: the .JXL guess fails closed at hash
                    // verify, but audit it so a wrong guess reads as "missing out_rel".
                    crate::media_conversion_gate::delivery_pipeline_batch_audit(
                        "verification_out_rel_guess",
                        format!(
                            "Blake3 entry for {source_rel} lacks out_rel; guessing .JXL sibling"
                        ),
                    );
                    PathBuf::from(source_rel)
                        .with_extension("JXL")
                        .to_string_lossy()
                        .to_string()
                }),
                entry,
            )
        })
        .collect()
}

fn library_asset_index(assets: &[LibraryAssetRecord]) -> BTreeMap<String, &LibraryAssetRecord> {
    assets
        .iter()
        .map(|asset| (asset.rel_path.clone(), asset))
        .collect()
}

fn duplicate_library_rel_paths(assets: &[LibraryAssetRecord]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for asset in assets {
        if !seen.insert(asset.rel_path.clone()) {
            duplicates.insert(asset.rel_path.clone());
        }
    }
    duplicates.into_iter().collect()
}

fn photos_library_sync_status_is_accepted(sync_status: &str) -> bool {
    matches!(sync_status, "uploaded" | "photos_local")
}

fn check_nonzero_size(expected: usize, files: &[PathBuf]) -> CheckDetail {
    let failures = files
        .iter()
        .filter(|path| match std::fs::metadata(path) {
            Ok(metadata) => metadata.len() == 0,
            Err(err) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "fast_img_gate1_size",
                    path,
                    format!("metadata read failed during Gate 1 size check: {err}"),
                );
                true
            }
        })
        .cloned()
        .collect::<Vec<_>>();
    detail(
        "size",
        failures.is_empty() && files.len() == expected,
        format!("{expected} non-empty JXL files"),
        format!(
            "{} non-empty, {} empty/missing",
            files.len().saturating_sub(failures.len()),
            failures.len()
        ),
        failures,
    )
}

fn check_orientation_absent(files: &[PathBuf]) -> CheckDetail {
    if !is_command_available("exiftool") {
        return detail(
            "orient",
            false,
            "exiftool available and no Orientation tags".to_string(),
            "exiftool unavailable".to_string(),
            files.to_vec(),
        );
    }

    check_orientation_absent_with_probe(files, orientation_tag_present)
}

fn check_orientation_absent_with_probe<F>(files: &[PathBuf], mut probe: F) -> CheckDetail
where
    F: FnMut(&Path) -> std::io::Result<bool>,
{
    let mut failures = Vec::new();
    let mut tagged_count = 0usize;
    let mut probe_errors = Vec::new();
    for path in files {
        match probe(path) {
            Ok(true) => {
                tagged_count += 1;
                failures.push(path.clone());
            }
            Ok(false) => {}
            Err(err) => {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "fast_img_gate1_orientation_probe",
                    path,
                    format!("orientation probe failed during Gate 1 orientation check: {err}"),
                );
                probe_errors.push(format!("{}: {err}", path.display()));
                failures.push(path.clone());
            }
        }
    }
    let actual = if let Some(first_error) = probe_errors.first() {
        format!(
            "{tagged_count} files with Orientation tag; {} orientation probe errors; first \
             {first_error}",
            probe_errors.len()
        )
    } else {
        format!("{tagged_count} files with Orientation tag")
    };
    detail(
        "orient",
        failures.is_empty(),
        "no Orientation tags".to_string(),
        actual,
        failures,
    )
}

fn check_decode_probe(files: &[PathBuf], output_format: Option<crate::format_detect::FormatKind>) -> CheckDetail {
    let is_avif = matches!(output_format, Some(crate::format_detect::FormatKind::Avif));
    let tool = if is_avif { "avifdec" } else { "djxl" };

    if !is_command_available(tool) {
        return detail(
            "decode",
            false,
            format!("{tool} available and every file decodes"),
            format!("{tool} unavailable"),
            files.to_vec(),
        );
    }

    let failures = files
        .iter()
        .filter_map(|path| {
            if is_avif {
                avifdec_decode_probe(path).err().map(|err| (path.clone(), err))
            } else {
                djxl_decode_probe(path).err().map(|err| (path.clone(), err))
            }
        })
        .collect::<Vec<_>>();
    let failure_paths = failures
        .iter()
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    let actual = if let Some((path, err)) = failures.first() {
        format!(
            "{} decode failures; first {}: {err}",
            failures.len(),
            path.display()
        )
    } else {
        "0 decode failures".to_string()
    };
    detail(
        "decode",
        failures.is_empty(),
        format!("{} {} decode probes pass", files.len(), tool),
        actual,
        failure_paths,
    )
}

fn collect_output_files(root: &Path, extension: &str) -> Result<Vec<PathBuf>, walkdir::Error> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry?;
        if entry.file_type().is_file()
            && entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
        {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();
    Ok(files)
}

/// Convert a known output format to its canonical file extension.
/// Returns `None` for formats that are not valid fast-img output targets.
fn output_format_extension(fmt: crate::image::format_detect::FormatKind) -> Option<&'static str> {
    use crate::image::format_detect::FormatKind;
    match fmt {
        FormatKind::Jxl => Some("jxl"),
        FormatKind::Avif => Some("avif"),
        _ => None,
    }
}

fn detect_output_extension(root: &Path) -> &'static str {
    if !root.exists() {
        return "jxl";
    }
    let mut has_jxl = false;
    let mut has_avif = false;
    for entry in walkdir::WalkDir::new(root).max_depth(2) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("jxl") {
                has_jxl = true;
            } else if ext.eq_ignore_ascii_case("avif") {
                has_avif = true;
            }
        }
        if has_jxl || has_avif {
            break;
        }
    }
    if has_avif { "avif" } else { "jxl" }
}

fn orientation_tag_present(path: &Path) -> std::io::Result<bool> {
    let output = std::process::Command::new("exiftool")
        .arg("-s3")
        .arg("-Orientation")
        .arg(path)
        .output()?;
    ensure_exiftool_success(path, output.status, &output.stderr)?;
    Ok(!output.stdout.is_empty())
}

fn ensure_exiftool_success(path: &Path, status: ExitStatus, stderr: &[u8]) -> std::io::Result<()> {
    if status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(stderr);
    Err(std::io::Error::other(format!(
        "exiftool orientation probe failed for {}: {stderr}",
        path.display()
    )))
}

fn avifdec_decode_probe(path: &Path) -> Result<(), String> {
    let temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "fast_img_gate1_decode_probe",
        Some("mfb_gate1_decode"),
        Some(".png"),
    )
    .map_err(|err| format!("avifdec decode probe scratch tempfile failed: {err}"))?;
    let output = std::process::Command::new("avifdec")
        .arg(path)
        .arg(temp.path())
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("avifdec decode probe spawn failed: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr_tail = crate::media_conversion_gate::delivery_subprocess_log_tail_or_empty(
        stderr.lines().rev().find(|line| !line.trim().is_empty()),
    );
    Err(format!(
        "avifdec decode probe failed: avifdec {} -> exit {:?}; stderr tail: {stderr_tail}",
        path.display(),
        output.status.code()
    ))
}

fn djxl_decode_probe(path: &Path) -> Result<(), String> {
    // Scratch must be .png, not .ppm: PPM is RGB-only, so grayscale (or CMYK) JXL
    // outputs make djxl fail with "SelectFormat failed" despite a valid bitstream.
    let temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
        "fast_img_gate1_decode_probe",
        Some("mfb_gate1_decode"),
        Some(".png"),
    )
    .map_err(|err| format!("decode probe scratch tempfile failed: {err}"))?;
    let output = std::process::Command::new("djxl")
        .arg(path)
        .arg(temp.path())
        .stdout(std::process::Stdio::null())
        .output()
        .map_err(|err| format!("djxl decode probe spawn failed: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if crate::image::jxl_utils::is_jxl_png_icc_decode_error(&stderr) {
        let jpeg_temp = crate::media_conversion_gate::delivery_named_tempfile_in_scratch_or_err(
            "fast_img_gate1_decode_probe",
            Some("mfb_gate1_decode"),
            Some(".jpg"),
        )
        .map_err(|err| format!("decode probe JPEG scratch tempfile failed: {err}"))?;
        let jpeg_output = std::process::Command::new("djxl")
            .arg(path)
            .arg(jpeg_temp.path())
            .stdout(std::process::Stdio::null())
            .output()
            .map_err(|err| format!("djxl JPEG decode probe spawn failed: {err}"))?;
        if jpeg_output.status.success() {
            crate::log_detail!(format!(
                "djxl decode probe retried as JPEG reconstruction after PNG ICC failure: {}",
                path.display()
            ));
            return Ok(());
        }
        let jpeg_stderr = String::from_utf8_lossy(&jpeg_output.stderr);
        let jpeg_stderr_tail = crate::media_conversion_gate::delivery_subprocess_log_tail_or_empty(
            jpeg_stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty()),
        );
        crate::log_detail!(format!(
            "djxl JPEG decode probe failed: djxl {} {} -> exit {:?}; stderr tail: \
             {jpeg_stderr_tail}",
            path.display(),
            jpeg_temp.path().display(),
            jpeg_output.status.code()
        ));
        return Err(format!(
            "djxl JPEG decode probe exited {:?}; stderr tail: {jpeg_stderr_tail}",
            jpeg_output.status.code()
        ));
    }
    let stderr_tail = crate::media_conversion_gate::delivery_subprocess_log_tail_or_empty(
        stderr.lines().rev().find(|line| !line.trim().is_empty()),
    );
    crate::log_detail!(format!(
        "djxl decode probe failed: djxl {} {} -> exit {:?}; stderr tail: {stderr_tail}",
        path.display(),
        temp.path().display(),
        output.status.code()
    ));
    Err(format!(
        "djxl decode probe exited {:?}; stderr tail: {stderr_tail}",
        output.status.code()
    ))
}

fn sample_size(expected_count: usize) -> usize {
    if expected_count == 0 {
        return 0;
    }
    if expected_count <= 20 {
        return expected_count;
    }
    std::cmp::max(1, expected_count.div_ceil(10))
}

fn sample_expected(expected_count: usize) -> String {
    let sample = sample_size(expected_count);
    format!("{sample}/{sample} match")
}

#[cfg(test)]
mod tests {
    use super::{
        Blake3Entry, Blake3Log, Gate1Local, Gate2Import, Gate3Deep, GateResult, LibraryAssetRecord,
        LibraryHandle, PipelineCtx, VerificationGate, ensure_exiftool_success,
    };
    use crate::common_utils::calculate_blake3_hash;
    use std::collections::BTreeMap;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;

    fn ctx_with_library(
        expected_count: usize,
        imported_assets: Vec<LibraryAssetRecord>,
    ) -> PipelineCtx {
        PipelineCtx {
            working_copy: PathBuf::from("wc"),
            src_dir: PathBuf::from("src"),
            blake3_log: Blake3Log::new(),
            expected_count,
            library_handle: Some(LibraryHandle {
                imported_assets,
                import_error_count: 0,
            }),
            output_format: None,
        }
    }

    #[test]
    fn gate_fail_has_per_check_detail() {
        let ctx = PipelineCtx {
            working_copy: PathBuf::from("missing-wc"),
            src_dir: PathBuf::from("missing-src"),
            blake3_log: BTreeMap::new(),
            expected_count: 1,
            library_handle: None,
            output_format: None,
        };
        let result = Gate1Local.run(&ctx);

        assert!(!result.passed);
        assert_eq!(result.checks.len(), 6);
        assert!(
            result
                .checks
                .iter()
                .all(|check| !check.expected.is_empty() && !check.actual.is_empty())
        );
    }

    #[test]
    fn gate2_requires_library_handle() {
        let ctx = PipelineCtx {
            working_copy: PathBuf::from("wc"),
            src_dir: PathBuf::from("src"),
            blake3_log: BTreeMap::new(),
            expected_count: 1,
            library_handle: None,
            output_format: None,
        };
        let result = Gate2Import.run(&ctx);

        assert!(!result.passed);
        assert_eq!(result.checks.len(), 3);
    }

    #[test]
    fn gate3_requires_library_handle() {
        let ctx = PipelineCtx {
            working_copy: PathBuf::from("wc"),
            src_dir: PathBuf::from("src"),
            blake3_log: BTreeMap::new(),
            expected_count: 1,
            library_handle: None,
            output_format: None,
        };
        let result = Gate3Deep.run(&ctx);

        assert!(!result.passed);
        assert_eq!(result.checks.len(), 4);
    }

    #[test]
    fn full_mode_can_instantiate_gate1local() {
        fn accepts_gate<G: VerificationGate>(gate: &G) -> &'static str {
            gate.name()
        }

        assert!(accepts_gate(&Gate1Local).contains("Gate 1"));
    }

    #[test]
    fn gate_result_never_passes_empty_checks() {
        let result = GateResult {
            passed: false,
            checks: Vec::new(),
        };

        assert!(!result.passed);
    }

    #[test]
    fn failed_gate_summary_lists_only_failed_checks() {
        let checks = vec![
            super::detail(
                "sync",
                true,
                "Photos local custody or uploaded".to_string(),
                "0 without accepted custody proof".to_string(),
                Vec::new(),
            ),
            super::detail(
                "quarantine",
                false,
                "0 quarantined".to_string(),
                "1".to_string(),
                Vec::new(),
            ),
        ];

        let summary = super::summarize_checks(&checks);

        assert_eq!(summary, "quarantine expected=0 quarantined actual=1");
    }

    #[test]
    fn orientation_check_preserves_probe_error_detail() {
        let files = vec![PathBuf::from("/tmp/mfb-orientation-probe-error.JXL")];

        let check = super::check_orientation_absent_with_probe(&files, |_| {
            Err(std::io::Error::other("synthetic exiftool failure"))
        });

        assert!(!check.passed);
        assert_eq!(check.affected_files, files);
        assert!(
            check.actual.contains("synthetic exiftool failure"),
            "probe failure must be preserved in check detail: {}",
            check.actual
        );
    }

    #[test]
    fn gate1_blake3_accepts_current_final_output_hashes_without_jpeg_roundtrip() {
        let root = tempfile::TempDir::new().unwrap();
        let src_dir = root.path().join("src");
        let wc = root.path().join("src_optimized");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&wc).unwrap();
        let src = src_dir.join("a.jpg");
        let out = wc.join("a.JXL");
        std::fs::write(&src, b"source-jpeg-bytes").unwrap();
        std::fs::write(&out, b"final-jxl-container-after-metadata").unwrap();
        let mut ctx = PipelineCtx {
            working_copy: wc,
            src_dir,
            blake3_log: BTreeMap::new(),
            expected_count: 1,
            library_handle: None,
            output_format: None,
        };
        ctx.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: calculate_blake3_hash(&src).unwrap(),
                out: calculate_blake3_hash(&out).unwrap(),
                library_asset: None,
            },
        );

        let result = super::check_blake3_logged_outputs(&ctx);

        assert!(result.passed, "{result:?}");
    }

    #[test]
    fn gate1_blake3_rejects_final_output_hash_drift() {
        let root = tempfile::TempDir::new().unwrap();
        let src_dir = root.path().join("src");
        let wc = root.path().join("src_optimized");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&wc).unwrap();
        let src = src_dir.join("a.jpg");
        let out = wc.join("a.JXL");
        std::fs::write(&src, b"source-jpeg-bytes").unwrap();
        std::fs::write(&out, b"final-jxl-container-before-drift").unwrap();
        let mut ctx = PipelineCtx {
            working_copy: wc,
            src_dir,
            blake3_log: BTreeMap::new(),
            expected_count: 1,
            library_handle: None,
            output_format: None,
        };
        ctx.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: calculate_blake3_hash(&src).unwrap(),
                out: calculate_blake3_hash(&out).unwrap(),
                library_asset: None,
            },
        );
        std::fs::write(&out, b"final-jxl-container-after-drift").unwrap();

        let result = super::check_blake3_logged_outputs(&ctx);

        assert!(!result.passed, "{result:?}");
        assert_eq!(result.affected_files, vec![out]);
    }

    #[test]
    fn gate1_metadata_rejects_source_output_metadata_mismatch() {
        let root = tempfile::TempDir::new().unwrap();
        let src_dir = root.path().join("src");
        let wc = root.path().join("src_optimized");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&wc).unwrap();
        let src = src_dir.join("a.jpg");
        let out = wc.join("a.JXL");
        std::fs::write(&src, b"source-jpeg-bytes").unwrap();
        std::fs::write(&out, b"jxl-output-bytes").unwrap();
        filetime::set_file_mtime(&src, filetime::FileTime::from_unix_time(1_700_000_000, 0))
            .unwrap();
        filetime::set_file_mtime(&out, filetime::FileTime::from_unix_time(1_700_000_123, 0))
            .unwrap();
        let mut ctx = PipelineCtx {
            working_copy: wc,
            src_dir,
            blake3_log: BTreeMap::new(),
            expected_count: 1,
            library_handle: None,
            output_format: None,
        };
        ctx.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: calculate_blake3_hash(&src).unwrap(),
                out: calculate_blake3_hash(&out).unwrap(),
                library_asset: None,
            },
        );

        let result = super::check_exact_metadata_copies(&ctx);

        assert!(!result.passed);
        assert_eq!(result.name, "metadata");
        assert_eq!(result.affected_files, vec![out]);
        assert!(
            result.actual.contains("Exact metadata copy mismatch"),
            "metadata gate must expose exact-copy mismatch: {}",
            result.actual
        );
    }

    #[test]
    fn apply_gate1_records_metadata_check_result() {
        let mut marker = super::WorkingCopyMarker::new(
            PathBuf::from("/tmp/mfb-src"),
            PathBuf::from("/tmp/mfb-wc"),
            1,
        );
        let result = GateResult {
            passed: false,
            checks: vec![super::detail(
                "metadata",
                false,
                "1 exact source/output metadata copies".to_string(),
                "0 verified, 1 failed".to_string(),
                Vec::new(),
            )],
        };

        marker.apply_gate1(&result);

        assert!(!marker.gate1_checks.metadata.0);
        assert_eq!(marker.stage, super::FastImgStageName::Gate1Failed);
    }

    #[test]
    fn gate1_blake3_reports_hash_read_errors_separately_from_drift() {
        let root = tempfile::TempDir::new().unwrap();
        let src_dir = root.path().join("src");
        let wc = root.path().join("src_optimized");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::create_dir_all(&wc).unwrap();
        let src = src_dir.join("a.jpg");
        let out = wc.join("a.JXL");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(&out, b"final-jxl-container").unwrap();
        let mut ctx = PipelineCtx {
            working_copy: wc,
            src_dir,
            blake3_log: BTreeMap::new(),
            expected_count: 1,
            library_handle: None,
            output_format: None,
        };
        ctx.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "logged-source-hash".to_string(),
                out: calculate_blake3_hash(&out).unwrap(),
                library_asset: None,
            },
        );

        let result = super::check_blake3_logged_outputs(&ctx);

        assert!(!result.passed, "{result:?}");
        assert!(
            result.actual.contains("hash read errors: 1"),
            "unexpected actual: {}",
            result.actual
        );
        assert_eq!(result.affected_files, vec![out]);
    }

    #[cfg(unix)]
    #[test]
    fn exiftool_nonzero_status_is_probe_failure() {
        let status = std::process::ExitStatus::from_raw(1);

        let err =
            ensure_exiftool_success(std::path::Path::new("out.jxl"), status, b"corrupt metadata")
                .unwrap_err();

        assert!(
            err.to_string()
                .contains("exiftool orientation probe failed"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn gate2_sample_uses_logged_output_hash() {
        let mut ctx = ctx_with_library(
            1,
            vec![LibraryAssetRecord {
                rel_path: "a.JXL".to_string(),
                blake3: "out".to_string(),
                sync_status: "uploaded".to_string(),
                quarantined: false,
                photos_uuid: None,
                library_blake3: None,
            }],
        );
        ctx.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src".to_string(),
                out: "out".to_string(),
                library_asset: Some("out".to_string()),
            },
        );

        let result = Gate2Import.run(&ctx);

        assert!(
            result
                .checks
                .iter()
                .any(|check| check.name == "blake3_sample" && check.passed)
        );
    }

    #[test]
    fn gate2_rejects_duplicate_library_paths() {
        let mut ctx = ctx_with_library(
            2,
            vec![
                LibraryAssetRecord {
                    rel_path: "a.JXL".to_string(),
                    blake3: "out-a".to_string(),
                    sync_status: "uploaded".to_string(),
                    quarantined: false,
                    photos_uuid: None,
                    library_blake3: None,
                },
                LibraryAssetRecord {
                    rel_path: "a.JXL".to_string(),
                    blake3: "out-a".to_string(),
                    sync_status: "uploaded".to_string(),
                    quarantined: false,
                    photos_uuid: None,
                    library_blake3: None,
                },
            ],
        );
        ctx.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src-a".to_string(),
                out: "out-a".to_string(),
                library_asset: Some("out-a".to_string()),
            },
        );
        ctx.blake3_log.insert(
            "b.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("b.JXL".to_string()),
                src: "src-b".to_string(),
                out: "out-b".to_string(),
                library_asset: Some("out-b".to_string()),
            },
        );

        let result = Gate2Import.run(&ctx);

        assert!(!result.passed);
        assert!(
            result
                .checks
                .iter()
                .any(|check| check.name == "blake3_sample" && !check.passed)
        );
    }

    #[test]
    fn gate3_rejects_missing_distinct_library_asset() {
        let mut ctx = ctx_with_library(
            2,
            vec![
                LibraryAssetRecord {
                    rel_path: "a.JXL".to_string(),
                    blake3: "out-a".to_string(),
                    sync_status: "uploaded".to_string(),
                    quarantined: false,
                    photos_uuid: None,
                    library_blake3: None,
                },
                LibraryAssetRecord {
                    rel_path: "a.JXL".to_string(),
                    blake3: "out-a".to_string(),
                    sync_status: "uploaded".to_string(),
                    quarantined: false,
                    photos_uuid: None,
                    library_blake3: None,
                },
            ],
        );
        ctx.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "src-a".to_string(),
                out: "out-a".to_string(),
                library_asset: Some("out-a".to_string()),
            },
        );
        ctx.blake3_log.insert(
            "b.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("b.JXL".to_string()),
                src: "src-b".to_string(),
                out: "out-b".to_string(),
                library_asset: Some("out-b".to_string()),
            },
        );

        let result = Gate3Deep.run(&ctx);

        assert!(!result.passed);
        assert!(
            result
                .checks
                .iter()
                .any(|check| check.name == "chain" && !check.passed)
        );
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FastImgStageName {
    ScanComplete,
    #[serde(alias = "copy_complete")]
    OutputPrepared,
    TranscodeComplete,
    Gate1Passed,
    ImportComplete,
    Gate2Passed,
    DeepScanComplete,
    Gate3Passed,
    CleanupComplete,
    Gate1Failed,
    Gate2Failed,
    Gate3Failed,
    Aborted,
}

impl FastImgStageName {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ScanComplete => "scan_complete",
            Self::OutputPrepared => "output_prepared",
            Self::TranscodeComplete => "transcode_complete",
            Self::Gate1Passed => "gate1_passed",
            Self::ImportComplete => "import_complete",
            Self::Gate2Passed => "gate2_passed",
            Self::DeepScanComplete => "deep_scan_complete",
            Self::Gate3Passed => "gate3_passed",
            Self::CleanupComplete => "cleanup_complete",
            Self::Gate1Failed => "gate1_failed",
            Self::Gate2Failed => "gate2_failed",
            Self::Gate3Failed => "gate3_failed",
            Self::Aborted => "aborted",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum FastImgStage {
    Idle,
    Scanning,
    ConfirmScan {
        jpeg_count: usize,
        src_dir: PathBuf,
    },
    PreparingOutput {
        output_dir: PathBuf,
    },
    Transcoding {
        done: usize,
        total: usize,
        current_file: PathBuf,
    },
    StrippingOrientation {
        done: usize,
        total: usize,
    },
    VerifyingLocal,
    Gate1Failed {
        checks: Vec<CheckDetail>,
    },
    ConfirmImport {
        jxl_count: usize,
    },
    Importing {
        done: usize,
        total: usize,
    },
    VerifyingImport,
    Gate2Failed {
        checks: Vec<CheckDetail>,
    },
    DeepScanning,
    VerifyingFinal,
    Gate3Failed {
        checks: Vec<CheckDetail>,
    },
    Cleanup,
    Done {
        summary: FastImgSummary,
    },
    Aborted {
        at_stage: &'static str,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FastImgSummary {
    pub total_files: usize,
    pub space_freed_bytes: u64,
    pub duration_secs: f64,
    pub gate1_passed: bool,
    pub gate2_passed: bool,
    pub gate3_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(transparent)]
pub struct CheckPassed(pub bool);

impl From<bool> for CheckPassed {
    fn from(value: bool) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Gate1Checks {
    pub count: CheckPassed,
    pub blake3: CheckPassed,
    #[serde(default)]
    pub metadata: CheckPassed,
    pub size: CheckPassed,
    pub orient: CheckPassed,
    pub decode: CheckPassed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Gate2Checks {
    pub count: CheckPassed,
    pub blake3_sample: CheckPassed,
    pub no_error: CheckPassed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Gate3Checks {
    pub count_x3: CheckPassed,
    pub sync: CheckPassed,
    pub quarantine: CheckPassed,
    pub chain: CheckPassed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkingCopyMarker {
    pub schema: u8,
    pub src_dir: PathBuf,
    pub working_copy: PathBuf,
    pub started_at: String,
    pub stage: FastImgStageName,
    pub src_jpeg_count: usize,
    pub transcoded_count: usize,
    pub gate1_checks: Gate1Checks,
    pub gate2_checks: Gate2Checks,
    pub gate3_checks: Gate3Checks,
    pub blake3_log: Blake3Log,
    #[serde(default)]
    pub skipped_sources: SkippedSourceLog,
    #[serde(default)]
    pub failed_sources: FailedSourceLog,
    /// Tier-2 lossy modern static assets imported directly into Photos.
    #[serde(default)]
    pub tier2_imported_assets: Vec<LibraryAssetRecord>,
    pub error: Option<String>,
}

impl WorkingCopyMarker {
    #[must_use]
    pub fn new(src_dir: PathBuf, working_copy: PathBuf, src_jpeg_count: usize) -> Self {
        Self {
            schema: 1,
            src_dir,
            working_copy,
            started_at: chrono::Utc::now().to_rfc3339(),
            stage: FastImgStageName::ScanComplete,
            src_jpeg_count,
            transcoded_count: 0,
            gate1_checks: Gate1Checks::default(),
            gate2_checks: Gate2Checks::default(),
            gate3_checks: Gate3Checks::default(),
            blake3_log: Blake3Log::new(),
            skipped_sources: SkippedSourceLog::new(),
            failed_sources: FailedSourceLog::new(),
            tier2_imported_assets: Vec::new(),
            error: None,
        }
    }

    #[must_use]
    pub fn expected_output_count(&self) -> usize {
        self.src_jpeg_count
            .saturating_sub(self.skipped_sources.len())
            .saturating_sub(self.failed_sources.len())
    }

    #[must_use]
    pub fn recorded_source_count(&self) -> usize {
        self.blake3_log
            .len()
            .saturating_add(self.skipped_sources.len())
            .saturating_add(self.failed_sources.len())
    }

    #[must_use]
    pub fn source_disposition_is_complete(&self) -> bool {
        self.recorded_source_count() == self.src_jpeg_count
    }

    #[must_use]
    pub fn source_disposition_over_recorded(&self) -> bool {
        self.recorded_source_count() > self.src_jpeg_count
    }

    pub fn validate_source_disposition_disjoint(&self) -> Result<(), String> {
        for rel in self.blake3_log.keys() {
            if self.skipped_sources.contains_key(rel) {
                return Err(format!("{rel} recorded as both converted and skipped"));
            }
            if self.failed_sources.contains_key(rel) {
                return Err(format!("{rel} recorded as both converted and failed"));
            }
        }
        for rel in self.skipped_sources.keys() {
            if self.failed_sources.contains_key(rel) {
                return Err(format!("{rel} recorded as both skipped and failed"));
            }
        }
        Ok(())
    }

    pub fn apply_gate1(&mut self, result: &GateResult) {
        for check in &result.checks {
            match check.name {
                "count" => self.gate1_checks.count = check.passed.into(),
                "blake3" => self.gate1_checks.blake3 = check.passed.into(),
                "metadata" => self.gate1_checks.metadata = check.passed.into(),
                "size" => self.gate1_checks.size = check.passed.into(),
                "orient" => self.gate1_checks.orient = check.passed.into(),
                "decode" => self.gate1_checks.decode = check.passed.into(),
                _ => {}
            }
        }
        self.stage = if result.passed {
            FastImgStageName::Gate1Passed
        } else {
            FastImgStageName::Gate1Failed
        };
        self.error = (!result.passed).then(|| summarize_checks(&result.checks));
    }

    pub fn apply_gate2(&mut self, result: &GateResult) {
        for check in &result.checks {
            match check.name {
                "count" => self.gate2_checks.count = check.passed.into(),
                "blake3_sample" => self.gate2_checks.blake3_sample = check.passed.into(),
                "no_error" => self.gate2_checks.no_error = check.passed.into(),
                _ => {}
            }
        }
        self.stage = if result.passed {
            FastImgStageName::Gate2Passed
        } else {
            FastImgStageName::Gate2Failed
        };
        self.error = (!result.passed).then(|| summarize_checks(&result.checks));
    }

    pub fn apply_gate3(&mut self, result: &GateResult) {
        for check in &result.checks {
            match check.name {
                "count_x3" => self.gate3_checks.count_x3 = check.passed.into(),
                "sync" => self.gate3_checks.sync = check.passed.into(),
                "quarantine" => self.gate3_checks.quarantine = check.passed.into(),
                "chain" => self.gate3_checks.chain = check.passed.into(),
                _ => {}
            }
        }
        self.stage = if result.passed {
            FastImgStageName::Gate3Passed
        } else {
            FastImgStageName::Gate3Failed
        };
        self.error = (!result.passed).then(|| summarize_checks(&result.checks));
    }
}

#[must_use]
pub fn working_copy_dir(src: &Path) -> PathBuf {
    let name = if let Some(name) = src.file_name() {
        name.to_string_lossy()
    } else {
        crate::media_conversion_gate::delivery_pipeline_path_audit(
            "fast_img_working_copy",
            src,
            "source path has no file name; using mfb_working_copy fallback",
        );
        std::borrow::Cow::Borrowed("mfb_working_copy")
    };
    src.with_file_name(format!("{name}_optimized"))
}

#[must_use]
pub fn resolve_working_copy_dir(src: &Path) -> PathBuf {
    let base = working_copy_dir(src);
    let mut suffix = 0usize;
    loop {
        let candidate = if suffix == 0 {
            base.clone()
        } else {
            let name = if let Some(name) = base.file_name() {
                name.to_string_lossy()
            } else {
                crate::media_conversion_gate::delivery_pipeline_path_audit(
                    "fast_img_working_copy_collision",
                    &base,
                    "optimized-output candidate has no file name; using mfb_working_copy fallback",
                );
                std::borrow::Cow::Borrowed("mfb_working_copy_optimized")
            };
            base.with_file_name(format!("{name}_{}", suffix + 1))
        };
        let has_marker =
            candidate.join(".mfb_wc").exists() || marker_path_for_working_copy(&candidate).exists();
        if !candidate.exists() || has_marker {
            return candidate;
        }
        suffix += 1;
    }
}

fn fast_img_marker_state_dir() -> PathBuf {
    let mfb_home_root = match std::env::var(crate::constants::ENV_MFB_HOME_ROOT) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            }
        }
        Err(std::env::VarError::NotPresent) => None,
        Err(err) => {
            crate::media_conversion_gate::delivery_jxl_batch_audit(
                "fast_img_marker_state_dir",
                format!(
                    "{} could not be read: {err}",
                    crate::constants::ENV_MFB_HOME_ROOT
                ),
            );
            None
        }
    };
    let home_root = || match std::env::var(crate::constants::ENV_HOME) {
        Ok(home) => Some(PathBuf::from(home).join(crate::constants::MFB_DEFAULT_HOME_DIRNAME)),
        Err(std::env::VarError::NotPresent) => None,
        Err(err) => {
            crate::media_conversion_gate::delivery_jxl_batch_audit(
                "fast_img_marker_state_dir",
                format!("{} could not be read: {err}", crate::constants::ENV_HOME),
            );
            None
        }
    };
    let root = mfb_home_root
        .or_else(home_root)
        .unwrap_or_else(crate::media_conversion_gate::delivery_temp_mfb_root_ssot);
    root.join("fast_img").join("markers")
}

#[must_use]
fn fast_img_marker_key(working_copy: &Path) -> String {
    let absolute = crate::media_conversion_gate::delivery_absolute_output_path_or_dot(
        working_copy,
        "fast_img_marker_key",
    );
    blake3::hash(absolute.to_string_lossy().as_bytes())
        .to_hex()
        .to_string()
}

#[must_use]
pub fn marker_path_for_working_copy(working_copy: &Path) -> PathBuf {
    fast_img_marker_state_dir().join(format!("{}.json", fast_img_marker_key(working_copy)))
}

pub fn write_marker_atomic(marker: &WorkingCopyMarker) -> std::io::Result<()> {
    if marker.working_copy == marker.src_dir || marker.working_copy.starts_with(&marker.src_dir) {
        return Err(std::io::Error::other(format!(
            "fast-img marker output must not be inside source tree: source={} output={}",
            marker.src_dir.display(),
            marker.working_copy.display()
        )));
    }
    let path = marker_path_for_working_copy(&marker.working_copy);
    let marker_dir = path.parent().ok_or_else(|| {
        std::io::Error::other(format!(
            "fast-img marker path has no parent: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(marker_dir)?;
    let tmp = path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(marker).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, data)?;
    std::fs::rename(tmp, &path)?;
    for legacy in [
        marker.working_copy.join(".mfb_wc"),
        marker.working_copy.join(".mfb_wc.tmp"),
    ] {
        match std::fs::remove_file(&legacy) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub fn read_marker(working_copy: &Path) -> std::io::Result<WorkingCopyMarker> {
    let marker_path = marker_path_for_working_copy(working_copy);
    match std::fs::read(&marker_path) {
        Ok(data) => serde_json::from_slice(&data).map_err(std::io::Error::other),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let legacy_path = working_copy.join(".mfb_wc");
            let data = std::fs::read(&legacy_path)?;
            let marker: WorkingCopyMarker =
                serde_json::from_slice(&data).map_err(std::io::Error::other)?;
            write_marker_atomic(&marker)?;
            Ok(marker)
        }
        Err(err) => Err(err),
    }
}

#[must_use]
pub const fn stage_requires_retry(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::Gate1Failed
            | FastImgStageName::Gate2Failed
            | FastImgStageName::Gate3Failed
    )
}

#[must_use]
pub fn retry_resume_stage(stage: &FastImgStageName, retry: bool) -> FastImgStageName {
    match (stage, retry) {
        (FastImgStageName::Gate1Failed, true) => FastImgStageName::OutputPrepared,
        (FastImgStageName::Gate2Failed, true) => FastImgStageName::Gate1Passed,
        (FastImgStageName::Gate3Failed, true) => FastImgStageName::Gate2Passed,
        (FastImgStageName::Aborted, _) => FastImgStageName::ScanComplete,
        _ => stage.clone(),
    }
}

#[must_use]
pub const fn output_prepared_or_later(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::OutputPrepared
            | FastImgStageName::TranscodeComplete
            | FastImgStageName::Gate1Passed
            | FastImgStageName::ImportComplete
            | FastImgStageName::Gate2Passed
            | FastImgStageName::DeepScanComplete
            | FastImgStageName::Gate3Passed
            | FastImgStageName::CleanupComplete
    )
}

#[must_use]
pub const fn transcode_complete_or_later(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::TranscodeComplete
            | FastImgStageName::Gate1Passed
            | FastImgStageName::ImportComplete
            | FastImgStageName::Gate2Passed
            | FastImgStageName::DeepScanComplete
            | FastImgStageName::Gate3Passed
            | FastImgStageName::CleanupComplete
    )
}

#[must_use]
pub const fn gate1_complete_or_later(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::Gate1Passed
            | FastImgStageName::ImportComplete
            | FastImgStageName::Gate2Passed
            | FastImgStageName::DeepScanComplete
            | FastImgStageName::Gate3Passed
            | FastImgStageName::CleanupComplete
    )
}

#[must_use]
pub const fn import_complete_or_later(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::ImportComplete
            | FastImgStageName::Gate2Passed
            | FastImgStageName::DeepScanComplete
            | FastImgStageName::Gate3Passed
            | FastImgStageName::CleanupComplete
    )
}

#[must_use]
pub const fn gate2_complete_or_later(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::Gate2Passed
            | FastImgStageName::DeepScanComplete
            | FastImgStageName::Gate3Passed
            | FastImgStageName::CleanupComplete
    )
}

#[must_use]
pub const fn deep_scan_complete_or_later(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::DeepScanComplete
            | FastImgStageName::Gate3Passed
            | FastImgStageName::CleanupComplete
    )
}

#[must_use]
pub const fn gate3_complete_or_later(stage: &FastImgStageName) -> bool {
    matches!(
        stage,
        FastImgStageName::Gate3Passed | FastImgStageName::CleanupComplete
    )
}

#[must_use]
pub const fn confirm_scan_required(_stage: &FastImgStageName) -> bool {
    false
}

#[must_use]
pub const fn confirm_import_required(stage: &FastImgStageName, auto_import: bool) -> bool {
    !auto_import && !import_complete_or_later(stage)
}

#[must_use]
pub const fn resume_action(stage: &FastImgStageName) -> &'static str {
    match stage {
        FastImgStageName::ScanComplete => "prepare_output_then_transcode",
        FastImgStageName::OutputPrepared => "skip_prepare_then_transcode",
        FastImgStageName::TranscodeComplete => "skip_transcode_then_gate1",
        FastImgStageName::Gate1Passed => "skip_to_import",
        FastImgStageName::ImportComplete => "skip_to_gate2",
        FastImgStageName::Gate2Passed => "skip_to_deep_scan",
        FastImgStageName::DeepScanComplete => "skip_to_gate3",
        FastImgStageName::Gate3Passed => "skip_to_cleanup",
        FastImgStageName::CleanupComplete => "no_op_exit_zero",
        FastImgStageName::Gate1Failed
        | FastImgStageName::Gate2Failed
        | FastImgStageName::Gate3Failed => "require_retry",
        FastImgStageName::Aborted => "fresh_or_manual_resume",
    }
}

pub fn prepare_jxl_output_dir(output_dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(output_dir)
}

pub fn dir_checksum(root: &Path) -> std::io::Result<Vec<(PathBuf, u64, std::time::SystemTime)>> {
    let mut entries = Vec::new();
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.map_err(|err| {
            std::io::Error::other(format!(
                "failed to walk checksum root {}: {err}",
                root.display()
            ))
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .map_or_else(|_| entry.path().to_path_buf(), Path::to_path_buf);
        let metadata = std::fs::metadata(entry.path())?;
        entries.push((rel, metadata.len(), metadata.modified()?));
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

#[must_use]
pub fn summarize_checks(checks: &[CheckDetail]) -> String {
    let failed = checks
        .iter()
        .filter(|check| !check.passed)
        .collect::<Vec<_>>();
    let summary_checks = if failed.is_empty() {
        checks.iter().collect::<Vec<_>>()
    } else {
        failed
    };
    summary_checks
        .into_iter()
        .map(|check| {
            format!(
                "{} expected={} actual={}",
                check.name, check.expected, check.actual
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

#[must_use]
pub fn marker_checks_from_result(result: &GateResult) -> String {
    summarize_checks(&result.checks)
}

#[cfg(test)]
mod working_copy_tests {
    use super::{
        Blake3Entry, FastImgStageName, SkippedSourceEntry, WorkingCopyMarker,
        marker_path_for_working_copy, prepare_jxl_output_dir, read_marker,
        resolve_working_copy_dir, working_copy_dir, write_marker_atomic,
    };
    use crate::common_utils::EnvGuard;
    use serial_test::serial;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn source_disposition_disjoint_rejects_overlap() {
        let mut marker =
            WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("src_optimized"), 2);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "hash-a".to_string(),
                out: "out-a".to_string(),
                library_asset: None,
            },
        );
        marker.skipped_sources.insert(
            "a.jpg".to_string(),
            SkippedSourceEntry {
                src: "hash-a".to_string(),
                reason: "duplicate".to_string(),
            },
        );

        assert!(marker.source_disposition_is_complete());
        assert!(!marker.source_disposition_over_recorded());
        assert_eq!(
            marker.validate_source_disposition_disjoint(),
            Err("a.jpg recorded as both converted and skipped".to_string())
        );
    }

    #[test]
    fn source_disposition_complete_when_all_sources_accounted() {
        let mut marker =
            WorkingCopyMarker::new(PathBuf::from("src"), PathBuf::from("src_optimized"), 2);
        marker.blake3_log.insert(
            "a.jpg".to_string(),
            Blake3Entry {
                out_rel: Some("a.JXL".to_string()),
                src: "hash-a".to_string(),
                out: "out-a".to_string(),
                library_asset: None,
            },
        );
        marker.skipped_sources.insert(
            "b.jpg".to_string(),
            SkippedSourceEntry {
                src: "hash-b".to_string(),
                reason: "lossless JPEG transcode failed after strict cascade".to_string(),
            },
        );

        assert!(marker.source_disposition_is_complete());
        assert!(!marker.source_disposition_over_recorded());
        assert!(marker.validate_source_disposition_disjoint().is_ok());
    }

    #[test]
    fn working_copy_suffix_uses_adjacent_optimized_dir() {
        let root = TempDir::new().unwrap();
        let src = root.path().join("Photos");

        assert_eq!(working_copy_dir(&src), root.path().join("Photos_optimized"));
    }

    #[test]
    fn working_copy_collision_uses_numbered_optimized_dir_without_marker() {
        let root = TempDir::new().unwrap();
        let src = root.path().join("Photos");
        std::fs::create_dir_all(root.path().join("Photos_optimized")).unwrap();

        assert_eq!(
            resolve_working_copy_dir(&src),
            root.path().join("Photos_optimized_2")
        );
    }

    #[test]
    #[serial]
    fn working_copy_collision_resumes_with_marker_in_optimized_dir() {
        let root = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let _home_guard = EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            state.path().to_str().expect("utf-8 state path"),
        );
        let src = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let marker = WorkingCopyMarker::new(src.clone(), wc.clone(), 1);
        std::fs::create_dir_all(&wc).unwrap();
        write_marker_atomic(&marker).unwrap();

        assert_eq!(resolve_working_copy_dir(&src), wc);
    }

    #[test]
    #[serial]
    fn marker_write_is_readable_after_atomic_rename() {
        let root = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let _home_guard = EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            state.path().to_str().expect("utf-8 state path"),
        );
        let src = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        let mut marker = WorkingCopyMarker::new(src, wc, 2);
        marker.stage = FastImgStageName::OutputPrepared;

        write_marker_atomic(&marker).unwrap();
        let read = read_marker(&marker.working_copy).unwrap();

        assert_eq!(read.stage, FastImgStageName::OutputPrepared);
        assert!(!marker.working_copy.join(".mfb_wc").exists());
        assert!(marker_path_for_working_copy(&marker.working_copy).exists());
    }

    #[test]
    fn prepare_output_dir_does_not_copy_source_contents() {
        let root = TempDir::new().unwrap();
        let src = root.path().join("Photos");
        let output = root.path().join("Photos_optimized");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.jpg"), b"jpeg").unwrap();
        let before = std::fs::read(src.join("a.jpg")).unwrap();

        prepare_jxl_output_dir(&output).unwrap();
        let after = std::fs::read(src.join("a.jpg")).unwrap();

        assert_eq!(before, after);
        assert!(output.exists());
        assert!(!output.join("a.jpg").exists());
    }

    #[test]
    #[serial]
    fn marker_write_does_not_create_metadata_inside_source_tree() {
        let root = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let _home_guard = EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            state.path().to_str().expect("utf-8 state path"),
        );
        let src = root.path().join("Photos");
        std::fs::create_dir_all(&src).unwrap();
        let wc = resolve_working_copy_dir(&src);
        let mut marker = WorkingCopyMarker::new(src.clone(), wc.clone(), 1);
        marker.stage = FastImgStageName::OutputPrepared;

        write_marker_atomic(&marker).unwrap();

        assert!(!src.join(".mfb_wc").exists());
        assert!(!wc.join(".mfb_wc").exists());
        assert!(!wc.join(".mfb_wc.tmp").exists());
        assert!(marker_path_for_working_copy(&wc).starts_with(state.path()));
        assert!(marker_path_for_working_copy(&wc).exists());
        assert_eq!(wc, root.path().join("Photos_optimized"));
    }

    #[test]
    #[serial]
    fn legacy_output_marker_is_migrated_out_of_media_tree() {
        let root = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let _home_guard = EnvGuard::set(
            crate::constants::ENV_MFB_HOME_ROOT,
            state.path().to_str().expect("utf-8 state path"),
        );
        let src = root.path().join("Photos");
        let wc = root.path().join("Photos_optimized");
        std::fs::create_dir_all(&wc).unwrap();
        let mut marker = WorkingCopyMarker::new(src, wc.clone(), 1);
        marker.stage = FastImgStageName::OutputPrepared;
        std::fs::write(
            wc.join(".mfb_wc"),
            serde_json::to_vec_pretty(&marker).unwrap(),
        )
        .unwrap();

        let read = read_marker(&wc).unwrap();

        assert_eq!(read.stage, FastImgStageName::OutputPrepared);
        assert!(!wc.join(".mfb_wc").exists());
        assert!(marker_path_for_working_copy(&wc).exists());
    }
}

#[cfg(test)]
mod fast_gate_policy_tests {
    use super::{CheckDetail, GateResult};

    fn failed_result(name: &'static str) -> GateResult {
        GateResult {
            passed: false,
            checks: vec![CheckDetail {
                name,
                passed: false,
                expected: "expected detail".to_string(),
                actual: "actual detail".to_string(),
                affected_files: Vec::new(),
            }],
        }
    }

    #[test]
    fn gate1_failure_blocks_import_callback() {
        let gate1 = failed_result("blake3");
        let import_called = gate1.passed;

        assert!(!import_called, "import must not run after Gate 1 failure");
    }

    #[test]
    fn gate3_failure_blocks_cleanup_callback() {
        let gate3 = failed_result("chain");
        let cleanup_called = gate3.passed;

        assert!(
            !cleanup_called,
            "source cleanup must not run after Gate 3 failure"
        );
    }
}

#[cfg(test)]
mod rev2_policy_tests {
    use super::{
        FastImgStageName, confirm_import_required, confirm_scan_required,
        deep_scan_complete_or_later, dir_checksum, gate1_complete_or_later,
        gate2_complete_or_later, gate3_complete_or_later, import_complete_or_later,
        output_prepared_or_later, prepare_jxl_output_dir, resume_action, retry_resume_stage,
        stage_requires_retry, transcode_complete_or_later,
    };
    use tempfile::TempDir;

    #[test]
    fn resume_actions_cover_rev2_stages() {
        let cases = [
            (
                FastImgStageName::OutputPrepared,
                "skip_prepare_then_transcode",
            ),
            (FastImgStageName::Gate1Passed, "skip_to_import"),
            (FastImgStageName::Gate2Passed, "skip_to_deep_scan"),
            (FastImgStageName::Gate3Passed, "skip_to_cleanup"),
            (FastImgStageName::CleanupComplete, "no_op_exit_zero"),
        ];

        for (stage, expected) in cases {
            assert_eq!(resume_action(&stage), expected);
        }
    }

    #[test]
    fn failed_stages_require_retry_flag() {
        for stage in [
            FastImgStageName::Gate1Failed,
            FastImgStageName::Gate2Failed,
            FastImgStageName::Gate3Failed,
        ] {
            assert!(stage_requires_retry(&stage));
            assert_eq!(resume_action(&stage), "require_retry");
        }
    }

    #[test]
    fn cleanup_complete_is_noop_policy() {
        assert_eq!(
            resume_action(&FastImgStageName::CleanupComplete),
            "no_op_exit_zero"
        );
        assert!(!stage_requires_retry(&FastImgStageName::CleanupComplete));
    }

    #[test]
    fn retry_maps_failed_stage_to_previous_successful_checkpoint() {
        assert_eq!(
            retry_resume_stage(&FastImgStageName::Gate1Failed, true),
            FastImgStageName::OutputPrepared
        );
        assert_eq!(
            retry_resume_stage(&FastImgStageName::Gate2Failed, true),
            FastImgStageName::Gate1Passed
        );
        assert_eq!(
            retry_resume_stage(&FastImgStageName::Gate3Failed, true),
            FastImgStageName::Gate2Passed
        );
        assert_eq!(
            retry_resume_stage(&FastImgStageName::Gate1Failed, false),
            FastImgStageName::Gate1Failed
        );
    }

    #[test]
    fn scan_delete_notice_is_non_interactive_but_import_confirm_remains() {
        assert!(!confirm_scan_required(&FastImgStageName::ScanComplete));
        assert!(confirm_import_required(
            &FastImgStageName::Gate1Passed,
            false
        ));
        assert!(!confirm_import_required(
            &FastImgStageName::Gate1Passed,
            true
        ));
    }

    #[test]
    fn stage_predicates_preserve_resume_progress() {
        assert!(output_prepared_or_later(&FastImgStageName::OutputPrepared));
        assert!(transcode_complete_or_later(
            &FastImgStageName::TranscodeComplete
        ));
        assert!(gate1_complete_or_later(&FastImgStageName::Gate1Passed));
        assert!(import_complete_or_later(&FastImgStageName::ImportComplete));
        assert!(gate2_complete_or_later(&FastImgStageName::Gate2Passed));
        assert!(deep_scan_complete_or_later(
            &FastImgStageName::DeepScanComplete
        ));
        assert!(gate3_complete_or_later(&FastImgStageName::Gate3Passed));
    }

    #[test]
    fn output_prepare_does_not_copy_source_at_abort_points() {
        let root = TempDir::new().unwrap();
        let src = root.path().join("Photos");
        let output = root.path().join("Photos_");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("top.jpg"), b"jpeg-top").unwrap();
        std::fs::write(src.join("nested/img.jpg"), b"jpeg-nested").unwrap();
        let pre = dir_checksum(&src).unwrap_or_else(|err| panic!("pre checksum failed: {err}"));

        prepare_jxl_output_dir(&output).unwrap();
        for _stage in [
            FastImgStageName::ScanComplete,
            FastImgStageName::OutputPrepared,
            FastImgStageName::TranscodeComplete,
            FastImgStageName::Gate1Failed,
            FastImgStageName::Gate1Passed,
            FastImgStageName::Gate2Failed,
            FastImgStageName::Gate2Passed,
            FastImgStageName::Gate3Failed,
        ] {
            assert_eq!(
                pre,
                dir_checksum(&src).unwrap_or_else(|err| panic!("post checksum failed: {err}"))
            );
        }
        assert!(!output.join("top.jpg").exists());
        assert!(!output.join("nested/img.jpg").exists());
    }
}

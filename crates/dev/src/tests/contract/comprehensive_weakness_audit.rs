//! Single CI entrypoint for weakness inventory + closure (avoids per-milestone
//! test sprawl).
//!
//! **Zero-fabrication policy:** gate audit / constants / 备案 do **not** exempt
//! numeric injection into measurement or decision fields. Missing values use
//! `NaN` or JSON `null`, never `0.0` / `1.0` posing as measured.
//! `media_conversion_gate.rs` is scanned (not blanket-skipped).

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn read_hardening_doc(root: impl AsRef<Path>, name: &str) -> String {
    let root = root.as_ref();
    for direct in [
        root.join("docs").join(name),
        root.join("docs/hardening").join(name),
    ] {
        if direct.is_file() {
            return fs::read_to_string(&direct)
                .unwrap_or_else(|err| panic!("read {}: {err:?}", direct.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        }
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

fn map_foundation_subpath(rel: &str) -> String {
    if rel.starts_with("video_explorer/") {
        return format!("video/{rel}");
    }
    for (prefix, dir) in [
        ("algorithm_", "algo"),
        ("image_", "image"),
        ("jxl_", "image"),
        ("loop_intent", "image"),
        ("live_photo", "image"),
        ("depth_channel", "image"),
        ("animated_image_quality_features", "image"),
        ("video_", "video"),
        ("ffmpeg_", "video"),
        ("ffprobe", "video"),
        ("x265_", "video"),
        ("msssim_", "video"),
        ("stream_size", "video"),
        ("gpu_accel", "video"),
        ("vmaf_", "video"),
        ("quality_", "quality"),
        ("ssim_mapping", "quality"),
        ("crf_constants", "quality"),
        ("analysis_cache", "convert"),
        ("batch", "convert"),
        ("checkpoint", "convert"),
        ("conversion", "convert"),
        ("delivery_codec_strategy", "convert"),
        ("explore_strategy", "convert"),
        ("file_copier", "convert"),
        ("file_sorter", "convert"),
        ("lru_cache", "convert"),
        ("media_conversion_gate", "convert"),
        ("media_passthrough", "convert"),
        ("media_penetration", "convert"),
        ("media_precision", "convert"),
        ("process_runner", "convert"),
        ("pure_media_verifier", "convert"),
        ("smart_file_copier", "convert"),
        ("database", "db"),
        ("multi_scenario_db", "db"),
        ("scenario", "db"),
        ("training_", "train"),
        ("c_api", "train"),
        ("logging", "infra"),
        ("ctrlc_guard", "infra"),
        ("path_", "infra"),
        ("numeric_cast", "infra"),
        ("common_utils", "infra"),
        ("io_utils", "infra"),
        ("error_handler", "infra"),
        ("entry_guard", "infra"),
        ("flag_validator", "infra"),
        ("system_memory", "infra"),
        ("real_physics", "quality"),
        ("hdr", "media"),
        ("xmp_merger", "media"),
        ("date_analysis", "media"),
        ("media_meta_utils", "media"),
        ("progress", "ui"),
        ("progress_mode", "ui"),
        ("modern_ui", "ui"),
        ("unified_progress", "ui"),
    ] {
        if rel.starts_with(prefix) {
            return format!("{dir}/{rel}");
        }
    }
    rel.to_string()
}

fn resolve_workspace_path(root: &Path, rel: &str) -> PathBuf {
    let direct = root.join(rel);
    if direct.is_file() {
        return direct;
    }
    if let Some(rest) = rel.strip_prefix("crates/foundation/src/") {
        let mapped = root
            .join("crates/foundation/src")
            .join(map_foundation_subpath(rest));
        if mapped.is_file() {
            return mapped;
        }
    }
    direct
}

fn production_scope(content: &str) -> &str {
    if let Some((idx, _)) = content.match_indices("\nmod tests {").next() {
        &content[..idx]
    } else if let Some((idx, _)) = content.match_indices("\n#[cfg(test)]\nmod tests").next() {
        &content[..idx]
    } else {
        content
    }
}

/// Mirrors `algorithm_audit::tests::MODULES` + gate (gate is not an exemption
/// umbrella).
const ALGORITHM_AUDIT_INFERENCE_MODULES: &[&str] = &[
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
    "crates/foundation/src/media_conversion_gate.rs",
];

/// img/vid delivery crates (M251 whole-project fabrication sweep).
const PROJECT_DELIVERY_MODULES: &[&str] = &[
    "crates/vid/src/conversion_api.rs",
    "crates/vid/src/animated_image.rs",
    "crates/vid/src/detection_api.rs",
    "crates/vid/src/ffprobe.rs",
    "crates/img/src/conversion_api.rs",
    "crates/img/src/lossless_converter.rs",
    "crates/img/src/detection_api.rs",
    "crates/img/src/analyzer.rs",
];

/// Production must not call legacy `*_or_zero` helpers that used to fabricate
/// `0`/`0.0` (use `*_optional`).
const PRODUCTION_FORBIDDEN_LEGACY_OR_ZERO: &[&str] = &[
    "probe_idet_count_or_zero(",
    "gpu_compression_potential_adjustment_or_zero(",
    "probe_jxlinfo_dimensions_or_zero(",
    "loop_gif_logical_screen_or_zero(",
    "explore_precheck_nb_frames_or_zero(",
    "jxl_best_telemetry_or_zero(",
    "delivery_path_modified_unix_secs_or_zero(",
    "delivery_batch_relative_depth_or_zero(",
];

/// `runtime_elapsed_secs_or_zero` is allowed only in the Ctrl-C watcher
/// (audited non-measurement 0s).
const CTRLC_ALLOWED_LEGACY_OR_ZERO: &str = "runtime_elapsed_secs_or_zero(";

/// AGENTS CAT-B: audited delivery/inference modules must route observability
/// through SSOT macros.
const CAT_B_LOG_MARKERS: &[&str] = &[
    "tracing::",
    "delivery_runtime_batch_audit(",
    "ui_stderr::",
    "log_upstream_error!",
    "log_info!",
    "log_warn!",
    "log_detail!",
    "static_logs::",
    "symbols::pick(",
];

/// Pure feature vectors, env toggles, or thin re-exports (logging lives in
/// `foundation`).
const CAT_B_LOG_EXEMPT: &[&str] = &[
    "crates/foundation/src/algorithm_runtime.rs",
    "crates/foundation/src/scenario.rs",
    "crates/foundation/src/c_api.rs",
    "crates/foundation/src/training_tier_audit.rs",
    "crates/foundation/src/database_vector.rs",
    "crates/foundation/src/animated_image_quality_features.rs",
    "crates/foundation/src/video_quality_features.rs",
    "crates/foundation/src/video_explorer/precision.rs",
    "crates/foundation/src/image_analyzer.rs",
    "crates/foundation/src/image_metrics.rs",
    "crates/img/src/detection_api.rs",
    "crates/img/src/analyzer.rs",
    "crates/vid/src/detection_api.rs",
    "crates/vid/src/ffprobe.rs",
];

/// AGENTS CAT-D: physics/geometry-heavy modules may use primitive casts;
/// decision modules may not.
const CAT_D_CAST_EXEMPT: &[&str] = &[
    "crates/foundation/src/numeric_cast.rs",
    "crates/foundation/src/real_physics.rs",
    "crates/foundation/src/image_detection.rs",
    "crates/foundation/src/image_jpeg_analysis.rs",
    "crates/foundation/src/image_heic_analysis.rs",
    "crates/foundation/src/video_detection.rs",
];

const CAT_D_FLOAT_CAST_PATTERNS: &[&str] = &[" as f64", " as f32"];
const CAT_D_INT_CAST_PATTERNS: &[&str] = &[" as u64", " as u32", " as usize", " as i32", " as i64"];

const PROJECT_INLINE_NUMERIC_FORGERY: &[&str] = &[
    "unwrap_or(0.0)",
    "unwrap_or(0)",
    "map_or(0.0,",
    "map_or(0,",
    "map_or(0.0_f64,",
    "confidence: Some(1.0)",
    "psnr: Some(0.0",
    "ssim: Some(0.0",
    "last_best_crf = Some(0.0)",
];

const DECISION_CHAIN_FABRICATION: &[&str] = &[
    "infer_quality_embedding_psnr_ssim",
    "fill_missing_percentiles_from_moments()",
    "estimate_psnr_from_quality(",
    "estimate_ssim_from_quality(",
];

const ALGORITHM_AUDIT_FORBIDDEN: &[&str] = &[
    "unwrap_or(0.5",
    "unwrap_or(knn",
    "unwrap_or_else(|| 0.5",
    "unwrap_or_else(|| 0.0",
    "0.5 neutral prior",
    "preserving raw",
    "confidence: Some(1.0)",
    "EXPLORE_CONFIDENCE_HIGH",
    "unwrap_or(1.0)",
];

/// Gate 备案 cannot re-introduce PSNR/SSIM 0.0 sentinels on decision paths.
const ZERO_TOLERANCE_MEASUREMENT_FABRICATION: &[&str] = &[
    "quality_embedding_optional_f64_or_zero(analysis.psnr",
    "quality_embedding_optional_f64_or_zero(analysis.ssim",
    "quality_embedding_optional_f64_or_zero(\n            analysis.psnr",
    "missing or non-finite feature; using 0.0",
    "using 0.0 sentinel",
    "0.0 sentinel when absent",
    "non-finite animated color richness; using 0.0",
    "SSIM mutex unavailable; using 0.0",
    "CRF mutex unavailable; using 0.0",
    "size mutex unavailable; using 0",
    "using 0.0s elapsed",
    "using (0,0,0) stats",
    "empty distance slice; using 0.0",
    "using 0.0 FPS",
    "using zero dimensions",
    "empty size_history; using 0",
    "using (0.0, 0) telemetry",
    "using 0.0 grid anchor",
    "using total_pixels 0.0",
    "using bytes_per_frame 0.0",
    "using neutral z-score 0.0",
    "using tail headroom 0.0",
    "using overflow 0.0",
    "using short proximity 0.0",
    "using median frames 0.0",
    "using adjustment 0.0",
    "using 0-byte GCT",
    "using 0 (non-animated)",
];

const ZERO_TOLERANCE_NONE_TO_ZERO_LITERAL: &str = "None => 0.0";
const ZERO_TOLERANCE_KNN_ABSENT_HELPER: &str = "knn_absent_feature_component";

const ZERO_TOLERANCE_REQUIRED_OPTION_TYPES: &[&str] = &["reference_entropy: Option<f64>"];

fn production_legacy_or_zero_call_offenders(
    root: &Path,
    modules: &[&str],
    patterns: &[&str],
) -> Vec<String> {
    let mut hits = Vec::new();
    for rel in modules {
        let path = resolve_workspace_path(root, rel);
        if !path.is_file() {
            hits.push(format!("{rel}: missing (inventory drift)"));
            continue;
        }
        let content =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for pattern in patterns {
            let fn_name = pattern.trim_end_matches('(');
            for (line_no, line) in prod.lines().enumerate() {
                if !line.contains(pattern) {
                    continue;
                }
                let trimmed = line.trim();
                if trimmed.starts_with("//") || trimmed.starts_with("///") {
                    continue;
                }
                if trimmed.contains(&format!("pub fn {fn_name}")) {
                    continue;
                }
                if trimmed.contains(&format!("\"{fn_name}")) {
                    continue;
                }
                hits.push(format!("{rel}:{}: `{pattern}`", line_no + 1));
            }
        }
    }
    hits
}

fn production_scope_offenders(root: &Path, modules: &[&str], patterns: &[&str]) -> Vec<String> {
    let mut hits = Vec::new();
    for rel in modules {
        let path = resolve_workspace_path(root, rel);
        if !path.is_file() {
            hits.push(format!("{rel}: missing (inventory drift)"));
            continue;
        }
        let content =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for pattern in patterns {
            if prod.contains(pattern) {
                hits.push(format!("{rel}: `{pattern}`"));
            }
        }
    }
    hits
}

fn gate_or_zero_call_offenders(gate_src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let prod = production_scope(gate_src);
    for (line_no, line) in prod.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        if line.contains("quality_embedding_optional_f64_or_zero(")
            && !line.contains("pub fn quality_embedding_optional_f64_or_zero")
            && !line.contains("let scaled = quality_embedding_optional_f64_or_zero")
        {
            hits.push(format!(
                "media_conversion_gate.rs:{}: internal call to \
                 quality_embedding_optional_f64_or_zero (prefer explicit NaN path)",
                line_no + 1
            ));
        }
    }
    hits
}

#[test]
fn comprehensive_weakness_audit_suite() {
    let root = workspace_root();
    let mut inventory: Vec<String> = Vec::new();

    check_tier_a_and_zero_tolerance(&root, &mut inventory);
    check_tier_b(&root, &mut inventory);
    check_tier_c(&root, &mut inventory);
    check_cat_b_log_coverage(&root, &mut inventory);
    check_cat_d_numeric_casts(&root, &mut inventory);

    assert!(
        inventory.is_empty(),
        "weakness inventory must be empty (fix all, then re-run):\n\n{}",
        inventory.join("\n\n")
    );
}

#[allow(clippy::too_many_lines)]
fn check_tier_a_and_zero_tolerance(root: &Path, inventory: &mut Vec<String>) {
    let decision_hits = production_scope_offenders(
        root,
        ALGORITHM_AUDIT_INFERENCE_MODULES,
        DECISION_CHAIN_FABRICATION,
    );
    if !decision_hits.is_empty() {
        inventory.push(format!(
            "A1 decision-chain in production scope ({} hits):\n  {}",
            decision_hits.len(),
            decision_hits.join("\n  ")
        ));
    }

    let audit_hits = production_scope_offenders(
        root,
        ALGORITHM_AUDIT_INFERENCE_MODULES,
        ALGORITHM_AUDIT_FORBIDDEN,
    );
    if !audit_hits.is_empty() {
        inventory.push(format!(
            "A2 algorithm_audit forbidden in production ({} hits):\n  {}",
            audit_hits.len(),
            audit_hits.join("\n  ")
        ));
    }

    let zero_tol = production_scope_offenders(
        root,
        ALGORITHM_AUDIT_INFERENCE_MODULES,
        ZERO_TOLERANCE_MEASUREMENT_FABRICATION,
    );
    if !zero_tol.is_empty() {
        inventory.push(format!(
            "A0 zero-tolerance measurement fabrication ({} hits; gate 备案 not exempt):\n  {}",
            zero_tol.len(),
            zero_tol.join("\n  ")
        ));
    }

    let iqd = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/image_quality_db.rs",
    ))
    .expect("image_quality_db.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let iqd_prod = production_scope(&iqd);
    if !iqd_prod.contains("quality_embed_measured_dimension_f32") {
        inventory.push(
            "A0 image_quality_db must use quality_embed_measured_dimension_f32 for optional \
             measurements"
                .into(),
        );
    }
    if iqd_prod.contains("quality_embedding_optional_f64_or_zero") {
        inventory.push(
            "A0 image_quality_db production must not call quality_embedding_optional_f64_or_zero"
                .into(),
        );
    }
    let animated = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/animated_image_quality_features.rs",
    ))
    .expect("animated_image_quality_features.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let animated_prod = production_scope(&animated);
    for needle in ZERO_TOLERANCE_REQUIRED_OPTION_TYPES {
        if !animated_prod.contains(needle) {
            inventory.push(format!(
                "A0 animated_image_quality_features must declare `{needle}`"
            ));
        }
    }
    if animated_prod.contains("pub reference_entropy: f64,") {
        inventory.push(
            "A0 reference_entropy must be Option<f64>, not bare f64 (prevents 0.0/NAN posing as \
             absent)"
                .into(),
        );
    }
    if iqd_prod.contains(ZERO_TOLERANCE_NONE_TO_ZERO_LITERAL) {
        inventory
            .push("A0 image_quality_db must not use `None => 0.0` for absent measurements".into());
    }
    if animated_prod.contains(ZERO_TOLERANCE_NONE_TO_ZERO_LITERAL) {
        inventory.push(
            "A0 animated_image_quality_features must not use `None => 0.0` for absent measurements"
                .into(),
        );
    }
    let loop_intent = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/loop_intent.rs",
    ))
    .expect("loop_intent.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let loop_prod = production_scope(&loop_intent);
    if loop_prod.contains(ZERO_TOLERANCE_NONE_TO_ZERO_LITERAL) {
        inventory.push(
            "A0 loop_intent must not use `None => 0.0` on decision paths (use Option/NaN/skip)"
                .into(),
        );
    }
    let precheck = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/video_explorer/precheck.rs",
    ))
    .expect("precheck.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let precheck_prod = production_scope(&precheck);
    if precheck_prod.contains("explore_precheck_nb_frames_or_zero(") {
        inventory.push(
            "A0 precheck must use explore_precheck_nb_frames_resolved/optional, not or_zero".into(),
        );
    }
    let image_detection = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/image_detection.rs",
    ))
    .expect("image_detection.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let image_prod = production_scope(&image_detection);
    for label in ["jxl_frame_count", "heif_frame_count"] {
        if image_prod.contains(label) && image_prod.contains(".or(Some(1))") {
            inventory.push(format!(
                "A0 image_detection must not `.or(Some(1))` after {label} (fabricated frame \
                 default)"
            ));
        }
    }
    if image_prod.contains("ToolNotFound") && image_prod.contains("djxl/jxlinfo") {
        inventory.push(
            "A0 JXL animation probe must audit missing djxl/jxlinfo instead of ToolNotFound error"
                .into(),
        );
    }

    let vid_conversion = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/vid/src/conversion_api.rs",
    ))
    .expect("vid conversion_api.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let vid_prod = production_scope(&vid_conversion);
    if vid_prod.contains("detection.frame_count = Some(1)") {
        inventory.push(
            "A0 vid static safeguard must not assign fabricated frame_count=1 when demux is absent"
                .into(),
        );
    }
    if vid_prod.contains("!is_animated || native_frames.is_none_or") {
        inventory.push(
            "A0 vid static safeguard must require !is_animated AND measured/low fc (no OR with \
             absent fc)"
                .into(),
        );
    }

    let image_analyzer = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/image_analyzer.rs",
    ))
    .expect("image_analyzer.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let image_analyzer_prod = production_scope(&image_analyzer);
    if image_analyzer_prod.contains("unwrap_or((0, 0") {
        inventory.push(
            "A0 image_analyzer must not unwrap_or fabricate 0×0 canvas on probe failure".into(),
        );
    }
    if image_analyzer_prod.contains("probe_jxlinfo_dimensions_or_zero") {
        inventory.push(
            "A0 image_analyzer must use probe_jxlinfo_dimensions_optional (no fake JXL dimensions)"
                .into(),
        );
    }
    if !image_analyzer_prod.contains("fn resolve_jxl_canvas") {
        inventory.push("A0 image_analyzer must resolve JXL canvas via resolve_jxl_canvas".into());
    }

    let jxl_explorer = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/jxl_explorer.rs",
    ))
    .expect("jxl_explorer.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let jxl_explorer_prod = production_scope(&jxl_explorer);
    if jxl_explorer_prod.contains(".unwrap_or((f32::NAN, 0))") {
        inventory.push(
            "A0 jxl_explorer must not unwrap_or fake (NAN,0) telemetry (use optional + candidate \
             fields)"
                .into(),
        );
    }
    if jxl_explorer_prod.contains("jxl_best_telemetry_or_zero(") {
        inventory.push(
            "A0 jxl_explorer must not use jxl_best_telemetry_or_zero (u64::MAX poisons screening \
             result)"
                .into(),
        );
    }
    if !image_analyzer_prod.contains("resolve_jxl_canvas_from_ffprobe") {
        inventory.push(
            "A0 image_analyzer must ffprobe-fallback after jxlinfo parse failure (no dead-end \
             None)"
                .into(),
        );
    }

    let gate_src = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/media_conversion_gate.rs",
    ))
    .expect("media_conversion_gate.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let gate_or_zero = gate_or_zero_call_offenders(&gate_src);
    if !gate_or_zero.is_empty() {
        inventory.push(format!(
            "A0 gate must not call quality_embedding_optional_f64_or_zero except the \
             definition:\n  {}",
            gate_or_zero.join("\n  ")
        ));
    }
    let gate_prod = production_scope(&gate_src);
    if gate_prod.contains("sentinel 0 for duration re-derive") {
        inventory.push("A0 gate must not document nb_frames sentinel 0 fabrication path".into());
    }
    if gate_prod.contains("missing or non-finite feature; using 0.0") {
        inventory.push("A0 probe_optional_f64_or_zero must not audit 'using 0.0'".into());
    }
    if gate_prod.contains("SSIM mutex unavailable; using 0.0") {
        inventory
            .push("A0 progress_explore_ssim must not audit fake SSIM 0.0 on mutex poison".into());
    }
    if gate_prod.contains("unwrap_or((0, 0, false, None))")
        || gate_prod.contains("unwrap_or((0, 0))")
    {
        inventory.push(
            "A0 gate legacy *_or_zero helpers must not fabricate (0,0) dimensions (panic or \
             optional)"
                .into(),
        );
    }
    if let Some(body) = gate_prod
        .split("pub fn explore_progress_time_millis_or_zero")
        .nth(1)
    {
        let body = body.split("\npub fn ").next().unwrap_or(body);
        if body.contains("None => 0") {
            inventory.push(
                "A0 explore_progress_time_millis_or_zero must panic on overflow, not fabricate \
                 millis 0"
                    .into(),
            );
        }
    }

    let db_src = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let db_prod = production_scope(&db_src);
    if db_prod.contains("fill_missing_percentiles_from_moments") {
        inventory.push(
            "A0 database production must not define fill_missing_percentiles_from_moments \
             (test-only)"
                .into(),
        );
    }

    if jxl_explorer_prod.contains(".unwrap_or(0)") {
        inventory.push(
            "A0 jxl_explorer must not unwrap_or(0) on screening telemetry (fabricated size)".into(),
        );
    }

    let silent_fb = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/dev/src/tests/contract/test_real_silent_fallbacks.rs",
    ))
    .expect("test_real_silent_fallbacks.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let skip_body = silent_fb
        .split("fn fabrication_scan_skip_file")
        .nth(1)
        .and_then(|s| s.split("fn gate_unified_fabrication_line_exempt").next())
        .expect("fabrication_scan_skip_file body should be present"); // audited: contract test assertion path; panic/expect is test-only failure signal
    if skip_body.contains("media_conversion_gate.rs") {
        inventory.push(
            "A0 fabrication_scan_skip_file must not blanket-skip media_conversion_gate.rs (M249)"
                .into(),
        );
    }
    if !silent_fb.contains("fn gate_unified_fabrication_line_exempt") {
        inventory.push(
            "A0 unified fabrication scan must gate-exempt only helper definitions + audit strings"
                .into(),
        );
    }

    let db_vec = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/database_vector.rs",
    ))
    .expect("database_vector.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let db_vec_prod = production_scope(&db_vec);
    if db_vec_prod.contains("None => 0.0_f64") {
        inventory.push(
            "A0 database_vector must route absent KNN dims through knn_absent_feature_component()"
                .into(),
        );
    }
    let db_src = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/database.rs",
    ))
    .expect("database.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let db_prod = production_scope(&db_src);
    if !db_prod.contains("is_knn_bootstrap_heuristic") {
        inventory.push(
            "A0 LoopReferenceProfile must expose is_knn_bootstrap_heuristic (cold-start vs \
             corpus-built)"
                .into(),
        );
    }
    let knn_default_test_only =
        db_src.contains("#[cfg(test)]\nimpl Default for LoopReferenceProfile");
    if db_prod.contains("is_knn_bootstrap_heuristic: true") && !knn_default_test_only {
        inventory.push(
            "A0 production database.rs must not ship KNN bootstrap Default \
             (is_knn_bootstrap_heuristic: true only in #[cfg(test)])"
                .into(),
        );
    }
    if !db_prod.contains("loop_reference_profile_corpus_shell") {
        inventory.push(
            "A0 database must build corpus profiles via loop_reference_profile_corpus_shell (not \
             Default)"
                .into(),
        );
    }
    if db_prod.contains("db_numeric_stats_triple_or_zero(") {
        inventory.push(
            "A0 database must not call db_numeric_stats_triple_or_zero (forged empty-corpus stats)"
                .into(),
        );
    }
    if gate_prod.contains("using (0,0,0) stats") {
        inventory.push("A0 gate must refuse (0,0,0) stats injection on empty corpus".into());
    }
    if gate_prod.contains("empty distance slice; using 0.0") {
        inventory.push("A0 db_sorted_distance_at must not forge 0.0 on empty slice".into());
    }
    if !db_vec_prod.contains(ZERO_TOLERANCE_KNN_ABSENT_HELPER) {
        inventory.push(
            "A0 database_vector must use knn_absent_feature_component for absent optional features"
                .into(),
        );
    }
    let quality_db_src = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/image/image_quality_db.rs",
    ))
    .expect("image_quality_db.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let quality_db_prod = production_scope(&quality_db_src);
    if quality_db_prod.contains("knn_only_prediction(&") {
        inventory.push(
            "B06 image_quality_db production must not call knn_only_prediction for decision scores"
                .into(),
        );
    }
    if quality_db_prod.contains("deliver_log_static_quality")
        && quality_db_prod.contains("StaticQualityDbBranch::FallbackKnnOnly")
    {
        inventory.push(
            "B06 image_quality_db production must not deliver \
             StaticQualityDbBranch::FallbackKnnOnly scores"
                .into(),
        );
    }
    let scenario_quality_src = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/db/scenario_quality_lookup.rs",
    ))
    .expect("scenario_quality_lookup.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let scenario_quality_prod = production_scope(&scenario_quality_src);
    if scenario_quality_prod.contains("knn_only_prediction(&") {
        inventory.push(
            "B06 scenario_quality_lookup production must not call knn_only_prediction for \
             decision scores"
                .into(),
        );
    }
    if scenario_quality_prod.contains("ScenarioQualityBranch::FallbackKnnOnly") {
        inventory.push(
            "B06 scenario_quality_lookup production must not use FallbackKnnOnly decision branch"
                .into(),
        );
    }

    let progress = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/progress.rs",
    ))
    .expect("progress.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let progress_prod = production_scope(&progress);
    if progress_prod.contains("best_ssim: Arc<Mutex<f64>>")
        || progress_prod.contains("best_ssim: f64,")
        || progress_prod.contains("best_crf: Arc<Mutex<f32>>")
        || progress_prod.contains("current_crf: Arc<Mutex<f32>>")
        || (progress_prod.contains("Mutex::new(0.0)") && progress_prod.contains("best_crf"))
    {
        inventory.push(
            "A0 explore progress CRF/SSIM must be Option (unset until measured, not 0.0)".into(),
        );
    }
    if progress_prod.contains("best_crf > 0.0") {
        inventory.push(
            "A0 explore UI must not treat CRF>0 as has-best (use has_best_crf / Option)".into(),
        );
    }
    if !gate_prod.contains("ui_f32_display_or_placeholder") {
        inventory
            .push("A0 gate must expose ui_f32_display_or_placeholder for honest CRF UI".into());
    }
    if !gate_prod.contains("explore_elapsed_secs_optional") {
        inventory.push("A0 gate must expose explore_elapsed_secs_optional (no fake 0.0s)".into());
    }

    let vqf = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/video_quality_features.rs",
    ))
    .expect("video_quality_features.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let vqf_prod = production_scope(&vqf);
    if vqf_prod.contains("probe_optional_f64_or_zero(") {
        inventory.push(
            "A0 video_quality_features must not call probe_optional_f64_or_zero on embed paths"
                .into(),
        );
    }

    let explore = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/explore_strategy.rs",
    ))
    .expect("explore_strategy.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let ssim_map = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/ssim_mapping.rs",
    ))
    .expect("ssim_mapping.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    if explore.contains("psnr_to_ssim_estimate") {
        inventory.push(
            "B08 explore_strategy production must not call psnr_to_ssim_estimate (zero tolerance)"
                .into(),
        );
    }
    if !ssim_map.contains("psnr_to_ssim_estimate") {
        inventory
            .push("A3 ssim_mapping must retain psnr_to_ssim_estimate symbol (unused OK)".into());
    }
    if gate_prod.contains("psnr_to_ssim_estimate") {
        inventory
            .push("A3 media_conversion_gate production must not call psnr_to_ssim_estimate".into());
    }

    let project_hits = production_scope_offenders(
        root,
        PROJECT_DELIVERY_MODULES,
        PROJECT_INLINE_NUMERIC_FORGERY,
    );
    if !project_hits.is_empty() {
        inventory.push(format!(
            "A4 img/vid delivery crates inline numeric fabrication ({} hits):\n  {}",
            project_hits.len(),
            project_hits.join("\n  ")
        ));
    }

    let mut legacy_or_zero_hits = production_legacy_or_zero_call_offenders(
        root,
        ALGORITHM_AUDIT_INFERENCE_MODULES,
        PRODUCTION_FORBIDDEN_LEGACY_OR_ZERO,
    );
    legacy_or_zero_hits.extend(production_legacy_or_zero_call_offenders(
        root,
        PROJECT_DELIVERY_MODULES,
        PRODUCTION_FORBIDDEN_LEGACY_OR_ZERO,
    ));
    if !legacy_or_zero_hits.is_empty() {
        inventory.push(format!(
            "A0 production must not call legacy *_or_zero fabrication helpers ({} hits):\n  {}",
            legacy_or_zero_hits.len(),
            legacy_or_zero_hits.join("\n  ")
        ));
    }

    let vid_conv = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/vid/src/conversion_api.rs",
    ))
    .expect("vid conversion_api.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let vid_prod = production_scope(&vid_conv);
    if vid_prod.contains("last_best_crf = Some(0.0)") {
        inventory.push(
            "A4 vid conversion_api must not fabricate last_best_crf=0.0 on GIF recovery".into(),
        );
    }
}

fn cat_audit_modules() -> Vec<&'static str> {
    let mut modules = ALGORITHM_AUDIT_INFERENCE_MODULES.to_vec();
    modules.extend(PROJECT_DELIVERY_MODULES);
    modules
}

fn production_direct_cast_offenders(
    root: &Path,
    modules: &[&str],
    patterns: &[&str],
    exempt: &[&str],
) -> Vec<String> {
    let mut hits = Vec::new();
    for rel in modules {
        if exempt.contains(rel) {
            continue;
        }
        let path = resolve_workspace_path(root, rel);
        if !path.is_file() {
            continue;
        }
        let content =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for (line_no, line) in prod.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }
            if trimmed.contains("represented as i32") || trimmed.contains("represented as i64") {
                continue;
            }
            if patterns.iter().any(|pat| line.contains(pat)) && !line.contains("numeric_cast::") {
                hits.push(format!("{rel}:{}: {}", line_no + 1, trimmed));
            }
        }
    }
    hits
}

fn check_cat_b_log_coverage(root: &Path, inventory: &mut Vec<String>) {
    let mut missing = Vec::new();
    for rel in cat_audit_modules() {
        if CAT_B_LOG_EXEMPT.contains(&rel) {
            continue;
        }
        let path = resolve_workspace_path(root, rel);
        if !path.is_file() {
            continue;
        }
        let content =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        if !CAT_B_LOG_MARKERS.iter().any(|marker| prod.contains(marker)) {
            missing.push(rel);
        }
    }
    if !missing.is_empty() {
        inventory.push(format!(
            "CAT-B delivery/inference modules missing tracing/ui_stderr/log_* SSOT ({}):\n  {}",
            missing.len(),
            missing.join("\n  ")
        ));
    }
}

fn check_cat_d_numeric_casts(root: &Path, inventory: &mut Vec<String>) {
    let modules = cat_audit_modules();
    let float_hits = production_direct_cast_offenders(
        root,
        &modules,
        CAT_D_FLOAT_CAST_PATTERNS,
        CAT_D_CAST_EXEMPT,
    );
    if !float_hits.is_empty() {
        inventory.push(format!(
            "CAT-D production must not use direct `as f64`/`as f32` (use numeric_cast) ({} \
             hits):\n  {}",
            float_hits.len(),
            float_hits.join("\n  ")
        ));
    }
    let int_hits = production_direct_cast_offenders(
        root,
        &modules,
        CAT_D_INT_CAST_PATTERNS,
        CAT_D_CAST_EXEMPT,
    );
    if !int_hits.is_empty() {
        inventory.push(format!(
            "CAT-D production must not use direct integer `as` casts outside numeric_cast ({} \
             hits):\n  {}",
            int_hits.len(),
            int_hits.join("\n  ")
        ));
    }
}

fn check_tier_b(root: &Path, inventory: &mut Vec<String>) {
    let check_all = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/dev/src/bin/check_all.rs",
    ))
    .expect("check_all.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for test_target in ["runtime_probe_regression", "comprehensive_weakness_audit"] {
        if !check_all.contains(test_target) {
            inventory.push(format!("B1 check_all --ci must run `{test_target}`"));
        }
    }
    let probe = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/dev/src/tests/runtime/runtime_probe_regression.rs",
    ))
    .expect("runtime_probe_regression.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    for sym in [
        "runtime_probe_regression_suite",
        "isobmff_sequence_brand_matrix_probe",
        "avis_gate_static_only_rejection_probe",
        "static_heic_minimal_moov_still_static_probe",
        "static_mif1_compat_not_sequence_probe",
        "zero_tolerance_quality_embed_nan_slots_probe",
    ] {
        if !probe.contains(sym) {
            inventory.push(format!("B2 runtime_probe missing `{sym}`"));
        }
    }
}

fn check_ctrlc_runtime_or_zero_scope(root: &Path, inventory: &mut Vec<String>) {
    let ctrlc = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/ctrlc_guard.rs",
    ))
    .expect("ctrlc_guard.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let ctrlc_prod = production_scope(&ctrlc);
    if !ctrlc_prod.contains(CTRLC_ALLOWED_LEGACY_OR_ZERO) {
        inventory.push(
            "A0 ctrlc_guard must use runtime_elapsed_secs_or_zero (M43 audited interrupt path)"
                .into(),
        );
    }
    let fn_name = CTRLC_ALLOWED_LEGACY_OR_ZERO.trim_end_matches('(');
    for rel in ALGORITHM_AUDIT_INFERENCE_MODULES {
        if rel.ends_with("ctrlc_guard.rs") || rel.ends_with("media_conversion_gate.rs") {
            continue;
        }
        let path = resolve_workspace_path(root, rel);
        if !path.is_file() {
            continue;
        }
        let content =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())); // audited: contract test assertion path; panic/expect is test-only failure signal
        let prod = production_scope(&content);
        for line in prod.lines() {
            let trimmed = line.trim();
            if !trimmed.contains(CTRLC_ALLOWED_LEGACY_OR_ZERO) {
                continue;
            }
            if trimmed.contains(&format!("pub fn {fn_name}")) {
                continue;
            }
            inventory.push(format!(
                "A0 {rel} must not call runtime_elapsed_secs_or_zero (ctrlc_guard only)"
            ));
            break;
        }
    }
}

#[allow(clippy::too_many_lines)]
fn check_tier_c(root: &Path, inventory: &mut Vec<String>) {
    let qrm_src = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/quality_regression_model.rs",
    ))
    .expect("quality_regression_model.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let qrm = production_scope(&qrm_src);
    if !qrm.contains("embed_measurement_slot_json") {
        inventory.push("C1 quality_regression_model missing embed_measurement_slot_json".into());
    }
    let iqd = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/image_quality_db.rs",
    ))
    .expect("image_quality_db.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let iqd_prod = production_scope(&iqd);
    if !iqd_prod.contains("sanitize_stale_quality_measurement_embed_slots") {
        inventory.push(
            "C5 image_quality_db must sanitize stale embed 17/18 0.0→NaN at runtime (M235/M246)"
                .into(),
        );
    }
    if !iqd_prod.contains("sanitized_quality_embedding_for_use") {
        inventory.push(
            "C5 image_quality_db KNN path must sanitize query embeddings before lookup".into(),
        );
    }
    if !qrm.contains("sanitize_stale_quality_measurement_embed_slots") {
        inventory.push(
            "C5 quality_regression_model LightGBM payload must sanitize embed 17/18 before \
             inference"
                .into(),
        );
    }
    let py = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/dev/scripts/quality_regression_model.py",
    ))
    .expect("quality_regression_model.py"); // audited: contract test assertion path; panic/expect is test-only failure signal
    if !py.contains("normalize_nullable_embed_slots") {
        inventory.push("C2 training script missing normalize_nullable_embed_slots".into());
    }
    let normalize_script =
        root.join("crates/dev/src/bin/normalize_stale_embed_measurement_slots.rs");
    if normalize_script.is_file() {
        let normalize_rs = fs::read_to_string(&normalize_script).expect("normalize script"); // audited: contract test assertion path; panic/expect is test-only failure signal
        if !normalize_rs.contains("EMBED_SLOT_INDICES")
            || !normalize_rs.contains("PGVECTOR_MISSING_MEASUREMENT")
        {
            inventory.push(
                "C3 normalize_stale_embed_measurement_slots.rs must rewrite optional slots to \
                 pgvector-safe sentinel"
                    .into(),
            );
        }
    } else {
        inventory.push(
            "C3 missing normalize_stale_embed_measurement_slots.rs (DB sentinel backfill)".into(),
        );
    }

    let batch = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/foundation/src/batch.rs",
    ))
    .expect("batch.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let batch_prod = production_scope(&batch);
    for ban in [
        "delivery_path_modified_unix_secs_or_zero(",
        "delivery_batch_relative_depth_or_zero(",
    ] {
        if batch_prod.contains(ban) {
            inventory.push(format!(
                "A0 batch.rs must not call legacy `{ban}` (use *_optional + Option sort keys)"
            ));
        }
    }
    if !batch_prod.contains("delivery_path_modified_unix_secs_optional") {
        inventory.push("A0 batch.rs must use delivery_path_modified_unix_secs_optional".into());
    }
    if !batch_prod.contains("delivery_batch_relative_depth_optional") {
        inventory.push("A0 batch.rs must use delivery_batch_relative_depth_optional".into());
    }

    check_ctrlc_runtime_or_zero_scope(root, inventory);

    let contract = read_hardening_doc(root, "MEDIA_CONVERSION_LAYER_CONTRACT.md");
    if !contract.contains("comprehensive_weakness_audit") {
        inventory.push("DOC contract must reference comprehensive_weakness_audit CI test".into());
    }
    if !contract.contains("零容忍") {
        inventory.push("DOC contract must document zero-fabrication policy (M248)".into());
    }

    let check_all = fs::read_to_string(resolve_workspace_path(
        root,
        "crates/dev/src/bin/check_all.rs",
    ))
    .expect("check_all.rs"); // audited: contract test assertion path; panic/expect is test-only failure signal
    let ci_quality = fs::read_to_string(resolve_workspace_path(
        root,
        ".github/workflows/ci-quality.yml",
    ))
    .expect("ci-quality.yml"); // audited: contract test assertion path; panic/expect is test-only failure signal
    if ci_quality.contains("extractions/setup-just@v2") {
        inventory.push(
            "CI ci-quality must not use extractions/setup-just@v2 (Node 20 deprecated on runners)"
                .into(),
        );
    }
    if !ci_quality.contains("extractions/setup-just@v4") {
        inventory.push(
            "CI ci-quality must use extractions/setup-just@v4 (composite action, Node-neutral)"
                .into(),
        );
    }
    if !check_all.contains("normalize_stale_embed_measurement_slots.rs") {
        inventory.push(
            "C4 check_all --ci must reference normalize_stale_embed_measurement_slots.rs (DB \
             backfill SSOT)"
                .into(),
        );
    }
}

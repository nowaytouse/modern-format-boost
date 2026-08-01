use crate::image_analyzer::ImageAnalysis;
use crate::multi_scenario_db::KnnRegressionFeatures;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const IMAGE_QUALITY_MODEL_NAME: &str = "lightgbm_model.txt";
const IMAGE_QUALITY_MODEL_METADATA_NAME: &str = "lightgbm_model.metadata.json";
const IMAGE_QUALITY_MODEL_SCHEMA: &str = "image_quality_lgbm_v1";
const IMAGE_QUALITY_MODEL_SCENARIO: &str = "image_quality";
const IMAGE_QUALITY_MODEL_PREDICTOR_FAMILY: &str = "lightgbm_binary";
/// Upper bound on on-disk `LightGBM` artifact size (reject corrupt/huge files
/// before subprocess).
const IMAGE_QUALITY_MODEL_MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct ImageQualityModelMetadata {
    scenario: String,
    feature_schema: String,
    #[serde(default)]
    predictor_family: Option<String>,
    #[serde(default)]
    feature_names: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ModelQualityPrediction {
    pub score: f64,
    pub confidence: f64,
}

#[derive(Debug, Deserialize)]
struct PythonModelPrediction {
    score: f64,
    confidence: f64,
    feature_schema: String,
    predictor_family: String,
}

/// Run the real `LightGBM` image-quality regressor when a trained model exists.
///
/// Subprocess contract:
/// - Stdin is closed after the JSON feature payload is written (EOF signals end
///   of request).
/// - Wall-clock is bounded by
///   [`crate::constants::IMAGE_QUALITY_MODEL_INFERENCE_TIMEOUT_SECS`] or
///   `MFB_IMAGE_QUALITY_MODEL_INFERENCE_TIMEOUT_SECS` (5–600 seconds); overrun
///   kills the child.
///
/// # Errors
/// Returns an error when feature extraction, subprocess execution, or response
/// decoding fails after a real model was requested.
pub fn predict_image_quality(
    analysis: &ImageAnalysis,
    embedding: &pgvector::Vector,
    knn: &KnnRegressionFeatures,
) -> Result<Option<ModelQualityPrediction>> {
    if quality_model_env_truthy(crate::constants::ENV_DISABLE_IMAGE_QUALITY_MODEL) {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "static_image_quality_lgbm",
            branch = "model_disabled",
            "LightGBM subprocess skipped (ENV_DISABLE_IMAGE_QUALITY_MODEL)"
        );
        return Ok(None);
    }

    let model_path = resolve_model_path()?;
    let metadata_path = resolve_metadata_path()?;
    if !model_path.exists() || !metadata_path.exists() {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "static_image_quality_lgbm",
            branch = "artifacts_missing",
            model_exists = model_path.exists(),
            metadata_exists = metadata_path.exists(),
            "LightGBM artifacts not present; caller should bootstrap"
        );
        return Ok(None);
    }

    validate_model_artifacts(&model_path, &metadata_path)?;
    let knn = knn
        .clone()
        .seal_aggregates()
        .ok_or_else(|| anyhow::anyhow!("KNN aggregates failed quality seal"))?;
    let feature_payload = build_feature_payload(analysis, embedding, &knn)?;
    let script_path = resolve_script_path();
    if !script_path.exists() {
        anyhow::bail!(
            "image quality model script not found at {}",
            script_path.display()
        );
    }

    let python = resolve_python_command();
    let input = serde_json::to_vec(&json!({ "features": feature_payload }))
        .context("Failed to serialize image quality model request")?;

    let mut child = Command::new(&python)
        .arg(&script_path)
        .arg("predict-image-quality")
        .arg("--model")
        .arg(&model_path)
        .arg("--metadata")
        .arg(&metadata_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "Failed to launch image quality model runtime via '{}' and '{}'",
                python,
                script_path.display()
            )
        })?;

    {
        let mut stdin = child
            .stdin
            .take()
            .context("Failed to open stdin for image quality model runtime")?;
        stdin
            .write_all(&input)
            .context("Failed to send feature payload to image quality model runtime")?;
    }

    let output = wait_child_output_with_timeout(child, inference_timeout())?;

    if output.stdout.is_empty() {
        anyhow::bail!("image quality model runtime returned empty stdout");
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr_snip: String = stderr.chars().take(512).collect();
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "static_image_quality_lgbm",
            branch = "subprocess_failed",
            status = ?output.status.code(),
            stderr = %stderr_snip,
            "LightGBM subprocess failed"
        );
        anyhow::bail!(
            "image quality model runtime failed (status {:?}): {}",
            output.status.code(),
            stderr.trim()
        );
    }

    let response: PythonModelPrediction = serde_json::from_slice(&output.stdout)
        .context("Failed to parse image quality model response JSON")?;
    anyhow::ensure!(
        response.feature_schema == IMAGE_QUALITY_MODEL_SCHEMA,
        "image quality model schema mismatch: expected {}, got {}",
        IMAGE_QUALITY_MODEL_SCHEMA,
        response.feature_schema
    );
    anyhow::ensure!(
        response.predictor_family == "lightgbm_binary",
        "unexpected predictor family from image quality model: {}",
        response.predictor_family
    );
    anyhow::ensure!(
        response.score.is_finite(),
        "image quality model returned non-finite score: {:?}",
        response.score
    );
    anyhow::ensure!(
        response.confidence.is_finite(),
        "image quality model returned non-finite confidence: {:?}",
        response.confidence
    );
    let slack = crate::constants::QUALITY_MODEL_PROBABILITY_SLACK;
    anyhow::ensure!(
        response.score >= -slack && response.score <= 1.0 + slack,
        "image quality model score outside [0,1] (slack={slack}): {}",
        response.score
    );
    anyhow::ensure!(
        response.confidence >= -slack && response.confidence <= 1.0 + slack,
        "image quality model confidence outside [0,1] (slack={slack}): {}",
        response.confidence
    );

    let (score, confidence) =
        crate::algorithm_seal::quality_probability_pair(response.score, response.confidence)
            .context("image quality model returned non-sealable score/confidence pair")?;

    tracing::debug!(
        target: "mfb.algorithm",
        pipeline = "static_image_quality_lgbm",
        branch = "prediction_ok",
        score,
        confidence,
        neighbor_count = knn.neighbor_count,
        "LightGBM subprocess prediction sealed"
    );

    Ok(Some(ModelQualityPrediction { score, confidence }))
}

fn quality_model_env_truthy(name: &'static str) -> bool {
    match std::env::var(name) {
        Ok(value) => value == "1" || value.eq_ignore_ascii_case("true"),
        Err(std::env::VarError::NotPresent) => false,
        Err(err) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "quality_model_env_read",
                format!("{name} could not be read: {err}"),
            );
            false
        }
    }
}

fn quality_model_env_path(name: &'static str) -> Option<PathBuf> {
    match std::env::var(name) {
        Ok(path) => {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(PathBuf::from(trimmed))
            }
        }
        Err(std::env::VarError::NotPresent) => None,
        Err(err) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "quality_model_env_read",
                format!("{name} could not be read: {err}"),
            );
            None
        }
    }
}

/// Reject missing, empty, oversized, or contract-mismatched model bundles.
fn validate_model_artifacts(model_path: &Path, metadata_path: &Path) -> Result<()> {
    let model_meta = std::fs::metadata(model_path).with_context(|| {
        format!(
            "failed to stat image quality model at {}",
            model_path.display()
        )
    })?;
    anyhow::ensure!(
        model_meta.len() > 0,
        "image quality model file is empty at {}",
        model_path.display()
    );
    anyhow::ensure!(
        model_meta.len() <= IMAGE_QUALITY_MODEL_MAX_BYTES,
        "image quality model file exceeds {} bytes at {}",
        IMAGE_QUALITY_MODEL_MAX_BYTES,
        model_path.display()
    );
    validate_model_metadata(metadata_path)
}

/// Reject stale or cross-scenario metadata before spawning Python.
fn validate_model_metadata(metadata_path: &Path) -> Result<()> {
    let raw = load_model_metadata_raw(metadata_path)?;
    parse_and_validate_model_metadata(&raw, metadata_path)
}

fn load_model_metadata_raw(metadata_path: &Path) -> Result<String> {
    std::fs::read_to_string(metadata_path).with_context(|| {
        format!(
            "failed to read image quality model metadata at {}",
            metadata_path.display()
        )
    })
}

fn parse_and_validate_model_metadata(raw: &str, metadata_path: &Path) -> Result<()> {
    let meta: ImageQualityModelMetadata = serde_json::from_str(raw).with_context(|| {
        format!(
            "failed to parse image quality model metadata at {}",
            metadata_path.display()
        )
    })?;
    anyhow::ensure!(
        meta.scenario == IMAGE_QUALITY_MODEL_SCENARIO,
        "image quality model scenario mismatch: expected {}, got {}",
        IMAGE_QUALITY_MODEL_SCENARIO,
        meta.scenario
    );
    anyhow::ensure!(
        meta.feature_schema == IMAGE_QUALITY_MODEL_SCHEMA,
        "image quality model schema mismatch: expected {}, got {}",
        IMAGE_QUALITY_MODEL_SCHEMA,
        meta.feature_schema
    );
    if let Some(family) = meta.predictor_family.as_deref()
        && family != IMAGE_QUALITY_MODEL_PREDICTOR_FAMILY
    {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "static_image_quality_lgbm",
            branch = "metadata_predictor_family_mismatch",
            expected = IMAGE_QUALITY_MODEL_PREDICTOR_FAMILY,
            actual = family,
            "metadata predictor_family differs from runtime contract (continuing)"
        );
    }
    if let Some(names) = meta.feature_names.as_ref() {
        anyhow::ensure!(
            !names.is_empty(),
            "image quality model metadata has empty feature_names"
        );
    }
    Ok(())
}

fn inference_timeout() -> Duration {
    let default_secs = crate::constants::IMAGE_QUALITY_MODEL_INFERENCE_TIMEOUT_SECS;
    match std::env::var(crate::constants::ENV_MFB_IMAGE_QUALITY_MODEL_INFERENCE_TIMEOUT_SECS) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(secs) if (5..=600).contains(&secs) => Duration::from_secs(secs),
            Ok(secs) => {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "quality_model_timeout",
                    format!(
                        "inference timeout {secs}s outside 5..=600; using default {default_secs}s"
                    ),
                );
                Duration::from_secs(default_secs)
            }
            Err(e) => {
                crate::media_conversion_gate::probe_quality_batch_audit(
                    "quality_model_timeout",
                    format!(
                        "failed to parse inference timeout '{raw}': {e}; using default \
                         {default_secs}s"
                    ),
                );
                Duration::from_secs(default_secs)
            }
        },
        Err(std::env::VarError::NotPresent) => Duration::from_secs(default_secs),
        Err(e) => {
            crate::media_conversion_gate::probe_quality_batch_audit(
                "quality_model_timeout",
                format!("failed to read inference timeout env: {e}; using default {default_secs}s"),
            );
            Duration::from_secs(default_secs)
        }
    }
}

/// Bounded wait with kill-on-timeout so a wedged Python cannot stall the
/// encoder indefinitely.
fn wait_child_output_with_timeout(
    mut child: Child,
    timeout: Duration,
) -> Result<std::process::Output> {
    let deadline = Instant::now() + timeout;
    let cap = crate::constants::IMAGE_QUALITY_MODEL_MAX_IO_BYTES;
    let stdout_reader = child
        .stdout
        .take()
        .map(|stdout| spawn_child_stream_reader(stdout, cap, "stdout"));
    let stderr_reader = child
        .stderr
        .take()
        .map(|stderr| spawn_child_stream_reader(stderr, cap, "stderr"));

    loop {
        if let Some(status) = child
            .try_wait()
            .context("image quality subprocess try_wait")?
        {
            return Ok(std::process::Output {
                status,
                stdout: collect_child_stream_reader(stdout_reader, "stdout")?,
                stderr: collect_child_stream_reader(stderr_reader, "stderr")?,
            });
        }
        if Instant::now() > deadline {
            let status = kill_quality_model_after_timeout(&mut child, timeout)?;
            audit_timeout_reader_result(collect_child_stream_reader(stdout_reader, "stdout"));
            audit_timeout_reader_result(collect_child_stream_reader(stderr_reader, "stderr"));
            anyhow::bail!(
                "image quality model runtime timed out after {timeout:?} (subprocess killed, \
                 exit={:?})",
                status.code()
            );
        }
        thread::sleep(Duration::from_millis(45));
    }
}

fn spawn_child_stream_reader(
    reader: impl Read + Send + 'static,
    max_bytes: usize,
    label: &'static str,
) -> JoinHandle<Result<Vec<u8>>> {
    thread::spawn(move || read_child_stream_limited(reader, max_bytes, label))
}

fn collect_child_stream_reader(
    reader: Option<JoinHandle<Result<Vec<u8>>>>,
    label: &'static str,
) -> Result<Vec<u8>> {
    match reader {
        Some(handle) => handle.join().map_err(|err| {
            anyhow::anyhow!("image quality model {label} reader panicked: {err:?}")
        })?,
        None => Ok(Vec::new()),
    }
}

fn audit_timeout_reader_result(result: Result<Vec<u8>>) {
    if let Err(err) = result {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "quality_model_timeout_drain",
            format!("image quality model timeout drain failed: {err}"),
        );
    }
}

fn kill_quality_model_after_timeout(child: &mut Child, timeout: Duration) -> Result<ExitStatus> {
    match child.kill() {
        Ok(()) => child
            .wait()
            .context("failed to reap timed-out image quality model subprocess"),
        Err(kill_err) => {
            if let Some(status) = child
                .try_wait()
                .context("image quality subprocess try_wait after kill failure")?
            {
                return Ok(status);
            }
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "quality_model_timeout_kill",
                format!("failed to kill image quality model after {timeout:?}: {kill_err}"),
            );
            anyhow::bail!(
                "image quality model runtime timed out after {timeout:?} and kill failed: \
                 {kill_err}"
            );
        }
    }
}

fn read_child_stream_limited(
    reader: impl Read,
    max_bytes: usize,
    label: &'static str,
) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut limited = reader.take(crate::numeric_cast::usize_to_u64(
        max_bytes.saturating_add(1),
    ));
    limited
        .read_to_end(&mut buf)
        .with_context(|| format!("read image quality model {label}"))?;
    if buf.len() > max_bytes {
        anyhow::bail!(
            "image quality model {label} exceeded {max_bytes} bytes (hard cap for JSON subprocess \
             output)"
        );
    }
    Ok(buf)
}

fn resolve_model_path() -> Result<PathBuf> {
    if let Some(path) = quality_model_env_path(crate::constants::ENV_MFB_IMAGE_QUALITY_MODEL_PATH) {
        return Ok(path);
    }
    Ok(default_model_dir()?.join(IMAGE_QUALITY_MODEL_NAME))
}

fn resolve_metadata_path() -> Result<PathBuf> {
    if let Some(path) =
        quality_model_env_path(crate::constants::ENV_MFB_IMAGE_QUALITY_MODEL_METADATA_PATH)
    {
        return Ok(path);
    }
    Ok(default_model_dir()?.join(IMAGE_QUALITY_MODEL_METADATA_NAME))
}

fn default_model_dir() -> Result<PathBuf> {
    let mut dir = crate::common_utils::get_user_project_cache_dir()?;
    dir.push("models");
    dir.push("image_quality");
    Ok(dir)
}

fn resolve_python_command() -> String {
    crate::media_conversion_gate::delivery_quality_model_python_command_or_default()
}

fn resolve_script_path() -> PathBuf {
    if let Some(path) = quality_model_env_path(crate::constants::ENV_MFB_IMAGE_QUALITY_MODEL_SCRIPT)
    {
        return path;
    }

    if let Some(packaged) = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .map(|root| {
            root.join("crates")
                .join("dev")
                .join("scripts")
                .join("quality_regression_model.py")
        })
        .filter(|path| path.is_file())
    {
        return packaged;
    }

    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(root) = crate_dir.parent().and_then(Path::parent) {
        return root
            .join("crates")
            .join("dev")
            .join("scripts")
            .join("quality_regression_model.py");
    }
    crate_dir.join("quality_regression_model.py")
}

fn build_feature_payload(
    analysis: &ImageAnalysis,
    embedding: &pgvector::Vector,
    knn: &KnnRegressionFeatures,
) -> Result<Map<String, Value>> {
    let mut embedding_vec = embedding.to_vec();
    crate::image_quality_db::sanitize_stale_quality_measurement_embed_slots(&mut embedding_vec);
    let embedding = pgvector::Vector::from(embedding_vec);
    let entropy = analysis
        .features
        .entropy
        .ok_or_else(|| anyhow::anyhow!("entropy unavailable for image quality model inference"))?;
    let compression_ratio = analysis.features.compression_ratio.ok_or_else(|| {
        anyhow::anyhow!("compression_ratio unavailable for image quality model inference")
    })?;
    anyhow::ensure!(
        entropy.is_finite(),
        "entropy must be finite for model inference"
    );
    anyhow::ensure!(
        compression_ratio.is_finite(),
        "compression_ratio must be finite for model inference"
    );

    let width = f64::from(analysis.width.max(1));
    let height = f64::from(analysis.height.max(1));
    let file_size_bytes = crate::numeric_cast::u64_to_f64(analysis.file_size.max(1));
    let total_pixels = (width * height).max(1.0);
    let spatial_bpp = crate::algorithm_seal::quality_finite_scalar(file_size_bytes / total_pixels)
        .ok_or_else(|| anyhow::anyhow!("non-finite spatial_bpp for model inference"))?;
    let heuristic_score = crate::algorithm_seal::quality_finite_scalar(
        crate::image_quality_db::bpp_heuristic_score(analysis)?,
    )
    .ok_or_else(|| anyhow::anyhow!("non-finite bpp heuristic for model inference"))?;
    let neighbor_coverage = crate::algorithm_seal::quality_unit_probability(
        crate::numeric_cast::usize_to_f64(knn.neighbor_count)
            / crate::numeric_cast::usize_to_f64(crate::image_quality_db::IMAGE_QUALITY_KNN_K),
    )
    .ok_or_else(|| anyhow::anyhow!("non-finite neighbor_coverage for model inference"))?;

    let mut map = Map::new();
    map.insert("width".into(), json!(width));
    map.insert("height".into(), json!(height));
    map.insert("file_size_bytes".into(), json!(file_size_bytes));
    map.insert("total_pixels".into(), json!(total_pixels));
    map.insert("entropy".into(), json!(entropy));
    map.insert("compression_ratio".into(), json!(compression_ratio));
    map.insert("spatial_bpp".into(), json!(spatial_bpp));
    map.insert("log_total_pixels".into(), json!(total_pixels.log10()));
    map.insert("log_file_size_bytes".into(), json!(file_size_bytes.log10()));
    map.insert(
        "log_spatial_bpp".into(),
        json!(spatial_bpp.max(0.0).ln_1p()),
    );
    map.insert("aspect_ratio".into(), json!(width / height));
    map.insert(
        "is_lossless".into(),
        json!(if analysis.is_lossless { 1.0 } else { 0.0 }),
    );
    map.insert("bpp_heuristic_score".into(), json!(heuristic_score));
    map.insert("psnr_measured".into(), json!(analysis.psnr.is_some()));
    map.insert("ssim_measured".into(), json!(analysis.ssim.is_some()));

    append_format_flags(&mut map, &analysis.format);

    anyhow::ensure!(
        knn.is_usable_for_regression(),
        "KNN regression features failed usability gate for model inference"
    );

    let knn_score_mean_k5 = crate::algorithm_seal::quality_finite_scalar(knn.knn_score_mean_k5)
        .ok_or_else(|| anyhow::anyhow!("non-finite knn_score_mean_k5"))?;
    let knn_score_std_k5 = crate::algorithm_seal::quality_finite_scalar(knn.knn_score_std_k5)
        .ok_or_else(|| anyhow::anyhow!("non-finite knn_score_std_k5"))?;
    let knn_score_min_k5 = crate::algorithm_seal::quality_finite_scalar(knn.knn_score_min_k5)
        .ok_or_else(|| anyhow::anyhow!("non-finite knn_score_min_k5"))?;
    let dist_to_nearest = crate::algorithm_seal::quality_finite_scalar(knn.dist_to_nearest)
        .ok_or_else(|| anyhow::anyhow!("non-finite dist_to_nearest"))?;
    let dist_weighted_score = crate::algorithm_seal::quality_finite_scalar(knn.dist_weighted_score)
        .ok_or_else(|| anyhow::anyhow!("non-finite dist_weighted_score"))?;
    let knn_confidence = crate::algorithm_seal::quality_unit_probability(knn.confidence)
        .ok_or_else(|| anyhow::anyhow!("non-finite knn_confidence"))?;
    map.insert("knn_score_mean_k5".into(), json!(knn_score_mean_k5));
    map.insert("knn_score_std_k5".into(), json!(knn_score_std_k5));
    map.insert("knn_score_min_k5".into(), json!(knn_score_min_k5));
    map.insert("dist_to_nearest".into(), json!(dist_to_nearest));
    map.insert("dist_weighted_score".into(), json!(dist_weighted_score));
    map.insert("knn_confidence".into(), json!(knn_confidence));
    map.insert(
        "knn_neighbor_count".into(),
        json!(crate::numeric_cast::usize_to_f64(knn.neighbor_count)),
    );
    map.insert("knn_neighbor_coverage".into(), json!(neighbor_coverage));
    map.insert(
        "knn_available".into(),
        json!(if knn.neighbor_count > 0 { 1.0 } else { 0.0 }),
    );

    let vector = embedding.as_slice();
    anyhow::ensure!(
        vector.len() == 256,
        "image quality model expects 256D embedding, got {}",
        vector.len()
    );
    for (index, value) in vector.iter().enumerate() {
        let v = f64::from(*value);
        anyhow::ensure!(
            v.is_finite(),
            "embedding_{index} must be finite for model inference"
        );
        let key = format!("embedding_{index:03}");
        let payload = match index {
            12 => embed_measurement_slot_json(
                *value,
                analysis.color_depth.is_some(),
                "embedding_012",
                "bit_depth",
            ),
            17 => embed_measurement_slot_json(
                *value,
                analysis.psnr.is_some_and(f64::is_finite),
                "embedding_017",
                "psnr",
            ),
            18 => embed_measurement_slot_json(
                *value,
                analysis.ssim.is_some_and(f64::is_finite),
                "embedding_018",
                "ssim",
            ),
            19 => embed_measurement_slot_json(
                *value,
                analysis.jpeg_analysis.is_some(),
                "embedding_019",
                "jpeg_quality",
            ),
            20 => embed_measurement_slot_json(
                *value,
                analysis.jpeg_analysis.is_some(),
                "embedding_020",
                "jpeg_confidence",
            ),
            _ => json!(v),
        };
        map.insert(key, payload);
    }

    Ok(map)
}

fn embed_measurement_slot_json(
    vector_component: f32,
    measured: bool,
    slot: &str,
    metric: &str,
) -> serde_json::Value {
    if measured {
        return json!(f64::from(vector_component));
    }
    if (vector_component - crate::image_quality_db::QUALITY_EMBED_MISSING_MEASUREMENT).abs()
        > f32::EPSILON
        && vector_component.abs() > f32::EPSILON
    {
        crate::media_conversion_gate::delivery_runtime_batch_audit(
            "quality_regression_stale_embed",
            format!("{slot}={vector_component} in vector but {metric} not measured; using null"),
        );
    }
    serde_json::Value::Null
}

fn append_format_flags(map: &mut Map<String, Value>, format: &str) {
    let format_lower = format.trim().to_ascii_lowercase();
    let is_png = format_lower.contains("png");
    let is_jpeg = format_lower.contains("jpeg") || format_lower.contains("jpg");
    let is_webp = format_lower.contains("webp");
    let is_tiff = format_lower.contains("tiff") || format_lower.contains("tif");
    let is_avif = format_lower.contains("avif");
    let is_heic = format_lower.contains("heic") || format_lower.contains("heif");
    let is_jxl = format_lower.contains("jxl") || format_lower.contains("jpeg-xl");
    let is_other = !(is_png || is_jpeg || is_webp || is_tiff || is_avif || is_heic || is_jxl);

    map.insert("fmt_png".into(), json!(if is_png { 1.0 } else { 0.0 }));
    map.insert("fmt_jpeg".into(), json!(if is_jpeg { 1.0 } else { 0.0 }));
    map.insert("fmt_webp".into(), json!(if is_webp { 1.0 } else { 0.0 }));
    map.insert("fmt_tiff".into(), json!(if is_tiff { 1.0 } else { 0.0 }));
    map.insert("fmt_avif".into(), json!(if is_avif { 1.0 } else { 0.0 }));
    map.insert("fmt_heic".into(), json!(if is_heic { 1.0 } else { 0.0 }));
    map.insert("fmt_jxl".into(), json!(if is_jxl { 1.0 } else { 0.0 }));
    map.insert("fmt_other".into(), json!(if is_other { 1.0 } else { 0.0 }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_analyzer::{ImageAnalysis, ImageFeatures};
    use crate::types::Visual;

    #[test]
    fn test_build_feature_payload_includes_expected_keys() {
        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: true,
            features: ImageFeatures {
                entropy: Some(6.5),
                compression_ratio: Some(2.0),
            },
            perception: Visual {
                average_luma: 0.5,
                peak_luma: 0.8,
                gray_center_of_mass: (0.5, 0.5),
            },
            physics_225: Some(vec![0.25; 225]),
            ..ImageAnalysis::default()
        };
        let embedding = pgvector::Vector::from(vec![0.25_f32; 256]);
        let payload = build_feature_payload(
            &analysis,
            &embedding,
            &KnnRegressionFeatures {
                knn_score_mean_k5: 0.7,
                knn_score_std_k5: 0.1,
                knn_score_min_k5: 0.5,
                dist_to_nearest: 0.2,
                dist_weighted_score: 0.8,
                confidence: 0.9,
                neighbor_count: 5,
            },
        )
        .expect("payload should build");

        assert_eq!(payload.get("fmt_png"), Some(&json!(1.0)));
        assert_eq!(payload.get("knn_available"), Some(&json!(1.0)));
        assert!(payload.contains_key("embedding_000"));
        assert!(payload.contains_key("embedding_255"));
        assert_eq!(payload.get("embedding_012"), Some(&serde_json::Value::Null));
        assert_eq!(payload.get("psnr_measured"), Some(&json!(false)));
        assert_eq!(payload.get("ssim_measured"), Some(&json!(false)));
        assert_eq!(payload.get("embedding_017"), Some(&serde_json::Value::Null));
        assert_eq!(payload.get("embedding_018"), Some(&serde_json::Value::Null));
        assert_eq!(payload.get("embedding_019"), Some(&serde_json::Value::Null));
        assert_eq!(payload.get("embedding_020"), Some(&serde_json::Value::Null));
        assert_eq!(payload.len(), 288);
    }

    #[test]
    fn build_feature_payload_nulls_stale_nonzero_embed_when_unmeasured() {
        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: true,
            features: ImageFeatures {
                entropy: Some(6.5),
                compression_ratio: Some(2.0),
            },
            perception: Visual {
                average_luma: 0.5,
                peak_luma: 0.8,
                gray_center_of_mass: (0.5, 0.5),
            },
            physics_225: Some(vec![0.25; 225]),
            ..ImageAnalysis::default()
        };
        let mut vec = vec![0.25_f32; 256];
        vec[17] = 0.42;
        vec[18] = 0.33;
        let embedding = pgvector::Vector::from(vec);
        let knn = KnnRegressionFeatures {
            knn_score_mean_k5: 0.7,
            knn_score_std_k5: 0.1,
            knn_score_min_k5: 0.5,
            dist_to_nearest: 0.2,
            dist_weighted_score: 0.8,
            confidence: 0.9,
            neighbor_count: 5,
        };
        let payload = build_feature_payload(&analysis, &embedding, &knn).expect("payload");
        assert_eq!(payload.get("embedding_017"), Some(&serde_json::Value::Null));
        assert_eq!(payload.get("embedding_018"), Some(&serde_json::Value::Null));

        let mut measured = analysis;
        measured.psnr = Some(40.0);
        measured.ssim = Some(0.95);
        let payload_measured =
            build_feature_payload(&measured, &embedding, &knn).expect("payload measured");
        let e17 = payload_measured
            .get("embedding_017")
            .and_then(serde_json::Value::as_f64)
            .expect("embedding_017");
        let e18 = payload_measured
            .get("embedding_018")
            .and_then(serde_json::Value::as_f64)
            .expect("embedding_018");
        assert!((e17 - 0.42_f64).abs() < 1e-5, "embedding_017 got {e17}");
        assert!((e18 - 0.33_f64).abs() < 1e-5, "embedding_018 got {e18}");
    }

    #[test]
    fn validate_model_metadata_rejects_wrong_schema() {
        let dir = std::env::temp_dir().join(format!("mfb_lgbm_meta_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bad.metadata.json");
        std::fs::write(
            &path,
            r#"{"scenario":"image_quality","feature_schema":"wrong_schema_v0"}"#,
        )
        .expect("write temp metadata");
        let err = super::validate_model_metadata(&path).expect_err("wrong schema");
        assert!(err.to_string().contains("schema mismatch"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_read_child_stream_limited_rejects_oversized_payload() {
        use super::read_child_stream_limited;
        use std::io::Cursor;
        let big = vec![b'x'; crate::constants::IMAGE_QUALITY_MODEL_MAX_IO_BYTES + 1];
        let err = read_child_stream_limited(Cursor::new(big), 64, "test")
            .expect_err("oversized stream should fail");
        assert!(err.to_string().contains("exceeded"));
    }

    #[cfg(unix)]
    #[test]
    fn wait_child_output_with_timeout_kills_slow_child() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("sleep 2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn slow child");

        let err = wait_child_output_with_timeout(child, Duration::from_millis(10))
            .expect_err("slow child must time out");

        assert!(err.to_string().contains("timed out"));
    }

    #[cfg(unix)]
    #[test]
    fn wait_child_output_with_timeout_rejects_oversized_stdout_without_pipe_deadlock() {
        let child = Command::new("sh")
            .arg("-c")
            .arg("dd if=/dev/zero bs=1024 count=600 2>/dev/null")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn oversized stdout child");

        let err = wait_child_output_with_timeout(child, Duration::from_secs(5))
            .expect_err("oversized stdout must fail");

        assert!(
            err.to_string().contains("exceeded"),
            "unexpected error: {err:?}"
        );
    }
}

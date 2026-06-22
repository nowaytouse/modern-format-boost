//! Static Image Quality Database (Multi-Scenario Architecture)
//!
//! Provides a hybrid quality-regression predictor for static (non-animated)
//! images.
//! Strictly aligned with the `001_multi_scenario_embedding.sql` schema.
//!
//! ## DB predictor ordering (audit contract)
//!
//! Callers must not insert stages without updating this list and the tracing
//! branch labels in `lookup_image_quality_with_path`:
//! 1. Animated inputs with `Some(path)` → delegated to `scenario_quality_lookup`.
//! 2. Animated inputs without path → `None` (caller must supply path).
//! 3. DB disabled → heuristic only (no TCP).
//! 3. `PostgreSQL` unavailable → heuristic only (no TCP).
//! 4. Corpus immature → refuse score (`None` + audit).
//! 5. Embedding extraction failed → refuse score.
//! 6. KNN returned zero rows / unusable / query error → refuse score.
//! 7. `LightGBM` subprocess when artifacts exist and model env not disabled.
//! 8. `LightGBM` missing or error → **`None`** (no KNN-only / hybrid bootstrap decision scores).
//! 9. `ENV_FORCE_QUALITY_KNN` → refused (`force_knn_env_refused`).
//! 10. Inference log on successful `LightGbm` branch only (no fabricated Phase-34 fallback).
//!
//! Features:
//! - 256D Physics-based Embedding (Color Moments, DCT, HOG, Entropy, BPP)
//! - KNN regression features for hybrid predictors.
//! - BYTEA blake3 hash support for high-performance indexing.

use crate::image_analyzer::ImageAnalysis;
use crate::scenario::ScenarioType;
use anyhow::{Context, Result};
use postgres::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

// ── Audit-stable branch labels (static image quality DB stack) ───────────────

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaticQualityDbBranch {
    DbDisabledHeuristic,
    DbUnavailableHeuristic,
    CorpusImmatureHeuristic,
    FeatureExtractionFailedHeuristic,
    KnnNoNeighborsHeuristic,
    KnnQueryFailedHeuristic,
    LightGbm,
    LightGbmUnavailableAbort,
    ForceKnnEnvRefused,
    PredictorOutputSealFailed,
    KnnFeaturesUnusableHeuristic,
    AnimatedRoutedToScenarioDb,
    AnimatedNoPath,
}

impl StaticQualityDbBranch {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DbDisabledHeuristic => "db_disabled_heuristic",
            Self::DbUnavailableHeuristic => "db_unavailable_heuristic",
            Self::CorpusImmatureHeuristic => "corpus_immature_heuristic",
            Self::FeatureExtractionFailedHeuristic => "feature_extraction_failed_heuristic",
            Self::KnnNoNeighborsHeuristic => "knn_no_neighbors_heuristic",
            Self::KnnQueryFailedHeuristic => "knn_query_failed_heuristic",
            Self::LightGbm => "lightgbm",
            Self::LightGbmUnavailableAbort => "lightgbm_unavailable_abort",
            Self::ForceKnnEnvRefused => "force_knn_env_refused",
            Self::PredictorOutputSealFailed => "predictor_output_seal_failed",
            Self::KnnFeaturesUnusableHeuristic => "knn_features_unusable_heuristic",
            Self::AnimatedRoutedToScenarioDb => "animated_routed_to_scenario_db",
            Self::AnimatedNoPath => "animated_no_path",
        }
    }

    /// Successful predictor branches that may write inference logs under tightened defaults.
    const fn inference_log_on_success_path(self) -> bool {
        matches!(self, Self::LightGbm)
    }

    /// Heuristic / immature paths must not write BPP scores into `knn_score` (non-KNN telemetry).
    const fn is_heuristic_only_branch(self) -> bool {
        matches!(
            self,
            Self::DbDisabledHeuristic
                | Self::DbUnavailableHeuristic
                | Self::CorpusImmatureHeuristic
                | Self::FeatureExtractionFailedHeuristic
                | Self::KnnNoNeighborsHeuristic
                | Self::KnnQueryFailedHeuristic
                | Self::LightGbmUnavailableAbort
                | Self::ForceKnnEnvRefused
                | Self::KnnFeaturesUnusableHeuristic
                | Self::PredictorOutputSealFailed
        )
    }
}

/// Last gate: finite unit-interval score/confidence before exposing to callers.
pub(crate) fn deliver_quality_prediction(
    prediction: ImageQualityPrediction,
) -> Option<QualityScore> {
    let (score, confidence) =
        crate::algorithm_seal::quality_probability_pair(prediction.score, prediction.confidence)?;
    Some(
        QualityScore {
            score,
            confidence,
            predictor_family: prediction.predictor_family.as_str().to_string(),
            fallback_reason: prediction.fallback_reason,
            knn_neighbor_count: prediction.knn_neighbor_count,
        }
        .sealed(),
    )
}

/// Heuristic-only score for unit tests (production scenario lookup refuses heuristics).
#[cfg(test)]
pub(crate) fn sealed_heuristic_quality_score(
    score: f64,
    reason: impl Into<String>,
) -> Option<QualityScore> {
    let score = crate::algorithm_seal::quality_unit_probability(score)?;
    // Heuristic-only: confidence mirrors sealed score rank (never hard-coded 0.0 / 0.5).
    let confidence = score;
    deliver_quality_prediction(ImageQualityPrediction {
        score,
        confidence,
        predictor_family: ImageQualityPredictorFamily::HeuristicOnly,
        fallback_reason: Some(reason.into()),
        knn_score: None,
        knn_confidence: None,
        knn_neighbor_count: None,
        bpp_fallback_score: Some(score),
        heuristic_score: Some(score),
        regression_score: Some(score),
    })
}

/// Seal primary prediction; on seal failure return `None` (no bpp heuristic downgrade).
fn deliver_with_heuristic_fallback(
    _analysis: &ImageAnalysis,
    prediction: ImageQualityPrediction,
) -> Option<QualityScore> {
    if let Some(quality) = deliver_quality_prediction(prediction) {
        return Some(quality);
    }
    log_static_quality_branch(StaticQualityDbBranch::PredictorOutputSealFailed);
    crate::media_conversion_gate::delivery_db_batch_audit(
        "static_quality_predictor_seal_failed",
        "predictor seal failed; refusing bpp heuristic fallback",
    );
    None
}

fn refuse_static_quality_heuristic(
    audit_branch: &'static str,
    detail: impl AsRef<str>,
) -> Option<QualityScore> {
    crate::media_conversion_gate::delivery_db_batch_audit(audit_branch, detail.as_ref());
    None
}

#[inline]
fn log_static_quality_branch(branch: StaticQualityDbBranch) {
    tracing::debug!(
        target: "mfb.algorithm",
        pipeline = "static_image_quality_db",
        branch = branch.as_str(),
        "static quality resolution"
    );
}

fn static_quality_env_truthy(name: &'static str) -> bool {
    match std::env::var(name) {
        Ok(value) => value == "1" || value.eq_ignore_ascii_case("true"),
        Err(std::env::VarError::NotPresent) => false,
        Err(err) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "static_quality_env_read",
                format!("{name} could not be read: {err}"),
            );
            false
        }
    }
}

// ── Public types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    pub score: f64,
    pub confidence: f64,
    pub predictor_family: String,
    pub fallback_reason: Option<String>,
    /// Neighbors aggregated for KNN/hybrid predictors (`None` for pure heuristic).
    #[serde(default)]
    pub knn_neighbor_count: Option<usize>,
}

/// Blend heuristic Q (0–100) with a sealed DB quality prediction when the pipeline fusion gate is on.
///
/// When fusion is disabled for `pipeline`, returns the sealed heuristic unchanged (no DB blend).
#[must_use]
pub fn fuse_quality_regression_prediction_if_enabled(
    pipeline: &'static str,
    heuristic_quality: Option<u8>,
    quality_prediction: QualityScore,
) -> Option<u8> {
    if !crate::algorithm_runtime::quality_db_fusion_enabled(pipeline) {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline,
            branch = "quality_db_fusion_disabled",
            "skipping quality regression fusion (gate disabled)"
        );
        return heuristic_quality.map(crate::algorithm_seal::seal_u8_quality_display);
    }
    fuse_quality_regression_prediction(heuristic_quality, quality_prediction)
}

/// Blend heuristic Q (0–100) with a sealed DB quality prediction.
///
/// Returns `None` when fusion cannot produce a valid display score.
#[must_use]
pub fn fuse_quality_regression_prediction(
    heuristic_quality: Option<u8>,
    quality_prediction: QualityScore,
) -> Option<u8> {
    let quality_prediction = quality_prediction.sealed();
    if !quality_prediction.is_usable() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "quality_fusion",
            branch = "fused_prediction_poisoned",
            "quality regression fusion rejected poisoned DB prediction"
        );
        return heuristic_quality.map(crate::algorithm_seal::seal_u8_quality_display);
    }
    let predicted_q = quality_prediction.score.mul_add(60.0, 35.0);
    let weight_knn =
        crate::algorithm_seal::quality_unit_probability(quality_prediction.confidence)?
            .clamp(0.0, 0.85);
    let weight_heuristic = 1.0 - weight_knn;

    if let Some(heuristic_q) = heuristic_quality {
        let val = f64::from(heuristic_q).mul_add(weight_heuristic, predicted_q * weight_knn);
        if let Some(blended_q) = crate::numeric_cast::f64_to_u8_strict(val, "blended_quality")
            .map(crate::algorithm_seal::seal_u8_quality_display)
        {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "quality_regression_fusion",
                branch = "heuristic_blended",
                predictor = %quality_prediction.predictor_family,
                heuristic_q,
                predicted_q,
                prob = quality_prediction.score,
                weight_knn,
                blended_q,
                "quality regression fusion applied"
            );
            tracing::info!(
                "{} Quality Regression Fusion [{}]: Heuristic Q={} blended with predicted Q={:.0} (Prob:{:.2}, weight: {:.2}) -> Final Q={}",
                crate::modern_ui::symbols::pick("🔬", "[AUDIT]"),
                quality_prediction.predictor_family,
                heuristic_q,
                predicted_q,
                quality_prediction.score,
                weight_knn,
                blended_q
            );
            Some(blended_q)
        } else {
            None
        }
    } else if weight_knn > 0.3 {
        tracing::info!(
            "{} Quality Regression Fallback [{}]: No heuristic available, using predicted Q={:.0} (Prob:{:.2}, confidence: {:.2})",
            crate::modern_ui::symbols::pick("🔬", "[AUDIT]"),
            quality_prediction.predictor_family,
            predicted_q,
            quality_prediction.score,
            quality_prediction.confidence
        );
        crate::numeric_cast::f64_to_u8_strict(predicted_q, "predicted_quality")
            .map(crate::algorithm_seal::seal_u8_quality_display)
    } else {
        None
    }
}

impl QualityScore {
    /// Re-apply the terminal probability contract (safe after cache/serde round-trips).
    #[must_use]
    pub fn sealed(mut self) -> Self {
        if let Some((score, confidence)) =
            crate::algorithm_seal::quality_probability_pair(self.score, self.confidence)
        {
            self.score = score;
            self.confidence = confidence;
        } else {
            tracing::error!(
                target: "mfb.algorithm",
                pipeline = "quality_score",
                branch = "seal_rejected_poisoned",
                score = self.score,
                confidence = self.confidence,
                "QualityScore failed seal; score/confidence poisoned (no neutral prior substituted)"
            );
            self.score = f64::NAN;
            self.confidence = f64::NAN;
        }
        self
    }

    #[must_use]
    pub const fn is_usable(&self) -> bool {
        self.score.is_finite() && self.confidence.is_finite()
    }
}

#[derive(Debug, Clone)]
pub struct QualityInferenceRecord {
    pub knn_score: Option<f64>,
    pub knn_confidence: Option<f64>,
    pub knn_neighbor_count: Option<usize>,
    pub bpp_fallback_score: Option<f64>,
    pub heuristic_score: Option<f64>,
    pub regression_score: Option<f64>,
    pub predictor_family: String,
    pub final_verdict: String,
    /// Audit-stable branch tag matching `StaticQualityDbBranch` / scenario quality branches.
    pub resolution_branch: String,
}

impl QualityInferenceRecord {
    fn seal_algorithm_outputs(&mut self) {
        self.knn_score = self
            .knn_score
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.knn_confidence = self
            .knn_confidence
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.bpp_fallback_score = self
            .bpp_fallback_score
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.heuristic_score = self
            .heuristic_score
            .and_then(crate::algorithm_seal::quality_unit_probability);
        self.regression_score = self
            .regression_score
            .and_then(crate::algorithm_seal::quality_unit_probability);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageQualityPredictorFamily {
    #[cfg(test)]
    HeuristicOnly,
    #[cfg(test)]
    KnnOnly,
    #[cfg(test)]
    HybridBootstrap,
    LightGbmPython,
}

impl ImageQualityPredictorFamily {
    const fn as_str(self) -> &'static str {
        match self {
            #[cfg(test)]
            Self::HeuristicOnly => "heuristic_only",
            #[cfg(test)]
            Self::KnnOnly => "knn_only",
            #[cfg(test)]
            Self::HybridBootstrap => "hybrid_bootstrap",
            Self::LightGbmPython => "lightgbm_python",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ImageQualityPrediction {
    score: f64,
    confidence: f64,
    predictor_family: ImageQualityPredictorFamily,
    fallback_reason: Option<String>,
    knn_score: Option<f64>,
    knn_confidence: Option<f64>,
    knn_neighbor_count: Option<usize>,
    bpp_fallback_score: Option<f64>,
    heuristic_score: Option<f64>,
    regression_score: Option<f64>,
}

impl ImageQualityPrediction {
    #[cfg(test)]
    fn into_quality_score(self) -> QualityScore {
        QualityScore {
            score: self.score,
            confidence: self.confidence,
            predictor_family: self.predictor_family.as_str().to_string(),
            fallback_reason: self.fallback_reason,
            knn_neighbor_count: self.knn_neighbor_count,
        }
    }

    fn to_record(&self) -> QualityInferenceRecord {
        QualityInferenceRecord {
            knn_score: self.knn_score,
            knn_confidence: self.knn_confidence,
            knn_neighbor_count: self.knn_neighbor_count,
            bpp_fallback_score: self.bpp_fallback_score,
            heuristic_score: self.heuristic_score,
            regression_score: self.regression_score,
            predictor_family: self.predictor_family.as_str().to_string(),
            final_verdict: if self.score >= 0.5 {
                "high".to_string()
            } else {
                "low".to_string()
            },
            resolution_branch: "unknown".to_string(),
        }
    }
}

#[inline]
fn inference_record_with_branch(
    mut record: QualityInferenceRecord,
    branch: StaticQualityDbBranch,
) -> QualityInferenceRecord {
    record.resolution_branch = branch.as_str().to_string();
    record
}

#[inline]
fn log_static_inference(
    conn: &mut Client,
    analysis: &ImageAnalysis,
    path: Option<&Path>,
    record: QualityInferenceRecord,
    branch: StaticQualityDbBranch,
) {
    log_quality_inference_record(
        conn,
        analysis,
        path,
        &inference_record_with_branch(record, branch),
    );
}

/// Deliver a sealed score, then write inference log with the effective audit branch.
fn deliver_log_static_quality(
    conn: &mut Client,
    analysis: &ImageAnalysis,
    path: Option<&Path>,
    prediction: ImageQualityPrediction,
    intended_branch: StaticQualityDbBranch,
    record: QualityInferenceRecord,
) -> Option<QualityScore> {
    let intended_family = prediction.predictor_family.as_str().to_string();
    let quality = deliver_with_heuristic_fallback(analysis, prediction)?;
    let effective_branch =
        if quality.predictor_family == "heuristic_only" && intended_family != "heuristic_only" {
            StaticQualityDbBranch::PredictorOutputSealFailed
        } else {
            intended_branch
        };
    let mut record = record;
    record
        .predictor_family
        .clone_from(&quality.predictor_family);
    if let Some((score, confidence)) =
        crate::algorithm_seal::quality_probability_pair(quality.score, quality.confidence)
    {
        record.regression_score = Some(score);
        if effective_branch.is_heuristic_only_branch() {
            record.heuristic_score = record.heuristic_score.or(Some(score));
            record.bpp_fallback_score = record.bpp_fallback_score.or(Some(score));
        } else {
            record.knn_score = record.knn_score.or(Some(score));
            record.knn_confidence = record.knn_confidence.or(Some(confidence));
        }
    }
    if crate::algorithm_runtime::static_quality_inference_logging_enabled()
        && (effective_branch.inference_log_on_success_path()
            || crate::algorithm_runtime::quality_inference_log_heuristic_fallbacks_enabled())
    {
        log_static_inference(conn, analysis, path, record, effective_branch);
    }
    Some(quality)
}

// ── Schema Initialization ────────────────────────────────────────────────────

/// Initialize the static-image quality schema on top of the multi-scenario DB.
///
/// # Errors
///
/// Returns an error when schema initialization fails or the legacy
/// `quality_label` backfill update cannot be applied.
pub fn init_quality_schema(conn: &mut Client) -> Result<()> {
    crate::multi_scenario_db::init_multi_scenario_schema(conn)?;

    // 🛡️ DATA MIGRATION GUARD: Repair legacy quality_score drift from quality_label.
    // This is a one-time operation during schema transition (V6.0+).
    // Rationale: Ensures existing training data remains visible to KNN regression.
    conn.execute(
        "UPDATE image_quality_samples
         SET quality_score = CASE
            WHEN quality_label IN ('png-high', 'modern-high') THEN 1.0
            WHEN quality_label IN ('png-low', 'modern-low') THEN 0.0
         END
         WHERE quality_label IN ('png-high', 'modern-high', 'png-low', 'modern-low')
           AND quality_score IS DISTINCT FROM CASE
               WHEN quality_label IN ('png-high', 'modern-high') THEN 1.0
               WHEN quality_label IN ('png-low', 'modern-low') THEN 0.0
               ELSE quality_score
           END",
        &[],
    )?;

    crate::ui_stderr::line(
        crate::modern_ui::symbols::SUCCESS,
        crate::modern_ui::symbols::plain::SUCCESS,
        "Static Quality Schema Synchronized (Multi-Scenario).",
    );
    Ok(())
}

// ── Feature Engineering ──────────────────────────────────────────────────────

/// JSONB payload persisted on `image_quality_samples.metadata` after ingest.
///
/// Captures geometry, bitstream/precision hints, perception scalars, and raw
/// EXIF-like key/value pairs from analysis — without dumping the 256D embedding.
///
/// # Errors
///
/// Returns an error when `training_label` is present but fails
/// [`crate::training_tier_audit::verify_training_tier_for_ingest`].
pub fn build_image_quality_ingest_metadata(
    analysis: &ImageAnalysis,
    training_label: Option<&str>,
    path: &Path,
) -> anyhow::Result<Value> {
    if let Some(label) = training_label {
        crate::training_tier_audit::verify_training_tier_for_ingest(analysis, label, path)?;
    }
    let aspect_ratio = if analysis.height > 0 {
        f64::from(analysis.width) / f64::from(analysis.height)
    } else {
        1.0
    };

    let mut precision = Map::new();
    precision.insert(
        "bit_depth".into(),
        match analysis.precision.bit_depth.or(analysis.color_depth) {
            None => Value::Null,
            Some(v) => Value::from(v),
        },
    );
    if let Some(palette_size) = analysis.precision.palette_size {
        precision.insert("palette_size".into(), Value::from(palette_size));
    }
    if let Some(color_type) = analysis.precision.color_type {
        precision.insert("color_type".into(), Value::from(color_type));
    }
    precision.insert(
        "is_lossless_deterministic".into(),
        Value::from(analysis.precision.is_lossless_deterministic),
    );
    if let Some(quality_estimate) = analysis.precision.quality_estimate {
        precision.insert("quality_estimate".into(), Value::from(quality_estimate));
    }
    if let Some(chroma) = analysis.precision.chroma_subsampling.clone() {
        precision.insert("chroma_subsampling".into(), Value::from(chroma));
    }

    let mut perception = Map::new();
    perception.insert(
        "average_luma".into(),
        crate::media_conversion_gate::json_inference_optional_f64_or_null(Some(
            analysis.perception.average_luma,
        )),
    );
    perception.insert(
        "peak_luma".into(),
        crate::media_conversion_gate::json_inference_optional_f64_or_null(Some(
            analysis.perception.peak_luma,
        )),
    );
    perception.insert(
        "gray_center_of_mass".into(),
        Value::Array(vec![
            crate::media_conversion_gate::json_inference_optional_f64_or_null(Some(
                analysis.perception.gray_center_of_mass.0,
            )),
            crate::media_conversion_gate::json_inference_optional_f64_or_null(Some(
                analysis.perception.gray_center_of_mass.1,
            )),
        ]),
    );

    let mut signals = Map::new();
    signals.insert(
        "entropy".into(),
        crate::media_conversion_gate::json_inference_optional_f64_or_null(
            analysis.features.entropy,
        ),
    );
    signals.insert(
        "compression_ratio".into(),
        crate::media_conversion_gate::json_inference_optional_f64_or_null(
            analysis.features.compression_ratio,
        ),
    );
    signals.insert(
        "psnr".into(),
        crate::media_conversion_gate::json_inference_optional_f64_or_null(analysis.psnr),
    );
    signals.insert(
        "ssim".into(),
        crate::media_conversion_gate::json_inference_optional_f64_or_null(analysis.ssim),
    );

    let mut media = Map::new();
    media.insert("has_alpha".into(), Value::from(analysis.has_alpha));
    media.insert("is_animated".into(), Value::from(analysis.is_animated));
    media.insert(
        "color_depth".into(),
        match analysis.color_depth {
            None => Value::Null,
            Some(v) => Value::from(v),
        },
    );
    if let Some(color_space) = analysis.color_space.clone() {
        media.insert("color_space".into(), Value::from(color_space));
    }
    media.insert("is_lossless".into(), Value::from(analysis.is_lossless));
    if let Some(duration_secs) = analysis.duration_secs {
        media.insert(
            "duration_secs".into(),
            crate::media_conversion_gate::json_inference_optional_f64_or_null(Some(f64::from(
                duration_secs,
            ))),
        );
    }

    match crate::conversion::media_info_without_ffprobe(path) {
        Ok(Some(info)) => {
            let mut bitstream = Map::new();
            bitstream.insert("width".into(), Value::from(info.width));
            bitstream.insert("height".into(), Value::from(info.height));
            if let Some(channel_type) = info.channel_type {
                bitstream.insert("channel_type".into(), Value::from(channel_type.clone()));
                media.insert("channel_type".into(), Value::from(channel_type));
            }
            if let Some(bit_depth) = info.bit_depth {
                bitstream.insert("bit_depth".into(), Value::from(bit_depth));
            }
            media.insert("bitstream".into(), Value::Object(bitstream));
        }
        Ok(None) => {}
        Err(err) => {
            crate::media_conversion_gate::probe_layer_audit(
                "quality_db_bitstream_media_probe_failed",
                path,
                format!("bitstream metadata projection failed: {err}"),
            );
        }
    }

    let source_metadata: Map<String, Value> = analysis
        .metadata
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();

    let mut root = Map::new();
    root.insert(
        "scenario_semantics".into(),
        Value::String("image_quality".into()),
    );
    root.insert(
        "storage_table".into(),
        Value::String("image_quality_samples".into()),
    );
    if let Some(label) = training_label.filter(|s| !s.trim().is_empty()) {
        root.insert("training_label".into(), Value::String(label.to_string()));
    }
    root.insert(
        "geometry".into(),
        Value::Object(Map::from_iter([
            ("width".into(), Value::from(analysis.width)),
            ("height".into(), Value::from(analysis.height)),
            ("file_size_bytes".into(), Value::from(analysis.file_size)),
            ("format".into(), Value::String(analysis.format.clone())),
            (
                "aspect_ratio".into(),
                crate::media_conversion_gate::json_inference_optional_f64_or_null(Some(
                    aspect_ratio,
                )),
            ),
        ])),
    );
    root.insert("media".into(), Value::Object(media));
    root.insert("precision".into(), Value::Object(precision));
    root.insert("signals".into(), Value::Object(signals));
    root.insert("perception".into(), Value::Object(perception));
    if !source_metadata.is_empty() {
        root.insert("source_metadata".into(), Value::Object(source_metadata));
    }
    if let Some(jpeg) = analysis.jpeg_analysis.as_ref() {
        root.insert(
            "jpeg_analysis".into(),
            Value::Object(Map::from_iter([
                (
                    "estimated_quality".into(),
                    Value::from(jpeg.estimated_quality),
                ),
                (
                    "confidence".into(),
                    crate::media_conversion_gate::json_inference_optional_f64_or_null(Some(
                        jpeg.confidence,
                    )),
                ),
                (
                    "is_standard_table".into(),
                    Value::from(jpeg.is_standard_table),
                ),
                (
                    "is_high_quality_original".into(),
                    Value::from(jpeg.is_high_quality_original),
                ),
                (
                    "encoder_hint".into(),
                    match jpeg.encoder_hint.as_ref() {
                        None => Value::Null,
                        Some(h) => Value::String(h.clone()),
                    },
                ),
            ])),
        );
    }
    if let Some(err) = analysis.analysis_error.as_ref() {
        root.insert("analysis_error".into(), Value::String(err.clone()));
    }

    let probe = crate::training_tier_audit::probe_from_analysis(analysis)?;
    root.insert(
        "training_tier_audit".into(),
        crate::training_tier_audit::build_training_tier_audit_value(&probe, training_label),
    );

    Ok(Value::Object(root))
}

/// JPEG sidecar slots for the 256D embedding; absent JPEG analysis → DB-safe
/// missing-measurement sentinels.
fn jpeg_sidecar_embedding_slots(
    jpeg: Option<&crate::image_jpeg_analysis::JpegQualityAnalysis>,
) -> (f32, f32, f32, f32) {
    let Some(jpeg) = jpeg else {
        // Quality/confidence are absent measurements; table/HQ flags are booleans.
        return (
            QUALITY_EMBED_MISSING_MEASUREMENT,
            QUALITY_EMBED_MISSING_MEASUREMENT,
            0.0_f32,
            0.0_f32,
        );
    };
    (
        unit_interval_f32(f64_to_f32_feature(
            f64::from(jpeg.estimated_quality) / 100.0,
        )),
        unit_interval_f32(f64_to_f32_feature(jpeg.confidence)),
        if jpeg.is_standard_table {
            1.0_f32
        } else {
            0.0_f32
        },
        if jpeg.is_high_quality_original {
            1.0_f32
        } else {
            0.0_f32
        },
    )
}

fn parse_metadata_dpi_value(key: &str, value: &str) -> anyhow::Result<f64> {
    let Some(raw_part) = value.split('/').next() else {
        anyhow::bail!("Metadata DPI field {key} is empty");
    };
    let trimmed = raw_part.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Metadata DPI field {key} is empty");
    }
    let parsed = trimmed
        .parse::<f64>()
        .map_err(|err| anyhow::anyhow!("Metadata DPI field {key} is malformed: {err}"))?;
    if !parsed.is_finite() || parsed <= 0.0_f64 {
        anyhow::bail!("Metadata DPI field {key} is not positive finite: {parsed}");
    }
    Ok(parsed)
}

/// Build the 256D static-image quality embedding expected by the KNN tables.
///
/// # Errors
///
/// Returns an error when required scalar features are missing or non-finite, or
/// when the mandatory `physics_225` tail is absent, malformed, or degenerate.
pub fn get_quality_features(analysis: &ImageAnalysis) -> Result<pgvector::Vector> {
    if analysis.width == 0 || analysis.height == 0 {
        anyhow::bail!(
            "Invalid image dimensions for quality embedding: {}x{}",
            analysis.width,
            analysis.height
        );
    }
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = crate::numeric_cast::u64_to_f64(analysis.file_size) / total_pixels;
    let aspect_ratio = f64::from(analysis.width) / f64::from(analysis.height);

    // 🛡️ ZERO FORGERY: If mandatory features are missing, extraction fails.
    let entropy = analysis
        .features
        .entropy
        .ok_or_else(|| anyhow::anyhow!("Mandatory feature missing: entropy"))?;
    if !entropy.is_finite() {
        anyhow::bail!("Mandatory feature invalid: entropy is not finite");
    }
    let comp_ratio = analysis
        .features
        .compression_ratio
        .ok_or_else(|| anyhow::anyhow!("Mandatory feature missing: compression_ratio"))?;
    if !comp_ratio.is_finite() {
        anyhow::bail!("Mandatory feature invalid: compression_ratio is not finite");
    }
    if !spatial_bpp.is_finite() || spatial_bpp < 0.0 {
        anyhow::bail!("Mandatory feature invalid: spatial_bpp is not finite");
    }

    let mut dpi_x = 72.0_f64;
    let mut dpi_y = 72.0_f64;
    for (key, value) in &analysis.metadata {
        let key_lower = key.to_lowercase();
        if key_lower.contains("xresolution")
            || key_lower.contains("x-resolution")
            || key_lower.contains("x_resolution")
            || key_lower == "dpix"
        {
            dpi_x = parse_metadata_dpi_value(key, value)?;
        } else if key_lower.contains("yresolution")
            || key_lower.contains("y-resolution")
            || key_lower.contains("y_resolution")
            || key_lower == "dpiy"
        {
            dpi_y = parse_metadata_dpi_value(key, value)?;
        } else if key_lower == "dpi" || key_lower == "resolution" || key_lower == "print_dpi" {
            let parsed = parse_metadata_dpi_value(key, value)?;
            dpi_x = parsed;
            dpi_y = parsed;
        }
    }

    let fmt_lower = analysis.format.to_lowercase();
    let is_png = if fmt_lower.contains("png") {
        1.0_f32
    } else {
        0.0_f32
    };
    let is_jpeg = if fmt_lower.contains("jpeg") || fmt_lower.contains("jpg") {
        1.0_f32
    } else {
        0.0_f32
    };
    let is_webp = if fmt_lower.contains("webp") {
        1.0_f32
    } else {
        0.0_f32
    };
    let is_gif = if fmt_lower.contains("gif") {
        1.0_f32
    } else {
        0.0_f32
    };
    let is_tiff = if fmt_lower.contains("tiff") || fmt_lower.contains("tif") {
        1.0_f32
    } else {
        0.0_f32
    };
    let is_avif = if fmt_lower.contains("avif") {
        1.0_f32
    } else {
        0.0_f32
    };
    let is_heic = if fmt_lower.contains("heic") || fmt_lower.contains("heif") {
        1.0_f32
    } else {
        0.0_f32
    };
    let is_jxl = if fmt_lower.contains("jxl") || fmt_lower.contains("jpeg-xl") {
        1.0_f32
    } else {
        0.0_f32
    };

    let (jpeg_quality, jpeg_confidence, jpeg_standard_flag, jpeg_high_quality_flag) =
        jpeg_sidecar_embedding_slots(analysis.jpeg_analysis.as_ref());

    let luma_avg = analysis.perception.average_luma;
    let luma_peak = analysis.perception.peak_luma;
    let gcom_x = analysis.perception.gray_center_of_mass.0;
    let gcom_y = analysis.perception.gray_center_of_mass.1;

    let mut vec = vec![0.0_f32; 256];

    // [0-30] Base Quality Stats
    vec[0] = unit_interval_f32(f64_to_f32_feature(entropy) / 8.0);
    vec[1] = unit_interval_f32(nonnegative_f32(f64_to_f32_feature(comp_ratio)).ln_1p() / 3.0);
    vec[2] = unit_interval_f32(nonnegative_f32(f64_to_f32_feature(spatial_bpp)).ln_1p() / 3.5);
    vec[3] = unit_interval_f32(f64_to_f32_feature(total_pixels.max(1.0).log10()) / 10.0);
    vec[4] = unit_interval_f32(nonnegative_f32(f64_to_f32_feature(aspect_ratio)).ln_1p() / 2.5);
    vec[5] = if analysis.is_lossless { 1.0 } else { 0.0 };
    vec[6] = unit_interval_f32(nonnegative_f32(
        f64_to_f32_feature(dpi_x.clamp(1.0, 1200.0)) / 300.0,
    ));
    vec[7] = unit_interval_f32(nonnegative_f32(
        f64_to_f32_feature(dpi_y.clamp(1.0, 1200.0)) / 300.0,
    ));
    let width_for_log = u32::max(analysis.width, 1);
    let height_for_log = u32::max(analysis.height, 1);
    vec[8] = unit_interval_f32(nonnegative_f32(
        f64_to_f32_feature(f64::from(width_for_log).log10()) / 5.0,
    ));
    vec[9] = unit_interval_f32(nonnegative_f32(
        f64_to_f32_feature(f64::from(height_for_log).log10()) / 5.0,
    ));
    vec[10] = unit_interval_f32(nonnegative_f32(
        f64_to_f32_feature(crate::numeric_cast::u64_to_f64(analysis.file_size).log10()) / 8.0,
    ));
    vec[11] = if analysis.has_alpha { 1.0 } else { 0.0 };
    vec[QUALITY_EMBED_COLOR_DEPTH_SLOT] = encode_optional_bit_depth_feature(analysis.color_depth);
    vec[13] = perception_unit_interval(luma_avg, "average_luma")?;
    vec[14] = perception_unit_interval(luma_peak, "peak_luma")?;
    vec[15] = perception_unit_interval(gcom_x, "gray_center_of_mass_x")?;
    vec[16] = perception_unit_interval(gcom_y, "gray_center_of_mass_y")?;
    vec[17] = quality_embed_measured_dimension_f32(analysis.psnr, 100.0);
    vec[18] = quality_embed_measured_dimension_f32(analysis.ssim, 1.0);
    vec[QUALITY_EMBED_JPEG_QUALITY_SLOT] = unit_interval_or_missing_measurement(jpeg_quality);
    vec[QUALITY_EMBED_JPEG_CONFIDENCE_SLOT] = unit_interval_or_missing_measurement(jpeg_confidence);
    vec[21] = jpeg_standard_flag;
    vec[22] = is_png;
    vec[23] = is_jpeg;
    vec[24] = is_webp;
    vec[25] = is_gif;
    vec[26] = is_tiff;
    vec[27] = is_avif;
    vec[28] = is_heic;
    vec[29] = is_jxl;
    vec[30] = jpeg_high_quality_flag;

    let physics = analysis
        .physics_225
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Mandatory feature missing: physics_225"))?;
    if physics.len() != 225 {
        anyhow::bail!(
            "Mandatory feature invalid: physics_225 length {} != 225",
            physics.len()
        );
    }
    if physics.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("Mandatory feature invalid: physics_225 contains non-finite values");
    }
    if physics.iter().all(|value| value.abs() <= f32::EPSILON) {
        anyhow::bail!("Mandatory feature invalid: physics_225 is all zero");
    }

    // [31-255] Physics Mapping (225D block)
    crate::real_physics::encode_normalized_physics_225(&mut vec, 31, physics);

    normalize_quality_embedding_for_pgvector_storage(&mut vec);
    assert_quality_embedding_finite_policy(&vec)?;

    Ok(pgvector::Vector::from(vec))
}

const fn unit_interval_f32(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        f32::NAN
    }
}

const fn nonnegative_f32(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        f32::NAN
    }
}

const fn unit_interval_or_missing_measurement(value: f32) -> f32 {
    if value == QUALITY_EMBED_MISSING_MEASUREMENT {
        QUALITY_EMBED_MISSING_MEASUREMENT
    } else {
        unit_interval_f32(value)
    }
}

fn encode_optional_bit_depth_feature(bit_depth: Option<u8>) -> f32 {
    quality_embed_measured_dimension_f32(bit_depth.map(f64::from), 16.0)
}

/// Optional-measurement slot indices in the 256D image-quality embedding (M225/M246).
///
/// All five slots may carry `QUALITY_EMBED_MISSING_MEASUREMENT` (-1.0) when the
/// corresponding measurement was unavailable — pgvector-safe finite sentinel.
pub const QUALITY_EMBED_COLOR_DEPTH_SLOT: usize = 12;
pub const QUALITY_EMBED_PSNR_SLOT: usize = 17;
pub const QUALITY_EMBED_SSIM_SLOT: usize = 18;
pub const QUALITY_EMBED_JPEG_QUALITY_SLOT: usize = 19;
pub const QUALITY_EMBED_JPEG_CONFIDENCE_SLOT: usize = 20;

/// Finite sentinel used for absent optional measurements in pgvector storage.
///
/// pgvector rejects `NaN` values at insert time, so DB/KNN embeddings must remain
/// finite. `LightGBM` payload construction still maps these sentinel slots to
/// JSON null / NaN missing values.
pub const QUALITY_EMBED_MISSING_MEASUREMENT: f32 = -1.0;

/// Rewrite stale DB sentinel `0.0` on measurement slots to the pgvector-safe
/// missing-measurement sentinel before KNN / `LightGBM` (M235/M246).
pub fn sanitize_stale_quality_measurement_embed_slots(vec: &mut [f32]) {
    for index in [QUALITY_EMBED_PSNR_SLOT, QUALITY_EMBED_SSIM_SLOT] {
        let Some(slot) = vec.get_mut(index) else {
            continue;
        };
        if crate::float_compare::approx_zero_f32(*slot) {
            *slot = QUALITY_EMBED_MISSING_MEASUREMENT;
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "quality_regression_stale_embed",
                format!(
                    "embedding slot {index} had stale 0.0 sentinel; rewritten to pgvector-safe missing sentinel"
                ),
            );
        }
    }
}

fn normalize_quality_embedding_for_pgvector_storage(vec: &mut [f32]) {
    for index in [
        QUALITY_EMBED_COLOR_DEPTH_SLOT,
        QUALITY_EMBED_PSNR_SLOT,
        QUALITY_EMBED_SSIM_SLOT,
        QUALITY_EMBED_JPEG_QUALITY_SLOT,
        QUALITY_EMBED_JPEG_CONFIDENCE_SLOT,
    ] {
        let Some(slot) = vec.get_mut(index) else {
            continue;
        };
        if slot.is_nan() {
            *slot = QUALITY_EMBED_MISSING_MEASUREMENT;
        }
    }
}

fn sanitized_quality_embedding_for_use(embedding: &pgvector::Vector) -> pgvector::Vector {
    let mut vec = embedding.to_vec();
    sanitize_stale_quality_measurement_embed_slots(&mut vec);
    pgvector::Vector::from(vec)
}

/// Embed slots that may carry the DB-safe missing-measurement sentinel when unmeasured.
#[must_use]
pub const fn quality_embedding_slot_allows_non_finite(index: usize) -> bool {
    const _: () = assert!(QUALITY_EMBED_COLOR_DEPTH_SLOT == 12);
    const _: () = assert!(QUALITY_EMBED_PSNR_SLOT == 17);
    const _: () = assert!(QUALITY_EMBED_SSIM_SLOT == 18);
    const _: () = assert!(QUALITY_EMBED_JPEG_QUALITY_SLOT == 19);
    const _: () = assert!(QUALITY_EMBED_JPEG_CONFIDENCE_SLOT == 20);
    matches!(index, 12 | 17 | 18 | 19 | 20)
}

/// SSOT for ingest + DB validation: pgvector storage must be finite.
///
/// # Errors
/// Returns an error when a required slot is non-finite.
pub fn assert_quality_embedding_finite_policy(slice: &[f32]) -> Result<()> {
    if let Some((idx, value)) = slice.iter().enumerate().find_map(|(index, &v)| {
        if v.is_finite() {
            None
        } else {
            Some((index, v))
        }
    }) {
        anyhow::bail!(
            "image_quality embedding slot {idx} is non-finite ({value}); pgvector storage requires finite values"
        );
    }
    Ok(())
}

fn perception_unit_interval(value: f64, name: &str) -> Result<f32> {
    if !value.is_finite() {
        anyhow::bail!("Mandatory perception feature {name} is not finite");
    }
    Ok(unit_interval_f32(f64_to_f32_feature(value)))
}

/// Measured scalar → unit interval; absent/non-finite → pgvector-safe missing sentinel.
fn quality_embed_measured_dimension_f32(value: Option<f64>, scale: f64) -> f32 {
    match value.filter(|v| v.is_finite()) {
        Some(v) if scale.is_finite() && scale > 0.0 => {
            unit_interval_f32(f64_to_f32_feature(v / scale))
        }
        _ => QUALITY_EMBED_MISSING_MEASUREMENT,
    }
}

const fn f64_to_f32_feature(value: f64) -> f32 {
    crate::numeric_cast::f64_to_f32_lossy(value)
}

// ── Heuristics ───────────────────────────────────────────────────────────────

pub(crate) fn bpp_heuristic_score(analysis: &ImageAnalysis) -> Result<f64> {
    let total_pixels = f64::from(analysis.width) * f64::from(analysis.height);
    let spatial_bpp = crate::numeric_cast::u64_to_f64(analysis.file_size) / total_pixels.max(1.0);
    let entropy = analysis
        .features
        .entropy
        .ok_or_else(|| anyhow::anyhow!("Entropy unmeasurable; heuristic aborted."))?;
    if !entropy.is_finite() {
        anyhow::bail!("Entropy invalid; heuristic aborted.");
    }
    let entropy_score = (entropy / 8.0).clamp(0.0, 1.0);
    let bpp_score = (1.0 - (spatial_bpp / 20.0).clamp(0.0, 1.0)).max(0.0);
    let lossless_bonus = if analysis.is_lossless { 0.1 } else { 0.0 };
    let raw = (entropy_score * 0.5 + bpp_score * 0.5 + lossless_bonus).clamp(0.0, 1.0);
    crate::algorithm_seal::quality_unit_probability(raw)
        .ok_or_else(|| anyhow::anyhow!("bpp heuristic score failed quality seal (non-finite)"))
}

#[cfg(test)]
fn bpp_heuristic_quality(
    analysis: &ImageAnalysis,
    reason: impl Into<String>,
) -> Result<ImageQualityPrediction> {
    let heuristic_score = bpp_heuristic_score(analysis)?;
    Ok(ImageQualityPrediction {
        score: heuristic_score,
        // Heuristic fallbacks must not claim high confidence proportional to score.
        // Confidence here is a conservative baseline; higher confidence requires measured/KNN evidence.
        confidence: crate::constants::HEURISTIC_SAFETY_FLOOR,
        predictor_family: ImageQualityPredictorFamily::HeuristicOnly,
        fallback_reason: Some(reason.into()),
        knn_score: None,
        knn_confidence: None,
        knn_neighbor_count: None,
        bpp_fallback_score: Some(heuristic_score),
        heuristic_score: Some(heuristic_score),
        regression_score: Some(heuristic_score),
    })
}

pub(crate) const IMAGE_QUALITY_KNN_K: usize = 5;
pub(crate) const IMAGE_QUALITY_KNN_THRESHOLD: f64 = 2.0;

pub(crate) fn query_quality_knn_features(
    conn: &mut Client,
    scenario: crate::scenario::ScenarioType,
    embedding: &pgvector::Vector,
) -> Result<Option<crate::multi_scenario_db::KnnRegressionFeatures>> {
    let query = crate::multi_scenario_db::ScenarioQuery::new(scenario)
        .with_k(IMAGE_QUALITY_KNN_K)
        .with_threshold(IMAGE_QUALITY_KNN_THRESHOLD);
    let embedding = if scenario.is_quality_regression() {
        sanitized_quality_embedding_for_use(embedding)
    } else {
        embedding.clone()
    };
    crate::multi_scenario_db::knn_regression_lookup(conn, &query, &embedding)
}

fn query_image_quality_knn_features(
    conn: &mut Client,
    embedding: &pgvector::Vector,
) -> Result<Option<crate::multi_scenario_db::KnnRegressionFeatures>> {
    query_quality_knn_features(conn, crate::scenario::ScenarioType::ImageQuality, embedding)
}

/// Test-only: production must not emit KNN-only decision scores (B06 no-fabrication rule).
#[cfg(test)]
pub(crate) fn knn_only_prediction(
    knn: &crate::multi_scenario_db::KnnRegressionFeatures,
    heuristic_score: Option<f64>,
    fallback_reason: Option<String>,
) -> Option<ImageQualityPrediction> {
    let neighbor_coverage = (crate::numeric_cast::usize_to_f64(knn.neighbor_count)
        / crate::numeric_cast::usize_to_f64(IMAGE_QUALITY_KNN_K))
    .clamp(0.0, 1.0);
    let score = crate::algorithm_seal::quality_unit_probability(knn.dist_weighted_score)?;
    let knn_conf = crate::algorithm_seal::quality_unit_probability(knn.confidence)?;
    let confidence = crate::algorithm_seal::quality_unit_probability(
        (knn_conf * neighbor_coverage).clamp(0.0, 1.0),
    )?;
    Some(ImageQualityPrediction {
        score,
        confidence,
        predictor_family: ImageQualityPredictorFamily::KnnOnly,
        fallback_reason,
        knn_score: Some(score),
        knn_confidence: Some(knn_conf),
        knn_neighbor_count: Some(knn.neighbor_count),
        bpp_fallback_score: None,
        heuristic_score,
        regression_score: Some(score),
    })
}

/// Pull `score` toward KNN when model/heuristic and neighbors disagree strongly.
#[must_use]
pub(crate) fn apply_knn_disagreement_guard(
    score: f64,
    knn: &crate::multi_scenario_db::KnnRegressionFeatures,
    pipeline: &'static str,
    audit_branch: &'static str,
) -> Option<f64> {
    if !crate::algorithm_runtime::quality_knn_disagreement_guard_enabled(pipeline) {
        return crate::algorithm_seal::quality_unit_probability(score);
    }
    let knn = knn.clone().seal_aggregates()?;
    let knn_score = crate::algorithm_seal::quality_unit_probability(knn.dist_weighted_score)?;
    let mut score = crate::algorithm_seal::quality_unit_probability(score)?;
    let neighbor_coverage = (crate::numeric_cast::usize_to_f64(knn.neighbor_count)
        / crate::numeric_cast::usize_to_f64(IMAGE_QUALITY_KNN_K))
    .clamp(0.0, 1.0);
    let disagree = (score - knn_score).abs();
    if disagree > crate::constants::QUALITY_LGBM_KNN_DISAGREE_THRESHOLD
        && knn.confidence >= crate::constants::QUALITY_LGBM_KNN_GUARD_MIN_CONFIDENCE
        && neighbor_coverage >= crate::constants::QUALITY_LGBM_KNN_GUARD_MIN_COVERAGE
    {
        let knn_sf = knn.confidence.clamp(0.0, 1.0);
        let denom = (1.0_f64 - crate::constants::QUALITY_LGBM_KNN_DISAGREE_THRESHOLD).max(1e-6);
        let excess = (disagree - crate::constants::QUALITY_LGBM_KNN_DISAGREE_THRESHOLD) / denom;
        let pull = (excess * crate::constants::QUALITY_LGBM_KNN_DISAGREE_BLEND_CAP * knn_sf)
            .clamp(0.0, crate::constants::QUALITY_LGBM_KNN_DISAGREE_BLEND_CAP);
        let score_before = score;
        score = score.mul_add(1.0 - pull, knn_score * pull).clamp(0.0, 1.0);
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline,
            branch = audit_branch,
            model_score = score_before,
            knn_score,
            disagree,
            pull,
            fused_score = score,
            neighbor_coverage,
            "quality score pulled toward KNN after disagreement guard"
        );
    }
    crate::algorithm_seal::quality_unit_probability(score)
}

/// Test-only: production must not emit hybrid-bootstrap decision scores (B06 no-fabrication rule).
#[cfg(test)]
pub(crate) fn hybrid_bootstrap_prediction(
    heuristic_score: f64,
    knn: &crate::multi_scenario_db::KnnRegressionFeatures,
    fallback_reason: Option<String>,
    pipeline: &'static str,
) -> Option<ImageQualityPrediction> {
    let knn = knn.clone().seal_aggregates()?;
    if !heuristic_score.is_finite() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline,
            branch = "hybrid_bootstrap_non_finite_heuristic",
            "hybrid_bootstrap: non-finite heuristic_score rejected"
        );
        return None;
    }
    let heuristic_score = crate::algorithm_seal::quality_unit_probability(heuristic_score)?;
    let knn_score = crate::algorithm_seal::quality_unit_probability(knn.dist_weighted_score)?;
    let neighbor_coverage = (crate::numeric_cast::usize_to_f64(knn.neighbor_count)
        / crate::numeric_cast::usize_to_f64(IMAGE_QUALITY_KNN_K))
    .clamp(0.0, 1.0);
    let locality = (1.0 / (1.0 + knn.dist_to_nearest.max(0.0))).clamp(0.0, 1.0);
    let stability = (1.0 - knn.knn_score_std_k5.clamp(0.0, 1.0)).clamp(0.0, 1.0);
    let agreement = (1.0 - (heuristic_score - knn_score).abs()).clamp(0.0, 1.0);
    let knn_weight = 0.2_f64
        .mul_add(
            neighbor_coverage,
            0.25_f64.mul_add(locality, 0.4_f64.mul_add(knn.confidence, 0.15)),
        )
        .clamp(0.15, 0.75);
    let mut score = heuristic_score.mul_add(1.0 - knn_weight, knn_score * knn_weight);
    score = apply_knn_disagreement_guard(score, &knn, pipeline, "hybrid_knn_disagreement_fusion")?;
    let confidence = crate::algorithm_seal::quality_unit_probability(0.2_f64.mul_add(
        neighbor_coverage,
        0.3_f64.mul_add(stability, 0.3_f64.mul_add(agreement, 0.2)),
    ))?;
    let knn_confidence = crate::algorithm_seal::quality_unit_probability(knn.confidence)?;

    Some(ImageQualityPrediction {
        score,
        confidence,
        predictor_family: ImageQualityPredictorFamily::HybridBootstrap,
        fallback_reason,
        knn_score: Some(knn_score),
        knn_confidence: Some(knn_confidence),
        knn_neighbor_count: Some(knn.neighbor_count),
        bpp_fallback_score: None,
        heuristic_score: Some(heuristic_score),
        regression_score: Some(score),
    })
}

fn lightgbm_python_prediction(
    prediction: &crate::quality_regression_model::ModelQualityPrediction,
    knn: &crate::multi_scenario_db::KnnRegressionFeatures,
    heuristic_score: Option<f64>,
) -> Option<ImageQualityPrediction> {
    let knn_score = crate::algorithm_seal::quality_unit_probability(knn.dist_weighted_score)?;
    let score = apply_knn_disagreement_guard(
        prediction.score,
        knn,
        "static_image_quality_lgbm",
        "knn_disagreement_fusion",
    )?;
    let neighbor_coverage = (crate::numeric_cast::usize_to_f64(knn.neighbor_count)
        / crate::numeric_cast::usize_to_f64(IMAGE_QUALITY_KNN_K))
    .clamp(0.0, 1.0);
    let cov_damp = neighbor_coverage
        .clamp(0.0, 1.0)
        .powf(crate::constants::QUALITY_MODEL_CONFIDENCE_COVERAGE_EXP);
    let confidence = crate::algorithm_seal::quality_unit_probability(
        (prediction.confidence.clamp(0.0, 1.0) * cov_damp).clamp(0.0, 1.0),
    )?;
    let knn_confidence = crate::algorithm_seal::quality_unit_probability(knn.confidence)?;

    Some(ImageQualityPrediction {
        score,
        confidence,
        predictor_family: ImageQualityPredictorFamily::LightGbmPython,
        fallback_reason: None,
        knn_score: Some(knn_score),
        knn_confidence: Some(knn_confidence),
        knn_neighbor_count: Some(knn.neighbor_count),
        bpp_fallback_score: None,
        heuristic_score,
        regression_score: Some(score),
    })
}

// ── Database Operations ──────────────────────────────────────────────────────

pub fn get_class_counts(conn: &mut Client) -> (i64, i64) {
    let Ok(rows) = conn.query(
        "SELECT
            CASE WHEN quality_label IN ('png-high', 'modern-high') THEN 'high'
                 WHEN quality_label IN ('png-low', 'modern-low') THEN 'low'
                 ELSE 'other'
            END as class,
            count(*)
         FROM image_quality_samples
         WHERE quality_label IN ('png-high', 'modern-high', 'png-low', 'modern-low')
         GROUP BY 1",
        &[],
    ) else {
        return (0, 0);
    };

    let mut high = 0;
    let mut low = 0;
    for row in rows {
        let class: String = row.get(0);
        let count: i64 = row.get(1);
        if class == "high" {
            high = count;
        } else if class == "low" {
            low = count;
        }
    }
    (high, low)
}

pub fn check_quality_db_maturity(conn: &mut Client) -> bool {
    let (h, l) = get_class_counts(conn);
    crate::algorithm_runtime::quality_corpus_is_mature(h, l)
}

#[must_use]
pub fn lookup_image_quality(analysis: &ImageAnalysis) -> Option<QualityScore> {
    let path = (!analysis.file_path.is_empty()).then(|| Path::new(analysis.file_path.as_str()));
    lookup_image_quality_with_path(analysis, path)
}

/// Static-image quality lookup, or animated lookup when `path` is provided.
#[must_use]
pub fn lookup_image_quality_with_path(
    analysis: &ImageAnalysis,
    path: Option<&Path>,
) -> Option<QualityScore> {
    if analysis.is_animated {
        let Some(path) = path else {
            log_static_quality_branch(StaticQualityDbBranch::AnimatedNoPath);
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "static_image_quality_db",
                branch = "animated_no_path",
                "animated quality lookup requires a filesystem path; caller must use lookup_animated_image_quality or pass path"
            );
            return None;
        };
        log_static_quality_branch(StaticQualityDbBranch::AnimatedRoutedToScenarioDb);
        return crate::scenario_quality_lookup::lookup_animated_image_quality(path);
    }

    lookup_static_image_quality(analysis, path)
}

fn lookup_static_image_quality(
    analysis: &ImageAnalysis,
    path: Option<&Path>,
) -> Option<QualityScore> {
    let disable_db = static_quality_env_truthy(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB)
        || static_quality_env_truthy(crate::constants::ENV_DISABLE_DB_FEEDBACK);
    if disable_db {
        log_static_quality_branch(StaticQualityDbBranch::DbDisabledHeuristic);
        return refuse_static_quality_heuristic(
            "static_quality_db_disabled",
            "quality DB disabled; refusing heuristic score",
        );
    }

    let force_knn = static_quality_env_truthy(crate::constants::ENV_FORCE_QUALITY_KNN);
    if force_knn {
        log_static_quality_branch(StaticQualityDbBranch::ForceKnnEnvRefused);
        crate::media_conversion_gate::delivery_db_batch_audit(
            "static_quality_force_knn_refused",
            "ENV_FORCE_QUALITY_KNN refused: zero-tolerance policy forbids KNN-only decision scores",
        );
        return None;
    }

    let Ok(mut conn) = crate::database::open_pg_client() else {
        log_static_quality_branch(StaticQualityDbBranch::DbUnavailableHeuristic);
        return refuse_static_quality_heuristic(
            "static_quality_db_unavailable",
            "quality DB unavailable; refusing heuristic score",
        );
    };

    if !force_knn && !check_quality_db_maturity(&mut conn) {
        log_static_quality_branch(StaticQualityDbBranch::CorpusImmatureHeuristic);
        return refuse_static_quality_heuristic(
            "static_quality_corpus_immature",
            "quality corpus immature; refusing heuristic score",
        );
    }

    let heuristic_score = match bpp_heuristic_score(analysis) {
        Ok(v) => Some(v),
        Err(err) => {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "static_quality_heuristic_score_missing",
                format!("heuristic score unavailable (continuing without anchor); error: {err}"),
            );
            None
        }
    };
    let Ok(embedding) = get_quality_features(analysis) else {
        log_static_quality_branch(StaticQualityDbBranch::FeatureExtractionFailedHeuristic);
        return refuse_static_quality_heuristic(
            "static_quality_feature_extraction_failed",
            "feature extraction failed; refusing heuristic score",
        );
    };

    let knn_features = match query_image_quality_knn_features(&mut conn, &embedding) {
        Ok(Some(features)) if features.is_usable_for_regression() => {
            let Some(k) = features.seal_aggregates() else {
                log_static_quality_branch(StaticQualityDbBranch::KnnFeaturesUnusableHeuristic);
                return refuse_static_quality_heuristic(
                    "static_quality_knn_seal_rejected",
                    "KNN aggregates failed seal; refusing heuristic score",
                );
            };
            k
        }
        Ok(Some(_)) => {
            log_static_quality_branch(StaticQualityDbBranch::KnnFeaturesUnusableHeuristic);
            return refuse_static_quality_heuristic(
                "static_quality_knn_features_unusable",
                "KNN features unusable; refusing heuristic score",
            );
        }
        Ok(None) => {
            log_static_quality_branch(StaticQualityDbBranch::KnnNoNeighborsHeuristic);
            return refuse_static_quality_heuristic(
                "static_quality_knn_no_neighbors",
                "KNN returned no neighbors; refusing heuristic score",
            );
        }
        Err(err) => {
            log_static_quality_branch(StaticQualityDbBranch::KnnQueryFailedHeuristic);
            return refuse_static_quality_heuristic(
                "static_quality_knn_query_failed",
                format!("KNN query failed: {err}; refusing heuristic score"),
            );
        }
    };

    match crate::quality_regression_model::predict_image_quality(
        analysis,
        &embedding,
        &knn_features,
    ) {
        Ok(Some(model_prediction)) => {
            log_static_quality_branch(StaticQualityDbBranch::LightGbm);
            let prediction =
                lightgbm_python_prediction(&model_prediction, &knn_features, heuristic_score)?;
            let record = prediction.to_record();
            return deliver_log_static_quality(
                &mut conn,
                analysis,
                path,
                prediction,
                StaticQualityDbBranch::LightGbm,
                record,
            );
        }
        Ok(None) => {
            log_static_quality_branch(StaticQualityDbBranch::LightGbmUnavailableAbort);
            crate::media_conversion_gate::delivery_db_batch_audit(
                "static_quality_lightgbm_unavailable",
                "LightGBM returned no prediction; refusing KNN-only fallback",
            );
        }
        Err(error) => {
            log_static_quality_branch(StaticQualityDbBranch::LightGbmUnavailableAbort);
            crate::media_conversion_gate::delivery_db_batch_audit(
                "static_quality_lightgbm_unavailable",
                format!("LightGBM runtime unavailable; refusing KNN-only fallback: {error}"),
            );
        }
    }
    None
}

/// Read runtime quality verdict from an inference-log JSON snapshot (audit-only mode).
#[must_use]
pub fn quality_inference_runtime_verdict_from_snapshot(
    snapshot: &serde_json::Value,
) -> Option<&str> {
    snapshot
        .get("runtime_final_verdict")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
}

fn apply_quality_inference_audit_only(record: &mut QualityInferenceRecord) -> serde_json::Value {
    if !crate::algorithm_runtime::quality_inference_audit_only_mode() {
        return serde_json::json!({});
    }
    let runtime_verdict = record.final_verdict.clone();
    let runtime_branch = record.resolution_branch.clone();
    record.final_verdict = crate::constants::INFERENCE_TELEMETRY_ONLY_VERDICT.to_string();
    let snapshot = serde_json::json!({
        "audit_only": true,
        "runtime_final_verdict": runtime_verdict,
        "runtime_resolution_branch": runtime_branch,
    });
    tracing::debug!(
        target: "mfb.algorithm",
        pipeline = "static_image_quality_db",
        branch = "inference_log_audit_only",
        runtime_final_verdict = %runtime_verdict,
        "quality inference_log persisted in audit-only mode (final_verdict column is placeholder)"
    );
    snapshot
}

pub fn log_quality_inference_record(
    conn: &mut Client,
    _analysis: &ImageAnalysis,
    path: Option<&Path>,
    record: &QualityInferenceRecord,
) {
    let mut record = record.clone();
    record.seal_algorithm_outputs();
    let inference_snapshot = apply_quality_inference_audit_only(&mut record);

    let blake3_bytes = path.and_then(|p| match std::fs::read(p) {
        Ok(data) => Some(blake3::hash(&data).as_bytes().to_vec()),
        Err(err) => {
            tracing::debug!(
                target: "mfb.algorithm",
                pipeline = "static_image_quality_db",
                path = %p.display(),
                %err,
                "inference log: could not read file for blake3 hash; inserting NULL"
            );
            None
        }
    });
    let source_path = path.map(|p| p.to_string_lossy().to_string());

    let knn_neighbor_count = record
        .knn_neighbor_count
        .and_then(|count| crate::numeric_cast::usize_to_i32_strict(count, "knn_neighbor_count"));

    if let Err(error) = conn.execute(
        "INSERT INTO image_quality_inference_log (
            blake3, source_path, knn_score, knn_confidence, knn_neighbor_count,
            bpp_fallback_score, heuristic_score, regression_score, predictor_family, final_verdict,
            resolution_branch, inference_snapshot
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::jsonb)",
        &[
            &blake3_bytes,
            &source_path,
            &record.knn_score,
            &record.knn_confidence,
            &knn_neighbor_count,
            &record.bpp_fallback_score,
            &record.heuristic_score,
            &record.regression_score,
            &record.predictor_family,
            &record.final_verdict,
            &record.resolution_branch,
            &inference_snapshot,
        ],
    ) {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = "static_image_quality_db",
            branch = "inference_log_write_failed",
            error = %error,
            "static image quality inference log insert failed (non-fatal)"
        );
    }
}

/// Ingest a static image quality sample into `image_quality_samples`.
///
/// # Errors
///
/// Returns an error when the image cannot be analyzed, required physical
/// features are missing, numeric fields cannot be represented safely in the
/// `PostgreSQL` schema, or the insert fails.
pub fn ingest_quality_sample(
    conn: &mut Client,
    path: &Path,
    label: &str,
    labeled_by: &str,
) -> Result<()> {
    use crate::image_analyzer::analyze_image;
    let analysis = analyze_image(path).context("Analysis failed")?;
    if analysis.is_animated {
        return Ok(());
    }

    // 🛡️ ERADICATE SILENT FORGERY: Ingestion layer enforces data integrity.
    let entropy = analysis.features.entropy.ok_or_else(|| {
        anyhow::anyhow!(
            "INGESTION REJECTED: Entropy unmeasurable for '{}'; refusing to pollute feature space.",
            path.display()
        )
    })?;
    let comp_ratio = analysis.features.compression_ratio.ok_or_else(|| {
        anyhow::anyhow!(
            "INGESTION REJECTED: Compression ratio calculation failed for '{}'.",
            path.display()
        )
    })?;

    let data = std::fs::read(path).context("Failed to read file for hash")?;
    let blake3_bytes = blake3::hash(&data).as_bytes().to_vec();
    let embedding = get_quality_features(&analysis)?;
    let width_i32 = crate::numeric_cast::u32_to_i32_strict(analysis.width, "image_width")
        .ok_or_else(|| anyhow::anyhow!("Image width '{}' out of i32 range", analysis.width))?;
    let height_i32 = crate::numeric_cast::u32_to_i32_strict(analysis.height, "image_height")
        .ok_or_else(|| anyhow::anyhow!("Image height '{}' out of i32 range", analysis.height))?;
    let file_size_i64 =
        crate::numeric_cast::u64_to_i64_strict(analysis.file_size, "image_file_size").ok_or_else(
            || anyhow::anyhow!("Image file size '{}' out of i64 range", analysis.file_size),
        )?;
    let quality_score =
        crate::scenario::ImageQualityLabel::resolve_for_format(label, &analysis.format)?.to_score();

    let mut sample =
        crate::multi_scenario_db::ScenarioSample::new(blake3_bytes, ScenarioType::ImageQuality)
            .with_path(path.to_string_lossy().to_string())
            .with_label(label.to_string())
            .with_embedding(embedding)
            .with_dimensions(width_i32, height_i32)
            .with_size(file_size_i64)
            .with_format(analysis.format.clone())
            .with_entropy(Some(entropy))
            .with_compression_ratio(Some(comp_ratio))
            .with_lossless(analysis.is_lossless)
            .with_quality_score(quality_score);
    sample.labeled_by = Some(labeled_by.to_string());
    sample.metadata = build_image_quality_ingest_metadata(&analysis, Some(label), path)?;

    crate::multi_scenario_db::ingest_image_quality_sample(conn, &sample)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image_analyzer::{ImageAnalysis, ImageFeatures};
    use crate::multi_scenario_db::KnnRegressionFeatures;
    use crate::quality_regression_model::ModelQualityPrediction;
    use crate::types::Visual;
    use std::collections::HashMap;

    #[test]
    fn test_sanitize_stale_quality_measurement_embed_slots_rewrites_zero_sentinels() {
        let mut vec = vec![1.0_f32; 256];
        vec[QUALITY_EMBED_PSNR_SLOT] = 0.0;
        vec[QUALITY_EMBED_SSIM_SLOT] = 0.0;
        sanitize_stale_quality_measurement_embed_slots(&mut vec);
        assert!(
            (vec[QUALITY_EMBED_PSNR_SLOT] - QUALITY_EMBED_MISSING_MEASUREMENT).abs()
                <= f32::EPSILON
        );
        assert!(
            (vec[QUALITY_EMBED_SSIM_SLOT] - QUALITY_EMBED_MISSING_MEASUREMENT).abs()
                <= f32::EPSILON
        );
        assert!(vec[16].is_finite());
    }

    #[test]
    fn test_build_image_quality_ingest_metadata_includes_core_sections() {
        let mut metadata = HashMap::new();
        metadata.insert("dpi".to_string(), "300".to_string());

        let analysis = ImageAnalysis {
            width: 4000,
            height: 3000,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: true,
            has_alpha: true,
            color_depth: Some(8),
            features: ImageFeatures {
                entropy: Some(7.7),
                compression_ratio: Some(1.5),
            },
            perception: Visual {
                average_luma: 0.4,
                peak_luma: 0.95,
                gray_center_of_mass: (0.25, 0.75),
            },
            metadata,
            physics_225: Some(vec![0.5; 225]),
            ..ImageAnalysis::default()
        };

        let json = build_image_quality_ingest_metadata(&analysis, Some("high"), Path::new("x.png"))
            .expect("metadata");
        let obj = json.as_object().expect("metadata object");
        assert_eq!(
            obj.get("scenario_semantics").and_then(|v| v.as_str()),
            Some("image_quality")
        );
        assert!(obj.contains_key("geometry"));
        assert!(obj.contains_key("media"));
        assert!(obj.contains_key("precision"));
        assert!(obj.contains_key("signals"));
        assert!(obj.contains_key("perception"));
        assert!(obj.contains_key("source_metadata"));
        assert_eq!(
            obj.get("training_label").and_then(|v| v.as_str()),
            Some("high")
        );
        assert!(obj.contains_key("training_tier_audit"));
        let audit = obj
            .get("training_tier_audit")
            .and_then(|v| v.as_object())
            .expect("training_tier_audit");
        assert_eq!(
            audit.get("entropy_engine").and_then(|v| v.as_str()),
            Some("rust_analyze_image")
        );
        assert_eq!(
            audit
                .get("tier_consistent")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn test_get_quality_features_rejects_non_finite_required_features() {
        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: true,
            features: ImageFeatures {
                entropy: Some(f64::NAN),
                compression_ratio: Some(1.5),
            },
            ..ImageAnalysis::default()
        };

        assert!(get_quality_features(&analysis).is_err());
    }

    #[test]
    fn test_get_quality_features_rejects_zero_dimensions() {
        let analysis = ImageAnalysis {
            width: 0,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            features: ImageFeatures {
                entropy: Some(7.0),
                compression_ratio: Some(1.5),
            },
            physics_225: Some(vec![0.5; 225]),
            ..ImageAnalysis::default()
        };
        let err = get_quality_features(&analysis).unwrap_err().to_string();
        assert!(err.contains("dimensions"), "{err}");
    }

    #[test]
    fn test_get_quality_features_clamps_embedding_values() {
        let analysis = ImageAnalysis {
            width: u32::MAX,
            height: 1,
            file_size: u64::MAX,
            format: "JPEG".to_string(),
            is_lossless: false,
            psnr: Some(f64::INFINITY),
            ssim: Some(f64::NAN),
            features: ImageFeatures {
                entropy: Some(12.0),
                compression_ratio: Some(1.0e12),
            },
            physics_225: Some(vec![f32::NAN, -1.0, 2.0]),
            ..ImageAnalysis::default()
        };

        assert!(get_quality_features(&analysis).is_err());
    }

    #[test]
    fn test_get_quality_features_requires_physics_tail() {
        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: true,
            features: ImageFeatures {
                entropy: Some(7.7),
                compression_ratio: Some(1.5),
            },
            physics_225: None,
            ..ImageAnalysis::default()
        };

        assert!(get_quality_features(&analysis).is_err());
    }

    #[test]
    fn test_get_quality_features_restores_core_signal_dimensions() {
        let mut metadata = HashMap::new();
        metadata.insert("dpi".to_string(), "300".to_string());

        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "GIF".to_string(),
            is_lossless: true,
            has_alpha: true,
            color_depth: Some(16),
            features: ImageFeatures {
                entropy: Some(7.7),
                compression_ratio: Some(1.5),
            },
            perception: Visual {
                average_luma: 0.4,
                peak_luma: 0.95,
                gray_center_of_mass: (0.25, 0.75),
            },
            metadata,
            physics_225: Some(vec![0.5; 225]),
            ..ImageAnalysis::default()
        };

        let features = get_quality_features(&analysis).expect("feature extraction should succeed");
        let vec = features.as_slice();
        assert_eq!(vec.len(), 256);
        assert!(vec[6] > 0.9, "dpi_x dimension should be populated");
        assert!(vec[7] > 0.9, "dpi_y dimension should be populated");
        assert!(
            (vec[11] - 1.0).abs() < f32::EPSILON,
            "alpha flag should be populated"
        );
        assert!(
            vec[QUALITY_EMBED_COLOR_DEPTH_SLOT] > 0.9,
            "color depth dimension should be populated"
        );
        assert!((vec[13] - 0.4).abs() < 1.0e-6);
        assert!((vec[14] - 0.95).abs() < 1.0e-6);
        assert!((vec[15] - 0.25).abs() < 1.0e-6);
        assert!((vec[16] - 0.75).abs() < 1.0e-6);
        assert!(
            (vec[25] - 1.0).abs() < f32::EPSILON,
            "GIF indicator should be populated"
        );
        assert!((vec[31] - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn test_bpp_heuristic_quality_preserves_lossless_bonus() {
        let mut lossy = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: false,
            features: ImageFeatures {
                entropy: Some(4.0),
                compression_ratio: Some(2.0),
            },
            ..ImageAnalysis::default()
        };
        let lossless = ImageAnalysis {
            is_lossless: true,
            ..lossy.clone()
        };

        let lossy_score = bpp_heuristic_score(&lossy).expect("lossy heuristic should succeed");
        let lossless_score =
            bpp_heuristic_score(&lossless).expect("lossless heuristic should succeed");
        assert!(lossless_score > lossy_score);

        lossy.is_lossless = true;
        assert!(bpp_heuristic_quality(&lossy, "test reason").is_ok());
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_bpp_heuristic_quality_uses_conservative_confidence_floor() {
        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: false,
            features: ImageFeatures {
                entropy: Some(6.5),
                compression_ratio: Some(1.8),
            },
            ..ImageAnalysis::default()
        };

        let prediction =
            bpp_heuristic_quality(&analysis, "test reason").expect("heuristic should succeed");
        assert!(
            (prediction.confidence - crate::constants::HEURISTIC_SAFETY_FLOOR).abs() < 1.0e-12,
            "heuristic confidence must be conservative floor"
        );
        assert_ne!(
            prediction.score, prediction.confidence,
            "heuristic score must not be reused as confidence"
        );
    }

    #[test]
    fn test_knn_only_prediction_preserves_knn_path_identity() {
        let prediction = knn_only_prediction(
            &KnnRegressionFeatures {
                knn_score_mean_k5: 0.7,
                knn_score_std_k5: 0.1,
                knn_score_min_k5: 0.4,
                dist_to_nearest: 0.15,
                dist_weighted_score: 0.8,
                confidence: 0.9,
                neighbor_count: 5,
            },
            Some(0.3),
            None,
        )
        .expect("finite knn prediction");
        let quality = prediction.clone().into_quality_score();
        assert_eq!(quality.predictor_family, "knn_only");
        assert!((prediction.score - 0.8).abs() < 1.0e-9);
        assert!(prediction.confidence > 0.85);
    }

    #[test]
    fn test_hybrid_bootstrap_prediction_blends_prior_and_knn_signal() {
        let prediction = hybrid_bootstrap_prediction(
            0.2,
            &KnnRegressionFeatures {
                knn_score_mean_k5: 0.75,
                knn_score_std_k5: 0.05,
                knn_score_min_k5: 0.6,
                dist_to_nearest: 0.1,
                dist_weighted_score: 0.8,
                confidence: 0.9,
                neighbor_count: 5,
            },
            None,
            "static_image_quality_db",
        )
        .expect("finite hybrid prediction");
        let quality = prediction.clone().into_quality_score();
        assert_eq!(quality.predictor_family, "hybrid_bootstrap");
        assert!(prediction.score > 0.2);
        assert!(prediction.score < 0.8);
        assert!(prediction.confidence > 0.5);
    }

    #[test]
    fn test_hybrid_bootstrap_rejects_non_finite_heuristic() {
        let prediction = hybrid_bootstrap_prediction(
            f64::NAN,
            &KnnRegressionFeatures {
                knn_score_mean_k5: 0.75,
                knn_score_std_k5: 0.05,
                knn_score_min_k5: 0.6,
                dist_to_nearest: 0.1,
                dist_weighted_score: 0.8,
                confidence: 0.9,
                neighbor_count: 5,
            },
            Some("nan heuristic test".to_string()),
            "static_image_quality_db",
        );
        assert!(prediction.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn test_lightgbm_python_prediction_skips_disagreement_guard_when_disabled() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD,
            "1",
        );
        let model = ModelQualityPrediction {
            score: 0.95,
            confidence: 0.92,
        };
        let knn = KnnRegressionFeatures {
            knn_score_mean_k5: 0.3,
            knn_score_std_k5: 0.05,
            knn_score_min_k5: 0.2,
            dist_to_nearest: 0.12,
            dist_weighted_score: 0.22,
            confidence: 0.88,
            neighbor_count: 4,
        };
        let p = lightgbm_python_prediction(&model, &knn, None).expect("finite prediction");
        assert!((p.score - model.score).abs() < 1e-9);
    }

    #[test]
    #[serial_test::serial]
    fn test_lightgbm_python_prediction_fuses_toward_knn_on_large_disagreement() {
        let _guard = crate::common_utils::EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD,
            "0",
        );
        let model = ModelQualityPrediction {
            score: 0.95,
            confidence: 0.92,
        };
        let knn = KnnRegressionFeatures {
            knn_score_mean_k5: 0.3,
            knn_score_std_k5: 0.05,
            knn_score_min_k5: 0.2,
            dist_to_nearest: 0.12,
            dist_weighted_score: 0.22,
            confidence: 0.88,
            neighbor_count: 5,
        };
        let p = lightgbm_python_prediction(&model, &knn, None).expect("finite prediction");
        assert!(p.score < model.score);
        assert!(p.score > knn.dist_weighted_score);
        assert!((p.confidence - model.confidence).abs() < 1e-9);
    }

    #[test]
    fn test_lightgbm_python_prediction_respects_model_when_knn_not_trusted() {
        let model = ModelQualityPrediction {
            score: 0.95,
            confidence: 0.9,
        };
        let knn = KnnRegressionFeatures {
            knn_score_mean_k5: 0.2,
            knn_score_std_k5: 0.4,
            knn_score_min_k5: 0.1,
            dist_to_nearest: 2.0,
            dist_weighted_score: 0.15,
            confidence: 0.35,
            neighbor_count: 5,
        };
        let p = lightgbm_python_prediction(&model, &knn, None).expect("finite prediction");
        assert!((p.score - model.score).abs() < 1e-9);
    }

    #[test]
    fn test_deliver_quality_prediction_rejects_non_finite() {
        let prediction = ImageQualityPrediction {
            score: f64::NAN,
            confidence: 0.8,
            predictor_family: ImageQualityPredictorFamily::KnnOnly,
            fallback_reason: None,
            knn_score: None,
            knn_confidence: None,
            knn_neighbor_count: None,
            bpp_fallback_score: None,
            heuristic_score: None,
            regression_score: None,
        };
        assert!(deliver_quality_prediction(prediction).is_none());
    }

    #[test]
    fn fallback_delivery_reject_non_finite_is_audited_and_fail_closed() {
        let fallback = ImageQualityPrediction {
            score: f64::NAN,
            confidence: 0.8,
            predictor_family: ImageQualityPredictorFamily::HeuristicOnly,
            fallback_reason: Some("test".to_string()),
            knn_score: None,
            knn_confidence: None,
            knn_neighbor_count: None,
            bpp_fallback_score: Some(f64::NAN),
            heuristic_score: Some(f64::NAN),
            regression_score: Some(f64::NAN),
        };
        assert!(
            deliver_quality_prediction(fallback).is_none(),
            "non-finite prediction must fail-closed at seal"
        );
    }

    #[test]
    fn test_fuse_quality_regression_prediction_blends_heuristic_and_db_score() {
        let prediction = QualityScore {
            score: 0.75,
            confidence: 0.8,
            predictor_family: "hybrid_bootstrap".into(),
            fallback_reason: None,
            knn_neighbor_count: Some(5),
        };
        let fused = fuse_quality_regression_prediction(Some(60), prediction).expect("fused");
        assert!((55..=85).contains(&fused));
    }

    #[test]
    fn static_quality_db_branch_tags_are_stable() {
        assert_eq!(StaticQualityDbBranch::LightGbm.as_str(), "lightgbm");
        assert_eq!(
            StaticQualityDbBranch::LightGbmUnavailableAbort.as_str(),
            "lightgbm_unavailable_abort"
        );
    }

    #[test]
    fn lightgbm_unavailable_abort_branch_tag_is_stable() {
        assert_eq!(
            StaticQualityDbBranch::LightGbmUnavailableAbort.as_str(),
            "lightgbm_unavailable_abort"
        );
        assert!(StaticQualityDbBranch::LightGbmUnavailableAbort.is_heuristic_only_branch());
    }

    #[test]
    fn force_knn_env_refused_branch_tag_is_stable() {
        assert_eq!(
            StaticQualityDbBranch::ForceKnnEnvRefused.as_str(),
            "force_knn_env_refused"
        );
        assert!(StaticQualityDbBranch::ForceKnnEnvRefused.is_heuristic_only_branch());
    }

    #[test]
    fn test_fuse_quality_regression_prediction_rejects_non_finite_score() {
        let prediction = QualityScore {
            score: f64::NAN,
            confidence: 0.9,
            predictor_family: "knn_only".into(),
            fallback_reason: None,
            knn_neighbor_count: Some(5),
        };
        assert_eq!(
            fuse_quality_regression_prediction(Some(70), prediction.clone()),
            Some(70),
            "poisoned DB prediction must not blend; heuristic-only when present"
        );
        assert!(
            fuse_quality_regression_prediction(None, prediction).is_none(),
            "poisoned DB prediction with no heuristic must not fabricate a score"
        );
    }

    #[test]
    fn test_get_quality_features_uses_pgvector_safe_missing_measurement_sentinel() {
        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: true,
            color_depth: None,
            features: ImageFeatures {
                entropy: Some(7.7),
                compression_ratio: Some(1.5),
            },
            perception: Visual {
                average_luma: 0.4,
                peak_luma: 0.95,
                gray_center_of_mass: (0.25, 0.75),
            },
            physics_225: Some(vec![0.5; 225]),
            ..ImageAnalysis::default()
        };

        let features = get_quality_features(&analysis).expect("feature extraction should succeed");
        let vec = features.as_slice();
        assert!(
            (vec[QUALITY_EMBED_COLOR_DEPTH_SLOT] - QUALITY_EMBED_MISSING_MEASUREMENT).abs()
                <= f32::EPSILON,
            "unknown color depth must use finite missing-measurement sentinel"
        );
        assert!(
            (vec[QUALITY_EMBED_JPEG_QUALITY_SLOT] - QUALITY_EMBED_MISSING_MEASUREMENT).abs()
                <= f32::EPSILON,
            "non-JPEG assets must not fabricate JPEG sidecar quality as 0.0"
        );
        assert!(
            (vec[QUALITY_EMBED_JPEG_CONFIDENCE_SLOT] - QUALITY_EMBED_MISSING_MEASUREMENT).abs()
                <= f32::EPSILON,
            "non-JPEG assets must not fabricate JPEG sidecar confidence as 0.0"
        );
        assert!(vec.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn animated_without_path_returns_none() {
        let analysis = ImageAnalysis {
            is_animated: true,
            ..ImageAnalysis::default()
        };
        assert!(lookup_image_quality_with_path(&analysis, None).is_none());
    }

    #[test]
    fn heuristic_branches_do_not_populate_knn_score_column() {
        assert!(StaticQualityDbBranch::CorpusImmatureHeuristic.is_heuristic_only_branch());
        assert!(StaticQualityDbBranch::DbUnavailableHeuristic.is_heuristic_only_branch());
        assert!(!StaticQualityDbBranch::LightGbm.is_heuristic_only_branch());

        let branch = StaticQualityDbBranch::CorpusImmatureHeuristic;
        let mut record = QualityInferenceRecord {
            knn_score: None,
            knn_confidence: None,
            knn_neighbor_count: None,
            bpp_fallback_score: None,
            heuristic_score: None,
            regression_score: None,
            predictor_family: "heuristic_only".to_string(),
            final_verdict: "immature".to_string(),
            resolution_branch: branch.as_str().to_string(),
        };
        let score = 0.42_f64;
        let confidence = 0.42_f64;
        if branch.is_heuristic_only_branch() {
            record.heuristic_score = Some(score);
            record.bpp_fallback_score = Some(score);
        } else {
            record.knn_score = Some(score);
            record.knn_confidence = Some(confidence);
        }
        assert!(record.knn_score.is_none());
        assert!(record.knn_confidence.is_none());
        assert_eq!(record.heuristic_score, Some(score));
        assert_eq!(record.bpp_fallback_score, Some(score));

        let snapshot = apply_quality_inference_audit_only(&mut record);
        assert_eq!(
            snapshot
                .get("runtime_resolution_branch")
                .and_then(|v| v.as_str()),
            Some("corpus_immature_heuristic")
        );
    }

    #[test]
    fn quality_inference_runtime_verdict_from_snapshot_reads_audit_field() {
        let snapshot = serde_json::json!({
            "audit_only": true,
            "runtime_final_verdict": "knn_only",
            "runtime_resolution_branch": "knn_success"
        });
        assert_eq!(
            quality_inference_runtime_verdict_from_snapshot(&snapshot),
            Some("knn_only")
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_env_disable_compatibility() {
        unsafe {
            std::env::set_var(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB, "1");
        }

        let analysis = ImageAnalysis {
            width: 1920,
            height: 1080,
            file_size: 500_000,
            format: "PNG".to_string(),
            is_lossless: true,
            features: ImageFeatures {
                entropy: Some(4.0),
                compression_ratio: Some(2.0),
            },
            ..ImageAnalysis::default()
        };

        let res = lookup_image_quality(&analysis);
        assert!(
            res.is_none(),
            "zero-tolerance: DB disabled must not emit heuristic decision scores"
        );

        unsafe {
            std::env::remove_var(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB);
        }
    }
}

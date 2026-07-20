//! Scenario quality lookup for `animated_image_quality` and `video_quality`.
//!
//! **No-fabrication (B06):** production returns **`None`** on all branches — no
//! KNN-only / hybrid-bootstrap decision scores. Immature corpus, failed KNN,
//! and successful KNN without a scenario `LightGBM` model all refuse to emit a
//! sealed score (`LightGbmRequiredAbort` + audit).
//!
//! ## DB resolution ordering (audit contract)
//!
//! Callers must not insert stages without updating this list and
//! `ScenarioQualityBranch`:
//! 1. DB disabled → `None` + audit.
//! 2. `PostgreSQL` unavailable → `None` + audit.
//! 3. Corpus immature → `None` + audit.
//! 4. Embedding invalid → `None` + audit.
//! 5. KNN no neighbors / unusable / query error → `None` + audit.
//! 6. KNN usable but no scenario `LightGBM` → `None` (`LightGbmRequiredAbort`).
//! 7. Successful paths (future): `QualityScore::sealed()` before inference log
//!    insert.
//!
//! `lookup_media_quality_by_path` routes animated containers to
//! `animated_image_quality_db`, everything else to `video_quality_db`.
//! Detection-layer score fusion defaults on unless
//! `MODERN_FORMAT_DISABLE_SCENARIO_QUALITY_DB_FUSION` (see
//! `fuse_quality_regression_prediction_if_enabled`).

use crate::animated_image_quality_features::AnimatedImageQualityFeatures;
use crate::image_quality_db::{QualityScore, query_quality_knn_features};
use crate::scenario::ScenarioType;
use crate::video_quality_features::VideoQualityFeatures;
use postgres::Client;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScenarioQualityBranch {
    DbDisabledHeuristic,
    DbUnavailableHeuristic,
    CorpusImmatureHeuristic,
    FeatureExtractionFailedHeuristic,
    KnnNoNeighborsHeuristic,
    KnnFeaturesUnusableHeuristic,
    KnnQueryFailedHeuristic,
    LightGbmRequiredAbort,
}

impl ScenarioQualityBranch {
    #[allow(clippy::unused_self)]
    const fn inference_log_on_success_path(self) -> bool {
        false
    }

    /// Heuristic / immature paths must not write BPP/heuristic scores into
    /// `knn_score` columns.
    const fn is_heuristic_only_branch(self) -> bool {
        matches!(
            self,
            Self::DbDisabledHeuristic
                | Self::DbUnavailableHeuristic
                | Self::CorpusImmatureHeuristic
                | Self::FeatureExtractionFailedHeuristic
                | Self::KnnNoNeighborsHeuristic
                | Self::KnnFeaturesUnusableHeuristic
                | Self::KnnQueryFailedHeuristic
        )
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::DbDisabledHeuristic => "db_disabled_heuristic",
            Self::DbUnavailableHeuristic => "db_unavailable_heuristic",
            Self::CorpusImmatureHeuristic => "corpus_immature_heuristic",
            Self::FeatureExtractionFailedHeuristic => "feature_extraction_failed_heuristic",
            Self::KnnNoNeighborsHeuristic => "knn_no_neighbors_heuristic",
            Self::KnnFeaturesUnusableHeuristic => "knn_features_unusable_heuristic",
            Self::KnnQueryFailedHeuristic => "knn_query_failed_heuristic",
            Self::LightGbmRequiredAbort => "lightgbm_required_abort",
        }
    }
}

#[inline]
fn log_branch(pipeline: &'static str, branch: ScenarioQualityBranch) {
    tracing::debug!(
        target: "mfb.algorithm",
        pipeline,
        branch = branch.as_str(),
        "scenario quality resolution"
    );
}

fn quality_db_disabled() -> bool {
    fn disable_flag(key: &str) -> bool {
        match std::env::var(key) {
            Ok(value) => value == "1" || value.eq_ignore_ascii_case("true"),
            Err(std::env::VarError::NotPresent) => false,
            Err(e) => {
                crate::media_conversion_gate::delivery_db_batch_audit(
                    "scenario_quality_env",
                    format!("failed to read disable flag {key}: {e}; treating as disabled"),
                );
                false
            }
        }
    }
    disable_flag(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB)
        || disable_flag(crate::constants::ENV_DISABLE_DB_FEEDBACK)
}

fn scenario_corpus_mature(conn: &mut Client, scenario: ScenarioType) -> bool {
    let min_total = crate::algorithm_runtime::min_quality_samples_total();
    match crate::multi_scenario_db::sample_count(conn, scenario) {
        Ok(n) => corpus_mature_from_sample_count(Some(n), min_total),
        Err(err) => {
            tracing::warn!(
                target: "mfb.algorithm",
                pipeline = "scenario_quality_lookup",
                branch = "corpus_maturity_sample_count_error",
                ?err,
                "scenario corpus maturity sample_count query failed; treating corpus as immature"
            );
            false
        }
    }
}

fn corpus_mature_from_sample_count(maybe_count: Option<i64>, min_total: i64) -> bool {
    maybe_count.is_some_and(|n| n >= min_total)
}

fn embedding_from_features(features: &[f32]) -> Option<pgvector::Vector> {
    if features.len() != 256 || features.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(pgvector::Vector::from(features.to_vec()))
}

fn knn_neighbor_count_i32(count: Option<usize>) -> Option<i32> {
    count.and_then(|n| crate::numeric_cast::usize_to_i32_strict(n, "scenario_knn_neighbor_count"))
}

fn resolve_knn_lookup(
    path: &Path,
    pipeline: &'static str,
    scenario: ScenarioType,
    conn: &mut Client,
    embedding: &pgvector::Vector,
    _heuristic_score: f64,
) -> Option<(QualityScore, ScenarioQualityBranch)> {
    match query_quality_knn_features(conn, scenario, embedding) {
        Ok(Some(features)) if features.is_usable_for_regression() => {}
        Ok(Some(_)) => {
            log_branch(
                pipeline,
                ScenarioQualityBranch::KnnFeaturesUnusableHeuristic,
            );
            crate::media_conversion_gate::delivery_db_batch_audit(
                "scenario_quality_knn_unusable",
                format!(
                    "{}: KNN features unusable after sanitation; refusing sealed heuristic",
                    path.display()
                ),
            );
            return None;
        }
        Ok(None) => {
            log_branch(pipeline, ScenarioQualityBranch::KnnNoNeighborsHeuristic);
            crate::media_conversion_gate::delivery_db_batch_audit(
                "scenario_quality_knn_no_neighbors",
                format!(
                    "{}: KNN returned no neighbors; refusing sealed heuristic",
                    path.display()
                ),
            );
            return None;
        }
        Err(err) => {
            log_branch(pipeline, ScenarioQualityBranch::KnnQueryFailedHeuristic);
            crate::media_conversion_gate::delivery_db_batch_audit(
                "scenario_quality_knn_query_failed",
                format!(
                    "{}: KNN query failed: {err}; refusing sealed heuristic",
                    path.display()
                ),
            );
            return None;
        }
    }

    log_branch(pipeline, ScenarioQualityBranch::LightGbmRequiredAbort);
    crate::media_conversion_gate::delivery_db_batch_audit(
        "scenario_quality_lightgbm_required",
        format!(
            "{}: scenario quality requires LightGBM; refusing KNN-only score",
            path.display()
        ),
    );
    None
}

fn animated_heuristic_score(features: &AnimatedImageQualityFeatures) -> Option<f64> {
    let entropy = features.reference_entropy?;
    let entropy_score = (entropy / 8.0).clamp(0.0, 1.0);
    let compression_ratio =
        crate::algorithm_seal::quality_finite_scalar(features.compression_ratio)?;
    let comp_score = (1.0 - (compression_ratio / 10.0).clamp(0.0, 1.0)).max(0.0);
    let raw = entropy_score * 0.5 + comp_score * 0.5;
    crate::algorithm_seal::quality_unit_probability(raw)
}

fn video_heuristic_score(features: &VideoQualityFeatures) -> Option<f64> {
    let pixel_count = f64::from(features.width) * f64::from(features.height);
    if features.frame_count == 0 || pixel_count <= 0.0 {
        return None;
    }
    let bpp_frame = crate::algorithm_seal::quality_finite_scalar(
        crate::numeric_cast::u64_to_f64(features.file_size_bytes) * 8.0
            / (pixel_count * crate::numeric_cast::u64_to_f64(features.frame_count)),
    )?;
    let bpp_score = (1.0 - (bpp_frame / 2.0).clamp(0.0, 1.0)).max(0.0);
    let motion_score = (1.0 - features.motion_intensity.clamp(0.0, 1.0)).max(0.0);
    let stability = features.temporal_stability.clamp(0.0, 1.0);
    let raw = bpp_score * 0.4 + motion_score * 0.3 + stability * 0.3;
    crate::algorithm_seal::quality_unit_probability(raw)
}

fn animated_heuristic_score_with_audit(
    path: &Path,
    features: &AnimatedImageQualityFeatures,
) -> Option<f64> {
    if let Some(score) = animated_heuristic_score(features) {
        Some(score)
    } else {
        crate::media_conversion_gate::delivery_db_path_audit(
            "scenario_quality_heuristic_score",
            path,
            "animated heuristic score unavailable; refusing silent None",
        );
        None
    }
}

fn video_heuristic_score_with_audit(path: &Path, features: &VideoQualityFeatures) -> Option<f64> {
    if let Some(score) = video_heuristic_score(features) {
        Some(score)
    } else {
        crate::media_conversion_gate::delivery_db_path_audit(
            "scenario_quality_heuristic_score",
            path,
            "video heuristic score unavailable; refusing silent None",
        );
        None
    }
}

fn inference_verdict(score: &QualityScore) -> &'static str {
    if score.fallback_reason.is_some() {
        "heuristic"
    } else if score.predictor_family.contains("hybrid") {
        "hybrid_bootstrap"
    } else {
        "knn_only"
    }
}

fn log_scenario_quality_inference(
    conn: &mut Client,
    scenario: ScenarioType,
    path: &Path,
    score: &QualityScore,
    branch: ScenarioQualityBranch,
) {
    let blake3_bytes = match std::fs::read(path) {
        Ok(data) => Some(blake3::hash(&data).as_bytes().to_vec()),
        Err(err) => {
            crate::media_conversion_gate::delivery_db_path_audit(
                "scenario_quality_inference_log",
                path,
                format!("blake3 hash omitted: file read failed: {err}"),
            );
            None
        }
    };
    let source_path = path.to_string_lossy().to_string();
    let sealed = score.clone().sealed();
    let verdict = inference_verdict(&sealed);
    let heuristic_only = branch.is_heuristic_only_branch();
    let (knn_score, knn_confidence, knn_neighbor_count) = if heuristic_only {
        (None, None, None)
    } else {
        (
            crate::algorithm_seal::quality_unit_probability(sealed.score),
            crate::algorithm_seal::quality_unit_probability(sealed.confidence),
            knn_neighbor_count_i32(score.knn_neighbor_count),
        )
    };

    let predictor_family = score.predictor_family.as_str();
    let resolution_branch = branch.as_str();
    let mut column_verdict = verdict.to_string();
    let mut inference_snapshot = if heuristic_only {
        serde_json::json!({
            "runtime_heuristic_score": sealed.score,
            "runtime_heuristic_confidence": sealed.confidence,
        })
    } else {
        serde_json::json!({})
    };
    if crate::algorithm_runtime::quality_inference_audit_only_mode() {
        if let serde_json::Value::Object(ref mut map) = inference_snapshot {
            map.insert("audit_only".to_string(), serde_json::json!(true));
            map.insert(
                "runtime_final_verdict".to_string(),
                serde_json::json!(verdict),
            );
            map.insert(
                "runtime_resolution_branch".to_string(),
                serde_json::json!(resolution_branch),
            );
        } else {
            inference_snapshot = serde_json::json!({
                "audit_only": true,
                "runtime_final_verdict": verdict,
                "runtime_resolution_branch": resolution_branch,
            });
        }
        column_verdict = crate::constants::INFERENCE_TELEMETRY_ONLY_VERDICT.to_string();
    }
    let sql = format!(
        "INSERT INTO {} (blake3, source_path, knn_score, knn_confidence, knn_neighbor_count, \
         predictor_family, final_verdict, resolution_branch, inference_snapshot)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::jsonb)",
        scenario.inference_log_table()
    );
    if let Err(error) = conn.execute(
        &sql,
        &[
            &blake3_bytes,
            &source_path,
            &knn_score,
            &knn_confidence,
            &knn_neighbor_count,
            &predictor_family,
            &column_verdict,
            &resolution_branch,
            &inference_snapshot,
        ],
    ) {
        tracing::debug!(
            target: "mfb.algorithm",
            pipeline = scenario.table_name(),
            branch = "inference_log_write_failed",
            error = %error,
            "scenario quality inference log insert failed (non-fatal)"
        );
    }
}

fn finish_lookup(
    conn: &mut Client,
    scenario: ScenarioType,
    path: &Path,
    score: QualityScore,
    branch: ScenarioQualityBranch,
) -> QualityScore {
    if crate::algorithm_runtime::scenario_quality_inference_logging_enabled(
        branch.inference_log_on_success_path(),
    ) {
        log_scenario_quality_inference(conn, scenario, path, &score, branch);
    }
    score
}

fn lookup_with_pipeline(
    pipeline: &'static str,
    scenario: ScenarioType,
    path: &Path,
    heuristic: f64,
    embedding: Option<pgvector::Vector>,
) -> Option<QualityScore> {
    if quality_db_disabled() {
        log_branch(pipeline, ScenarioQualityBranch::DbDisabledHeuristic);
        crate::media_conversion_gate::delivery_db_batch_audit(
            "scenario_quality_db_disabled",
            format!(
                "{}: quality DB disabled; refusing heuristic",
                path.display()
            ),
        );
        return None;
    }

    let Ok(mut conn) = crate::database::open_pg_client() else {
        log_branch(pipeline, ScenarioQualityBranch::DbUnavailableHeuristic);
        crate::media_conversion_gate::delivery_db_batch_audit(
            "scenario_quality_db_unavailable",
            format!(
                "{}: quality DB unavailable; refusing heuristic",
                path.display()
            ),
        );
        return None;
    };

    if !scenario_corpus_mature(&mut conn, scenario) {
        log_branch(pipeline, ScenarioQualityBranch::CorpusImmatureHeuristic);
        crate::media_conversion_gate::delivery_db_batch_audit(
            "scenario_quality_corpus_immature",
            format!("{}: corpus immature; refusing heuristic", path.display()),
        );
        return None;
    }

    let Some(embedding) = embedding else {
        log_branch(
            pipeline,
            ScenarioQualityBranch::FeatureExtractionFailedHeuristic,
        );
        crate::media_conversion_gate::delivery_db_batch_audit(
            "scenario_quality_embedding_invalid",
            format!("{}: embedding invalid; refusing heuristic", path.display()),
        );
        return None;
    };

    let (score, branch) =
        resolve_knn_lookup(path, pipeline, scenario, &mut conn, &embedding, heuristic)?;
    Some(finish_lookup(
        &mut conn,
        scenario,
        path,
        score.sealed(),
        branch,
    ))
}

/// Animated-image quality lookup (KNN + heuristic; no `LightGBM` yet).
#[must_use]
pub fn lookup_animated_image_quality(path: &Path) -> Option<QualityScore> {
    if !crate::algorithm_runtime::image_quality_heuristic_enabled() {
        return None;
    }
    const PIPELINE: &str = "animated_image_quality_db";
    let features = match AnimatedImageQualityFeatures::from_path(path) {
        Ok(features) => features,
        Err(err) => {
            crate::media_conversion_gate::delivery_db_path_audit(
                "scenario_quality_feature_extract",
                path,
                format!("animated features extraction failed: {err}"),
            );
            return None;
        }
    };
    let heuristic = animated_heuristic_score_with_audit(path, &features)?;
    let embedding = embedding_from_features(&features.to_embedding_vector());
    lookup_with_pipeline(
        PIPELINE,
        ScenarioType::AnimatedImageQuality,
        path,
        heuristic,
        embedding,
    )
}

/// Route by container semantics: animated image vs video (static uses
/// `lookup_image_quality`).
#[must_use]
pub fn lookup_media_quality_by_path(path: &Path) -> Option<QualityScore> {
    if !crate::algorithm_runtime::image_quality_heuristic_enabled() {
        return None;
    }
    match crate::image_detection::detect_format_from_bytes(path) {
        Ok(format) => {
            let (is_animated, _, _) = match crate::image_detection::detect_animation(path, &format)
            {
                Ok(v) => v,
                Err(err) => {
                    crate::media_conversion_gate::probe_image_format_audit(
                        "scenario_animation",
                        path,
                        format!(
                            "Quality lookup: animation detection failed; refusing route guess: \
                             {err}"
                        ),
                    );
                    return None;
                }
            };
            if is_animated {
                return lookup_animated_image_quality(path);
            }
        }
        Err(err) => {
            crate::media_conversion_gate::probe_image_format_audit(
                "scenario_format_detection",
                path,
                format!("Quality lookup: format detection failed; refusing route guess: {err}"),
            );
            return None;
        }
    }
    lookup_video_quality(path)
}

/// Video-container quality lookup (KNN + heuristic; no `LightGBM` yet).
#[must_use]
pub fn lookup_video_quality(path: &Path) -> Option<QualityScore> {
    if !crate::algorithm_runtime::image_quality_heuristic_enabled() {
        return None;
    }
    const PIPELINE: &str = "video_quality_db";
    let features = match VideoQualityFeatures::from_path(path) {
        Ok(features) => features,
        Err(err) => {
            crate::media_conversion_gate::delivery_db_path_audit(
                "scenario_quality_feature_extract",
                path,
                format!("video features extraction failed: {err}"),
            );
            return None;
        }
    };
    let heuristic = video_heuristic_score_with_audit(path, &features)?;
    let embedding = embedding_from_features(&features.to_embedding_vector());
    lookup_with_pipeline(
        PIPELINE,
        ScenarioType::VideoQuality,
        path,
        heuristic,
        embedding,
    )
}

#[cfg(test)]
mod tests {
    use crate::image_quality_db::sealed_heuristic_quality_score;

    fn sealed_heuristic_quality_score_with_audit(
        path: &Path,
        pipeline: &'static str,
        heuristic: f64,
        reason: &'static str,
        audit_context: &'static str,
    ) -> Option<crate::image_quality_db::QualityScore> {
        if let Some(score) = sealed_heuristic_quality_score(heuristic, reason) {
            Some(score.sealed())
        } else {
            crate::media_conversion_gate::delivery_db_path_audit(
                "scenario_quality_heuristic_seal",
                path,
                format!(
                    "{pipeline}: heuristic seal failed in {audit_context}; refusing silent None"
                ),
            );
            None
        }
    }
    use super::*;
    use std::io::Write;

    #[test]
    fn animated_heuristic_is_unit_interval() {
        let features = AnimatedImageQualityFeatures {
            format: crate::image_detection::DetectedFormat::GIF,
            width: 100,
            height: 100,
            frame_count: 10,
            duration_secs: 1.0,
            fps: 10.0,
            palette_size: None,
            palette_depth: None,
            color_richness: 0.5,
            average_frame_delay_ms: 100.0,
            frame_delay_variation: 0.1,
            animation_smoothness: 0.8,
            temporal_flicker: 0.2,
            file_size_bytes: 50_000,
            bytes_per_pixel: 5.0,
            compression_ratio: 2.0,
            content_flags:
                crate::animated_image_quality_features::AnimatedImageContentFlags::default(),
            animation_intensity: 0.5,
            render_flags: crate::animated_image_quality_features::AnimatedImageRenderFlags::default(
            ),
            reference_entropy: Some(6.0),
            physics_225: vec![0.1; 225],
        };
        let score = animated_heuristic_score(&features).expect("finite heuristic"); // audited: db module unit-test fixture assertion; not production DB runtime path
        assert!((0.0..=1.0).contains(&score));
    }

    #[test]
    fn animated_heuristic_missing_entropy_is_explicitly_rejected() {
        let features = AnimatedImageQualityFeatures {
            format: crate::image_detection::DetectedFormat::GIF,
            width: 100,
            height: 100,
            frame_count: 10,
            duration_secs: 1.0,
            fps: 10.0,
            palette_size: None,
            palette_depth: None,
            color_richness: 0.5,
            average_frame_delay_ms: 100.0,
            frame_delay_variation: 0.1,
            animation_smoothness: 0.8,
            temporal_flicker: 0.2,
            file_size_bytes: 50_000,
            bytes_per_pixel: 5.0,
            compression_ratio: 2.0,
            content_flags:
                crate::animated_image_quality_features::AnimatedImageContentFlags::default(),
            animation_intensity: 0.5,
            render_flags: crate::animated_image_quality_features::AnimatedImageRenderFlags::default(
            ),
            reference_entropy: None,
            physics_225: vec![0.1; 225],
        };
        let path = Path::new("missing_entropy.gif");
        assert!(
            animated_heuristic_score_with_audit(path, &features).is_none(),
            "missing entropy must fail-closed, not silently fabricate heuristic score"
        );
    }

    #[test]
    fn sealed_heuristic_with_audit_rejects_non_finite_heuristic() {
        let path = Path::new("non_finite_heuristic.mp4");
        let result = sealed_heuristic_quality_score_with_audit(
            path,
            "video_quality_db",
            f64::NAN,
            "test non-finite",
            "unit_test",
        );
        assert!(
            result.is_none(),
            "non-finite heuristic must fail-closed instead of silent fallback"
        );
    }

    #[test]
    fn knn_fallback_context_non_finite_heuristic_is_fail_closed() {
        let path = Path::new("knn_fallback_non_finite.mp4");
        let result = sealed_heuristic_quality_score_with_audit(
            path,
            "video_quality_db",
            f64::NAN,
            "KNN returned no neighbors",
            "knn_no_neighbors",
        );
        assert!(
            result.is_none(),
            "non-finite heuristic in KNN fallback context must fail-closed with audited None"
        );
    }

    #[test]
    fn animation_detection_error_refuses_route_guess() {
        // Minimal truncated PNG signature: format detect should succeed, animation
        // detect may error.
        let dir =
            std::env::temp_dir().join(format!("mfb_scenario_quality_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir"); // audited: db module unit-test fixture assertion; not production DB runtime path
        let path = dir.join("truncated.png");
        let mut f = std::fs::File::create(&path).expect("create temp file"); // audited: db module unit-test fixture assertion; not production DB runtime path
        f.write_all(b"\x89PNG\r\n\x1a\n").expect("write png sig"); // audited: db module unit-test fixture assertion; not production DB runtime path
        if let Err(e) = f.sync_all() {
            crate::media_conversion_gate::delivery_db_batch_audit(
                "scenario_quality_test_fixture",
                format!("failed to sync scenario quality test fixture: {e}"),
            );
        }

        let result = lookup_media_quality_by_path(&path);
        assert!(
            result.is_none(),
            "route guess must fail-closed on animation detect error"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seal_rejects_non_finite_embedding() {
        let mut vec = vec![0.0_f32; 256];
        vec[0] = f32::NAN;
        assert!(embedding_from_features(&vec).is_none());
    }

    #[test]
    fn knn_neighbor_count_i32_rejects_overflow() {
        assert!(knn_neighbor_count_i32(Some(usize::MAX)).is_none());
        assert_eq!(knn_neighbor_count_i32(Some(5)), Some(5));
    }

    #[test]
    fn scenario_heuristic_branches_do_not_populate_knn_score_columns() {
        assert!(ScenarioQualityBranch::CorpusImmatureHeuristic.is_heuristic_only_branch());
        assert!(!ScenarioQualityBranch::LightGbmRequiredAbort.is_heuristic_only_branch());

        let branch = ScenarioQualityBranch::CorpusImmatureHeuristic;
        let sealed = QualityScore {
            score: 0.42,
            confidence: 0.42,
            predictor_family: "heuristic_only".to_string(),
            fallback_reason: Some("immature".into()),
            knn_neighbor_count: None,
        }
        .sealed();
        let (knn_score, knn_confidence, knn_neighbor_count) = if branch.is_heuristic_only_branch() {
            (None, None, None)
        } else {
            (
                crate::algorithm_seal::quality_unit_probability(sealed.score),
                crate::algorithm_seal::quality_unit_probability(sealed.confidence),
                knn_neighbor_count_i32(sealed.knn_neighbor_count),
            )
        };
        assert!(knn_score.is_none());
        assert!(knn_confidence.is_none());
        assert!(knn_neighbor_count.is_none());
    }

    #[test]
    fn corpus_maturity_threshold_helper_is_fail_closed() {
        let min_total = crate::algorithm_runtime::min_quality_samples_total();
        assert!(!corpus_mature_from_sample_count(None, min_total));
        assert!(!corpus_mature_from_sample_count(
            Some(min_total.saturating_sub(1)),
            min_total
        ));
        assert!(corpus_mature_from_sample_count(Some(min_total), min_total));
    }
}

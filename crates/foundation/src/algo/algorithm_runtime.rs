//! Runtime toggles for algorithm-layer behavior (env-gated semantic features).
//!
//! **Tightened defaults:** HNSW quorum ≥2; loop/quality/exploration unit
//! probabilities always clamp/reject non-finite in [`crate::algorithm_seal`];
//! structural sealing default **on** (disable via `MODERN_FORMAT_DISABLE_*
//! _ALGORITHM_SEAL`); Layer 6 KNN default **off** unless explicitly enabled;
//! loop `feature_stats` fail-closed (fail-open opt-in only); loop `inference_log` +
//! audit-only default **on**; exploration SSIM-presence/threshold/size-target +
//! confidence gates default **on**; strict corpus maturity default **on**;
//! quality DB lookup/fusion and HDBSCAN fusion default **on** (disable via
//! `MODERN_FORMAT_DISABLE_*`).

#[cfg(not(test))]
use std::sync::OnceLock;

#[inline]
fn env_truthy(key: &str) -> bool {
    match std::env::var(key) {
        Ok(value) => value == "1" || value.eq_ignore_ascii_case("true"),
        Err(std::env::VarError::NotPresent) => false,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "algorithm_runtime_env",
                format!("failed to read env flag {key}: {e}; treating as disabled"),
            );
            false
        }
    }
}

/// Default-on algorithm gates: enabled unless the disable kill-switch is set.
#[inline]
fn algorithm_gate_enabled(disable_key: &str) -> bool {
    !env_truthy(disable_key)
}

#[inline]
fn env_usize(key: &str) -> Option<usize> {
    match std::env::var(key) {
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(n) if n > 0 => Some(n),
            Ok(_) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "algorithm_runtime_env",
                    format!("env override {key} must be greater than zero; ignoring value"),
                );
                None
            }
            Err(e) => {
                crate::media_conversion_gate::delivery_runtime_batch_audit(
                    "algorithm_runtime_env",
                    format!("failed to parse env override {key}='{value}': {e}; ignoring value"),
                );
                None
            }
        },
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "algorithm_runtime_env",
                format!("failed to read env override {key}: {e}; ignoring value"),
            );
            None
        }
    }
}

#[inline]
fn env_i64_at_least(key: &str, floor: i64) -> i64 {
    let Some(n) = env_usize(key) else {
        return floor;
    };
    match i64::try_from(n) {
        Ok(v) => v.max(floor),
        Err(e) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "algorithm_runtime_env",
                format!("env override {key}={n} does not fit i64: {e}; using floor {floor}"),
            );
            floor
        }
    }
}

/// Strict corpus maturity floors (150/30 loop, 50/20 quality) unless disabled.
#[inline]
fn strict_algorithm_corpus_enabled() -> bool {
    !env_truthy(crate::constants::ENV_DISABLE_STRICT_ALGORITHM_CORPUS)
}

/// Active loop KNN minimum total samples (base or strict floor, optional env
/// raise-only override).
#[must_use]
pub(crate) fn min_gif_samples_total() -> i64 {
    let floor = if strict_algorithm_corpus_enabled() {
        crate::constants::MIN_GIF_SAMPLES_TOTAL_STRICT
    } else {
        crate::constants::MIN_GIF_SAMPLES_TOTAL
    };
    env_i64_at_least(crate::constants::ENV_MIN_GIF_SAMPLES_TOTAL, floor)
}

/// Active loop KNN minimum per-class samples.
#[must_use]
pub(crate) fn min_gif_samples_per_class() -> i64 {
    let floor = if strict_algorithm_corpus_enabled() {
        crate::constants::MIN_GIF_SAMPLES_PER_CLASS_STRICT
    } else {
        crate::constants::MIN_GIF_SAMPLES_PER_CLASS
    };
    env_i64_at_least(crate::constants::ENV_MIN_GIF_SAMPLES_PER_CLASS, floor)
}

/// Active static/scenario quality KNN minimum total samples.
#[must_use]
pub(crate) fn min_quality_samples_total() -> i64 {
    let floor = if strict_algorithm_corpus_enabled() {
        crate::constants::MIN_QUALITY_SAMPLES_TOTAL_STRICT
    } else {
        crate::constants::MIN_QUALITY_SAMPLES_TOTAL
    };
    env_i64_at_least(crate::constants::ENV_MIN_QUALITY_SAMPLES_TOTAL, floor)
}

/// Active static/scenario quality KNN minimum per-class samples.
#[must_use]
pub(crate) fn min_quality_samples_per_class() -> i64 {
    let floor = if strict_algorithm_corpus_enabled() {
        crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS_STRICT
    } else {
        crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS
    };
    env_i64_at_least(crate::constants::ENV_MIN_QUALITY_SAMPLES_PER_CLASS, floor)
}

/// Loop-intent training corpus meets total + per-class floors (M153).
#[must_use]
pub fn loop_corpus_is_mature(total: i64, quality_class: i64, video_class: i64) -> bool {
    total >= min_gif_samples_total()
        && quality_class >= min_gif_samples_per_class()
        && video_class >= min_gif_samples_per_class()
}

/// Non-negative loop samples still needed for maturity messaging.
///
/// Uses `max(total_gap, sum(per_class_gaps))` so over-filled totals still
/// report class shortfalls (fixes db-health showing `Need 0 more` or aborting
/// on bad conversions).
#[must_use]
pub fn loop_corpus_samples_shortfall(total: i64, quality_class: i64, video_class: i64) -> i64 {
    let needed_total = (min_gif_samples_total() - total).max(0);
    let needed_quality = (min_gif_samples_per_class() - quality_class).max(0);
    let needed_video = (min_gif_samples_per_class() - video_class).max(0);
    needed_total.max(needed_quality.saturating_add(needed_video))
}

/// Static/scenario image-quality corpus meets total + per-class floors (M153).
#[must_use]
pub fn quality_corpus_is_mature(high: i64, low: i64) -> bool {
    let total = high + low;
    total >= min_quality_samples_total()
        && high >= min_quality_samples_per_class()
        && low >= min_quality_samples_per_class()
}

/// Non-negative static-quality samples still needed for maturity messaging.
#[must_use]
pub fn quality_corpus_samples_shortfall(high: i64, low: i64) -> i64 {
    let total = high + low;
    let needed_total = (min_quality_samples_total() - total).max(0);
    let needed_high = (min_quality_samples_per_class() - high).max(0);
    let needed_low = (min_quality_samples_per_class() - low).max(0);
    needed_total.max(needed_high.saturating_add(needed_low))
}

/// Loop + static training corpus maturity (M153 / db-health).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrainingCorpusMaturity {
    pub loop_mature: bool,
    pub loop_shortfall: i64,
    pub static_mature: bool,
    pub static_shortfall: i64,
}

impl TrainingCorpusMaturity {
    #[must_use]
    pub const fn overall_mature(self) -> bool {
        self.loop_mature && self.static_mature
    }

    /// User-facing maturity line for `db-health` and drag-and-drop preflight.
    #[must_use]
    pub fn format_db_health_status(self) -> String {
        if self.overall_mature() {
            return "Mature (KNN Active)".to_string();
        }
        let mut parts = Vec::new();
        if !self.loop_mature {
            parts.push(format!("loop needs {} more", self.loop_shortfall.max(0)));
        }
        if !self.static_mature {
            parts.push(format!(
                "static quality needs {} more",
                self.static_shortfall.max(0)
            ));
        }
        if parts.is_empty() {
            "Immature (corpus thresholds not met)".to_string()
        } else {
            format!("Immature ({})", parts.join("; "))
        }
    }
}

/// Evaluate both training corpora with sanitized non-negative counts.
#[must_use]
pub fn evaluate_training_corpus_maturity(
    loop_total: i64,
    loop_quality_class: i64,
    loop_video_class: i64,
    static_high: i64,
    static_low: i64,
) -> TrainingCorpusMaturity {
    let loop_total = loop_total.max(0);
    let loop_quality_class = loop_quality_class.max(0);
    let loop_video_class = loop_video_class.max(0);
    let static_high = static_high.max(0);
    let static_low = static_low.max(0);
    TrainingCorpusMaturity {
        loop_mature: loop_corpus_is_mature(loop_total, loop_quality_class, loop_video_class),
        loop_shortfall: loop_corpus_samples_shortfall(
            loop_total,
            loop_quality_class,
            loop_video_class,
        ),
        static_mature: quality_corpus_is_mature(static_high, static_low),
        static_shortfall: quality_corpus_samples_shortfall(static_high, static_low),
    }
}

/// HDBSCAN cluster fusion into KNN keep probability (default on). Catalog must
/// exist and resolve.
#[must_use]
pub(crate) fn loop_hdbscan_fusion_enabled() -> bool {
    #[cfg(test)]
    {
        algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION)
    }
    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION)
        })
    }
}

/// Minimum in-radius HNSW neighbors before emitting a loop keep posterior
/// (default `2`).
#[must_use]
pub(crate) fn loop_hnsw_min_weighted_neighbors() -> usize {
    #[cfg(test)]
    {
        crate::media_conversion_gate::algorithm_env_usize_or_default(
            crate::constants::ENV_LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS,
            crate::constants::LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS,
            "loop_hnsw_min_weighted_neighbors",
        )
    }
    #[cfg(not(test))]
    {
        static MIN: OnceLock<usize> = OnceLock::new();
        *MIN.get_or_init(|| {
            crate::media_conversion_gate::algorithm_env_usize_or_default(
                crate::constants::ENV_LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS,
                crate::constants::LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS,
                "loop_hnsw_min_weighted_neighbors",
            )
        })
    }
}

/// KNN-vs-model disagreement fusion (default on;
/// [`ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD`] kills all).
#[must_use]
pub(crate) fn quality_knn_disagreement_guard_enabled(pipeline: &str) -> bool {
    let _ = pipeline;
    !env_truthy(crate::constants::ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD)
}

/// Corrupt loop `feature_stats` bootstrap via env (removed — always
/// fail-closed).
#[cfg(test)]
#[must_use]
pub(crate) const fn loop_feature_stats_fail_open_on_parse_error() -> bool {
    false
}

/// Inference log writes for static quality when the quality DB stack is fully
/// disabled.
#[must_use]
pub(crate) fn static_quality_inference_logging_enabled() -> bool {
    quality_db_stack_globally_enabled() && quality_inference_log_heuristic_fallbacks_enabled()
}

/// Immature/fallback heuristic paths may insert into quality `inference_log`
/// tables only after both explicit heuristic opt-ins are enabled.
#[must_use]
pub(crate) fn quality_inference_log_heuristic_fallbacks_enabled() -> bool {
    image_quality_heuristic_enabled()
        && env_truthy(crate::constants::QUALITY_INFERENCE_HEURISTIC_LOG_ENV_KEY)
        && algorithm_gate_enabled(crate::constants::ENV_DISABLE_QUALITY_INFERENCE_HEURISTIC_LOGS)
}

#[inline]
fn quality_db_stack_globally_enabled() -> bool {
    image_quality_heuristic_enabled()
        && !env_truthy(crate::constants::ENV_DISABLE_DB_FEEDBACK)
        && !env_truthy(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB)
}

/// Fuse DB quality scores into detection outputs when heuristic quality is
/// explicitly enabled.
#[must_use]
pub(crate) fn quality_db_fusion_enabled(pipeline: &str) -> bool {
    if !quality_db_stack_globally_enabled() {
        return false;
    }
    match pipeline {
        "video_detection" | "video_quality_detector" | "image_detection_animated" => {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_SCENARIO_QUALITY_DB_FUSION)
        }
        "image_detection_static" => {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_FUSION)
        }
        _ => false,
    }
}

/// Runtime Postgres quality lookup when heuristic quality is explicitly
/// enabled; fusion-on pipelines always allow lookup.
#[must_use]
pub(crate) fn quality_db_lookup_enabled(pipeline: &str) -> bool {
    if !quality_db_stack_globally_enabled() {
        return false;
    }
    if quality_db_fusion_enabled(pipeline) {
        return true;
    }
    match pipeline {
        "img_lossless_convert" | "image_detection_static" => {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP)
                || env_truthy(crate::constants::ENV_FORCE_QUALITY_KNN)
        }
        "video_detection" | "video_quality_detector" | "image_detection_animated" => {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_SCENARIO_QUALITY_DB_LOOKUP)
        }
        _ => false,
    }
}

/// `img` convert path: forensic quality log via static DB (no score fusion at
/// convert time).
#[must_use]
pub fn static_quality_db_lookup_enabled() -> bool {
    quality_db_lookup_enabled("img_lossless_convert")
}

/// Returns `true` when the image quality heuristic score is explicitly enabled.
#[must_use]
pub fn image_quality_heuristic_enabled() -> bool {
    env_truthy(crate::constants::HEURISTIC_QUALITY_ENV_KEY)
}

/// Exploration/matcher output sealing before CRF and quality gates (default
/// on).
#[must_use]
pub(crate) fn exploration_algorithm_seal_enabled() -> bool {
    #[cfg(test)]
    {
        algorithm_gate_enabled(crate::constants::ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL)
    }
    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL)
        })
    }
}

/// Layer 6 HNSW when the tree is uncertain (explicit opt-in).
#[must_use]
pub(crate) fn loop_intent_layer6_knn_enabled() -> bool {
    env_truthy(crate::constants::LOOP_INTENT_LAYER6_KNN_OPT_IN_ENV_KEY)
        && algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_INTENT_LAYER6_KNN)
}

/// Strict exploration delivery (default on). Disable via
/// [`crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION`].
#[must_use]
pub(crate) fn strict_media_conversion_delivery_enabled() -> bool {
    algorithm_gate_enabled(crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION)
}

/// Reject `quality_passed` exploration when overall confidence is missing or
/// below floor (default on).
#[must_use]
pub(crate) fn exploration_confidence_gate_enabled() -> bool {
    algorithm_gate_enabled(crate::constants::ENV_DISABLE_EXPLORATION_CONFIDENCE_GATE)
}

/// `quality_passed` requires `ExploreResult.ssim` to be present (default on).
#[must_use]
pub(crate) fn exploration_ssim_presence_gate_enabled() -> bool {
    algorithm_gate_enabled(crate::constants::ENV_DISABLE_EXPLORATION_SSIM_PRESENCE_GATE)
}

/// `quality_passed` requires measured SSIM ≥ `ExploreResult.actual_min_ssim`
/// (default on).
#[must_use]
pub(crate) fn exploration_ssim_threshold_gate_enabled() -> bool {
    algorithm_gate_enabled(crate::constants::ENV_DISABLE_EXPLORATION_SSIM_THRESHOLD_GATE)
}

/// `quality_passed` cannot coexist with an explicit `size_target_met` failure
/// (default on).
#[must_use]
pub(crate) fn exploration_size_target_gate_enabled() -> bool {
    algorithm_gate_enabled(crate::constants::ENV_DISABLE_EXPLORATION_SIZE_TARGET_GATE)
}

/// Persist loop `inference_log` rows (default on; requires DB feedback not
/// globally disabled).
#[must_use]
pub(crate) fn loop_inference_telemetry_enabled() -> bool {
    if env_truthy(crate::constants::ENV_DISABLE_DB_FEEDBACK) {
        return false;
    }
    #[cfg(test)]
    {
        algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG)
    }
    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG)
        })
    }
}

/// Audit-only quality inference rows (default on): `final_verdict` is
/// [`crate::constants::INFERENCE_TELEMETRY_ONLY_VERDICT`].
#[must_use]
pub(crate) fn quality_inference_audit_only_mode() -> bool {
    algorithm_gate_enabled(crate::constants::ENV_DISABLE_QUALITY_INFERENCE_AUDIT_ONLY)
}

/// Audit-only `inference_log` rows (default on): `final_verdict` is
/// [`crate::constants::LOOP_INFERENCE_TELEMETRY_ONLY_VERDICT`].
#[must_use]
pub(crate) fn loop_inference_audit_only_mode() -> bool {
    if !loop_inference_telemetry_enabled() {
        return false;
    }
    env_truthy(crate::constants::ENV_LOOP_INTENT_INFERENCE_AUDIT_ONLY)
        || algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_AUDIT_ONLY)
}

/// [`crate::image_quality_db::QualityScore::sealed`] and related inference
/// field mutation (default on).
#[must_use]
pub(crate) fn quality_algorithm_seal_enabled() -> bool {
    #[cfg(test)]
    {
        algorithm_gate_enabled(crate::constants::ENV_DISABLE_QUALITY_ALGORITHM_SEAL)
    }
    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_QUALITY_ALGORITHM_SEAL)
        })
    }
}

/// Loop inference telemetry field sealing (default on).
#[must_use]
pub(crate) fn loop_intent_algorithm_seal_enabled() -> bool {
    #[cfg(test)]
    {
        algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_INTENT_ALGORITHM_SEAL)
    }
    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            algorithm_gate_enabled(crate::constants::ENV_DISABLE_LOOP_INTENT_ALGORITHM_SEAL)
        })
    }
}

/// Scenario (animated/video) tables use the same feedback + heuristic-log gates
/// as static.
#[must_use]
pub(crate) fn scenario_quality_inference_logging_enabled(
    branch_logs_heuristic_fallback: bool,
) -> bool {
    static_quality_inference_logging_enabled() && branch_logs_heuristic_fallback
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common_utils::EnvGuard;
    use serial_test::serial;

    #[test]
    #[serial]
    fn hdbscan_fusion_on_by_default() {
        let _guard = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION, "0");
        assert!(loop_hdbscan_fusion_enabled());
    }

    #[test]
    #[serial]
    fn hdbscan_fusion_disable_kill_switch() {
        let _guard = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_HDBSCAN_FUSION, "1");
        assert!(!loop_hdbscan_fusion_enabled());
    }

    #[test]
    #[serial]
    fn knn_disagreement_guard_on_by_default() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD,
            "0",
        );
        assert!(quality_knn_disagreement_guard_enabled("video_quality_db"));
        assert!(quality_knn_disagreement_guard_enabled(
            "static_image_quality_lgbm"
        ));
    }

    #[test]
    #[serial]
    fn knn_disagreement_guard_disable_kill_switch() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_KNN_DISAGREE_GUARD,
            "1",
        );
        assert!(!quality_knn_disagreement_guard_enabled(
            "static_image_quality_db"
        ));
    }

    #[test]
    #[serial]
    fn loop_hnsw_min_neighbors_default_two() {
        let _guard = EnvGuard::set(crate::constants::ENV_LOOP_HNSW_MIN_WEIGHTED_NEIGHBORS, "0");
        assert_eq!(loop_hnsw_min_weighted_neighbors(), 2);
    }

    #[test]
    #[serial]
    fn static_quality_logging_off_when_quality_db_disabled() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _logs = EnvGuard::set(
            crate::constants::QUALITY_INFERENCE_HEURISTIC_LOG_ENV_KEY,
            "1",
        );
        let _feedback = EnvGuard::set(crate::constants::ENV_DISABLE_DB_FEEDBACK, "0");
        let _db = EnvGuard::set(crate::constants::ENV_DISABLE_IMAGE_QUALITY_DB, "1");
        assert!(!static_quality_inference_logging_enabled());
    }

    #[test]
    #[serial]
    fn scenario_quality_db_fusion_requires_explicit_heuristic_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "0");
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_SCENARIO_QUALITY_DB_FUSION,
            "0",
        );
        assert!(!quality_db_fusion_enabled("video_detection"));
    }

    #[test]
    #[serial]
    fn scenario_quality_db_fusion_enabled_after_explicit_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_SCENARIO_QUALITY_DB_FUSION,
            "0",
        );
        assert!(quality_db_fusion_enabled("video_detection"));
    }

    #[test]
    #[serial]
    fn scenario_quality_db_fusion_disable_kill_switch() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_SCENARIO_QUALITY_DB_FUSION,
            "1",
        );
        assert!(!quality_db_fusion_enabled("video_detection"));
    }

    #[test]
    #[serial]
    fn static_quality_db_fusion_requires_explicit_heuristic_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "0");
        let _guard = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_FUSION, "0");
        assert!(!quality_db_fusion_enabled("image_detection_static"));
    }

    #[test]
    #[serial]
    fn static_quality_db_fusion_enabled_after_explicit_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _guard = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_FUSION, "0");
        assert!(quality_db_fusion_enabled("image_detection_static"));
    }

    #[test]
    #[serial]
    fn static_quality_db_fusion_disable_kill_switch() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _guard = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_FUSION, "1");
        assert!(!quality_db_fusion_enabled("image_detection_static"));
    }

    #[test]
    #[serial]
    fn static_quality_db_lookup_requires_explicit_heuristic_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "0");
        let _lookup = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP, "0");
        let _fusion = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_FUSION, "0");
        let _force = EnvGuard::set(crate::constants::ENV_FORCE_QUALITY_KNN, "0");
        assert!(!static_quality_db_lookup_enabled());
    }

    #[test]
    #[serial]
    fn static_quality_db_lookup_enabled_after_explicit_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _lookup = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP, "0");
        let _fusion = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_FUSION, "0");
        let _force = EnvGuard::set(crate::constants::ENV_FORCE_QUALITY_KNN, "0");
        assert!(static_quality_db_lookup_enabled());
    }

    #[test]
    #[serial]
    fn static_quality_db_lookup_disable_kill_switch() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _lookup = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP, "1");
        let _fusion = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_FUSION, "1");
        let _force = EnvGuard::set(crate::constants::ENV_FORCE_QUALITY_KNN, "0");
        assert!(!static_quality_db_lookup_enabled());
    }

    #[test]
    #[serial]
    fn static_quality_db_lookup_force_knn_does_not_override_heuristic_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "0");
        let _lookup = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP, "1");
        let _force = EnvGuard::set(crate::constants::ENV_FORCE_QUALITY_KNN, "1");
        assert!(!static_quality_db_lookup_enabled());
    }

    #[test]
    #[serial]
    fn static_quality_db_lookup_force_knn_after_explicit_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _lookup = EnvGuard::set(crate::constants::ENV_DISABLE_STATIC_QUALITY_DB_LOOKUP, "1");
        let _force = EnvGuard::set(crate::constants::ENV_FORCE_QUALITY_KNN, "1");
        assert!(static_quality_db_lookup_enabled());
    }

    #[test]
    #[serial]
    fn exploration_seal_on_by_default() {
        let _disable = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL,
            "0",
        );
        assert!(exploration_algorithm_seal_enabled());
    }

    #[test]
    #[serial]
    fn exploration_seal_disable_kill_switch() {
        let _disable = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_ALGORITHM_SEAL,
            "1",
        );
        assert!(!exploration_algorithm_seal_enabled());
    }

    #[test]
    #[serial]
    fn loop_intent_layer6_knn_off_by_default() {
        let _opt_in = EnvGuard::set(crate::constants::LOOP_INTENT_LAYER6_KNN_OPT_IN_ENV_KEY, "0");
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_LAYER6_KNN, "0");
        assert!(!loop_intent_layer6_knn_enabled());
    }

    #[test]
    #[serial]
    fn loop_intent_layer6_knn_requires_explicit_opt_in() {
        let _opt_in = EnvGuard::set(crate::constants::LOOP_INTENT_LAYER6_KNN_OPT_IN_ENV_KEY, "1");
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_LAYER6_KNN, "0");
        assert!(loop_intent_layer6_knn_enabled());
    }

    #[test]
    #[serial]
    fn loop_intent_layer6_knn_disable_kill_switch() {
        let _opt_in = EnvGuard::set(crate::constants::LOOP_INTENT_LAYER6_KNN_OPT_IN_ENV_KEY, "1");
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_LAYER6_KNN, "1");
        assert!(!loop_intent_layer6_knn_enabled());
    }

    #[test]
    #[serial]
    fn quality_algorithm_seal_on_by_default() {
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_QUALITY_ALGORITHM_SEAL, "0");
        assert!(quality_algorithm_seal_enabled());
        assert!(crate::algorithm_seal::quality_unit_probability(f64::NAN).is_none());
        assert_eq!(
            crate::algorithm_seal::quality_unit_probability(1.5),
            Some(1.0)
        );
    }

    #[test]
    #[serial]
    fn quality_algorithm_seal_disable_kill_switch() {
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_QUALITY_ALGORITHM_SEAL, "1");
        assert!(!quality_algorithm_seal_enabled());
    }

    #[test]
    #[serial]
    fn loop_intent_algorithm_seal_on_by_default() {
        let _disable = EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_INTENT_ALGORITHM_SEAL,
            "0",
        );
        assert!(loop_intent_algorithm_seal_enabled());
        assert!(crate::algorithm_seal::loop_unit_probability(f64::NAN).is_none());
        assert_eq!(crate::algorithm_seal::loop_unit_probability(1.5), Some(1.0));
    }

    #[test]
    #[serial]
    fn loop_intent_algorithm_seal_disable_kill_switch() {
        let _disable = EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_INTENT_ALGORITHM_SEAL,
            "1",
        );
        assert!(!loop_intent_algorithm_seal_enabled());
    }

    #[test]
    #[serial]
    fn loop_feature_stats_fail_open_off_by_default() {
        let _open = EnvGuard::set(crate::constants::ENV_LOOP_FEATURE_STATS_FAIL_OPEN, "0");
        let _kill = EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_FEATURE_STATS_FAIL_OPEN,
            "0",
        );
        assert!(!loop_feature_stats_fail_open_on_parse_error());
    }

    #[test]
    #[serial]
    fn loop_feature_stats_fail_open_env_ignored_always_fail_closed() {
        let _open = EnvGuard::set(crate::constants::ENV_LOOP_FEATURE_STATS_FAIL_OPEN, "1");
        let _kill = EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_FEATURE_STATS_FAIL_OPEN,
            "0",
        );
        assert!(!loop_feature_stats_fail_open_on_parse_error());
    }

    #[test]
    #[serial]
    fn loop_inference_log_on_by_default() {
        let _log = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG, "0");
        let _feedback = EnvGuard::set(crate::constants::ENV_DISABLE_DB_FEEDBACK, "0");
        assert!(loop_inference_telemetry_enabled());
    }

    #[test]
    #[serial]
    fn loop_inference_log_disable_kill_switch() {
        let _log = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG, "1");
        let _feedback = EnvGuard::set(crate::constants::ENV_DISABLE_DB_FEEDBACK, "0");
        assert!(!loop_inference_telemetry_enabled());
    }

    #[test]
    #[serial]
    fn loop_inference_log_off_when_db_feedback_disabled() {
        let _log = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG, "0");
        let _feedback = EnvGuard::set(crate::constants::ENV_DISABLE_DB_FEEDBACK, "1");
        assert!(!loop_inference_telemetry_enabled());
    }

    #[test]
    #[serial]
    fn loop_inference_audit_only_on_by_default() {
        let _log = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG, "0");
        let _audit = EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_AUDIT_ONLY,
            "0",
        );
        let _feedback = EnvGuard::set(crate::constants::ENV_DISABLE_DB_FEEDBACK, "0");
        assert!(loop_inference_audit_only_mode());
    }

    #[test]
    #[serial]
    fn strict_media_conversion_on_by_default() {
        let _guard = EnvGuard::set(crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION, "0");
        assert!(strict_media_conversion_delivery_enabled());
    }

    #[test]
    #[serial]
    fn strict_media_conversion_disable_kill_switch() {
        let _guard = EnvGuard::set(crate::constants::ENV_DISABLE_STRICT_MEDIA_CONVERSION, "1");
        assert!(!strict_media_conversion_delivery_enabled());
    }

    #[test]
    #[serial]
    fn exploration_confidence_gate_on_by_default() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_CONFIDENCE_GATE,
            "0",
        );
        assert!(exploration_confidence_gate_enabled());
    }

    #[test]
    #[serial]
    fn exploration_ssim_presence_gate_on_by_default() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_SSIM_PRESENCE_GATE,
            "0",
        );
        assert!(exploration_ssim_presence_gate_enabled());
    }

    #[test]
    #[serial]
    fn exploration_ssim_threshold_gate_on_by_default() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_SSIM_THRESHOLD_GATE,
            "0",
        );
        assert!(exploration_ssim_threshold_gate_enabled());
    }

    #[test]
    #[serial]
    fn exploration_ssim_presence_gate_disable_kill_switch() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_SSIM_PRESENCE_GATE,
            "1",
        );
        assert!(!exploration_ssim_presence_gate_enabled());
    }

    #[test]
    #[serial]
    fn exploration_confidence_gate_disable_kill_switch() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_CONFIDENCE_GATE,
            "1",
        );
        assert!(!exploration_confidence_gate_enabled());
    }

    #[test]
    #[serial]
    fn loop_inference_audit_only_disable_writes_runtime_verdict() {
        let _log = EnvGuard::set(crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_LOG, "0");
        let _audit = EnvGuard::set(
            crate::constants::ENV_DISABLE_LOOP_INTENT_INFERENCE_AUDIT_ONLY,
            "1",
        );
        let _feedback = EnvGuard::set(crate::constants::ENV_DISABLE_DB_FEEDBACK, "0");
        assert!(!loop_inference_audit_only_mode());
    }

    #[test]
    #[serial]
    fn quality_inference_audit_only_on_by_default() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_INFERENCE_AUDIT_ONLY,
            "0",
        );
        assert!(quality_inference_audit_only_mode());
    }

    #[test]
    #[serial]
    fn quality_inference_audit_only_disable_writes_runtime_verdict() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_INFERENCE_AUDIT_ONLY,
            "1",
        );
        assert!(!quality_inference_audit_only_mode());
    }

    #[test]
    #[serial]
    fn heuristic_inference_logs_off_by_default() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "0");
        let _enable_logs = EnvGuard::set(
            crate::constants::QUALITY_INFERENCE_HEURISTIC_LOG_ENV_KEY,
            "1",
        );
        let _disable_logs = EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_INFERENCE_HEURISTIC_LOGS,
            "0",
        );
        assert!(!quality_inference_log_heuristic_fallbacks_enabled());
        assert!(!scenario_quality_inference_logging_enabled(false));
        assert!(!scenario_quality_inference_logging_enabled(true));
    }

    #[test]
    #[serial]
    fn heuristic_inference_logs_require_explicit_opt_in() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _enable_logs = EnvGuard::set(
            crate::constants::QUALITY_INFERENCE_HEURISTIC_LOG_ENV_KEY,
            "1",
        );
        let _disable_logs = EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_INFERENCE_HEURISTIC_LOGS,
            "0",
        );
        assert!(quality_inference_log_heuristic_fallbacks_enabled());
        assert!(!scenario_quality_inference_logging_enabled(false));
        assert!(scenario_quality_inference_logging_enabled(true));
    }

    #[test]
    #[serial]
    fn heuristic_inference_logs_disable_kill_switch() {
        let _heuristic = EnvGuard::set(crate::constants::HEURISTIC_QUALITY_ENV_KEY, "1");
        let _enable_logs = EnvGuard::set(
            crate::constants::QUALITY_INFERENCE_HEURISTIC_LOG_ENV_KEY,
            "1",
        );
        let _disable_logs = EnvGuard::set(
            crate::constants::ENV_DISABLE_QUALITY_INFERENCE_HEURISTIC_LOGS,
            "1",
        );
        assert!(!quality_inference_log_heuristic_fallbacks_enabled());
        assert!(!scenario_quality_inference_logging_enabled(false));
    }

    #[test]
    #[serial]
    fn strict_corpus_default_on_without_disable() {
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_STRICT_ALGORITHM_CORPUS, "0");
        assert_eq!(
            min_gif_samples_total(),
            crate::constants::MIN_GIF_SAMPLES_TOTAL_STRICT
        );
        assert_eq!(
            min_gif_samples_per_class(),
            crate::constants::MIN_GIF_SAMPLES_PER_CLASS_STRICT
        );
        assert_eq!(
            min_quality_samples_total(),
            crate::constants::MIN_QUALITY_SAMPLES_TOTAL_STRICT
        );
        assert_eq!(
            min_quality_samples_per_class(),
            crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS_STRICT
        );
    }

    #[test]
    #[serial]
    fn strict_corpus_disable_kill_switch_relaxes_floors() {
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_STRICT_ALGORITHM_CORPUS, "1");
        assert_eq!(
            min_gif_samples_total(),
            crate::constants::MIN_GIF_SAMPLES_TOTAL
        );
        assert_eq!(
            min_quality_samples_total(),
            crate::constants::MIN_QUALITY_SAMPLES_TOTAL
        );
        assert_eq!(
            min_quality_samples_per_class(),
            crate::constants::MIN_QUALITY_SAMPLES_PER_CLASS
        );
    }

    #[test]
    fn loop_corpus_samples_shortfall_counts_per_class_when_total_is_overfilled() {
        let min_total = min_gif_samples_total();
        let min_per = min_gif_samples_per_class();
        let shortfall = loop_corpus_samples_shortfall(min_total + 50, min_per - 3, min_per);
        assert_eq!(
            shortfall, 3,
            "quality class gap must surface even when total exceeds floor"
        );
    }

    #[test]
    fn training_corpus_maturity_status_reports_both_shortfalls() {
        let min_loop = min_gif_samples_per_class();
        let min_static = min_quality_samples_per_class();
        let min_static_total = min_quality_samples_total();
        let loop_total = min_gif_samples_total() + 100;
        let static_high = min_static - 1;
        let static_low = min_static_total - static_high;
        let maturity = evaluate_training_corpus_maturity(
            loop_total,
            min_loop - 2,
            min_loop,
            static_high,
            static_low,
        );
        assert!(!maturity.overall_mature());
        assert_eq!(maturity.loop_shortfall, 2);
        assert_eq!(
            maturity.static_shortfall,
            quality_corpus_samples_shortfall(static_high, static_low)
        );
        let status = maturity.format_db_health_status();
        assert!(status.contains("loop needs 2 more"));
        assert!(status.contains("static quality needs"));
    }

    #[test]
    fn loop_corpus_samples_shortfall_never_negative() {
        let shortfall = loop_corpus_samples_shortfall(10_000, 10_000, 10_000);
        assert_eq!(shortfall, 0);
    }

    #[test]
    fn quality_corpus_samples_shortfall_never_negative_when_overfilled() {
        assert_eq!(quality_corpus_samples_shortfall(10_000, 10_000), 0);
    }

    #[test]
    fn quality_corpus_samples_shortfall_includes_total_and_class_gaps() {
        let min_total = min_quality_samples_total();
        let min_per = min_quality_samples_per_class();
        let low = min_per - 2;
        let high = min_total - low;
        assert_eq!(high + low, min_total);
        assert_eq!(quality_corpus_samples_shortfall(high, low), 2);
        let mature_low = min_per;
        let mature_high = min_total - mature_low;
        assert!(mature_high >= min_per);
        assert_eq!(quality_corpus_samples_shortfall(mature_high, mature_low), 0);
    }

    #[test]
    #[serial]
    fn min_gif_samples_env_override_only_raises() {
        let _disable = EnvGuard::set(crate::constants::ENV_DISABLE_STRICT_ALGORITHM_CORPUS, "1");
        let _total = EnvGuard::set(crate::constants::ENV_MIN_GIF_SAMPLES_TOTAL, "5");
        assert_eq!(
            min_gif_samples_total(),
            crate::constants::MIN_GIF_SAMPLES_TOTAL
        );
        let _total = EnvGuard::set(crate::constants::ENV_MIN_GIF_SAMPLES_TOTAL, "200");
        assert_eq!(min_gif_samples_total(), 200);
    }

    #[test]
    #[serial]
    fn exploration_size_target_gate_on_by_default() {
        let _guard = EnvGuard::set(
            crate::constants::ENV_DISABLE_EXPLORATION_SIZE_TARGET_GATE,
            "0",
        );
        assert!(exploration_size_target_gate_enabled());
    }
}

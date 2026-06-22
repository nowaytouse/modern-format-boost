//! Terminal contract for algorithm-layer probabilities.
//!
//! **Strict:** non-finite values always become [`None`]. Forbidden at call sites:
//! `unwrap_or` / `unwrap_or_else` / `map_or` / `map_or_else` / `Option::or` that
//! substitute numeric defaults (0.0, 0.5, 1.0, `DEFAULT_*`, `knn_kp`, etc.).
//! Use `if let Some` / `match` / `?` and skip the branch when seal returns `None`.
//!
//! When a runtime seal gate is **off**, finite values pass through without unit clamp;
//! when **on**, probabilities clamp to `[0, 1]`.

/// Reject non-finite values; clamp finite values into the unit interval.
#[must_use]
pub(crate) const fn seal_unit_probability(value: f64) -> Option<f64> {
    if value.is_finite() {
        Some(value.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// Any finite scalar (log-odds, distances without an upper bound, etc.).
#[must_use]
pub(crate) fn seal_finite_scalar(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

/// Optional finite distance-style metric.
#[must_use]
pub(crate) fn seal_optional_non_negative_distance(value: Option<f64>) -> Option<f64> {
    value.and_then(seal_non_negative_finite)
}

/// Seal a quality-regression or loop-keep pair before logging or downstream fusion.
#[must_use]
pub(crate) fn seal_probability_pair(score: f64, confidence: f64) -> Option<(f64, f64)> {
    let score = seal_unit_probability(score)?;
    let confidence = seal_unit_probability(confidence)?;
    Some((score, confidence))
}

/// Non-negative finite metric (BPP, bitrate-derived ratios, etc.).
#[must_use]
pub(crate) fn seal_non_negative_finite(value: f64) -> Option<f64> {
    if value.is_finite() && value >= 0.0 {
        Some(value)
    } else {
        None
    }
}

/// Display quality score on the 0–100 scale used by video routing heuristics.
#[must_use]
pub(crate) const fn seal_u8_quality_display(value: u8) -> u8 {
    if value > 100 { 100 } else { value }
}

/// CRF setpoint guard (0–51 covers H.264/HEVC/AV1 practical range).
#[must_use]
pub(crate) const fn seal_u8_crf_setpoint(value: u8) -> u8 {
    if value > 51 { 51 } else { value }
}

/// Optional metric: drop non-finite values instead of propagating NaN into gates.
#[must_use]
pub(crate) fn seal_optional_unit_metric(value: Option<f64>) -> Option<f64> {
    value.and_then(seal_unit_probability)
}

/// JXL distance: reject non-finite; clamp finite values to the legal range.
#[must_use]
pub(crate) fn seal_jxl_distance(value: f32) -> Option<f32> {
    if !value.is_finite() {
        tracing::warn!(
            target: "mfb.algorithm",
            pipeline = "jxl_exploration",
            branch = "jxl_distance_non_finite_rejected",
            value,
            "non-finite JXL distance rejected (not substituted with floor)"
        );
        return None;
    }
    Some(value.clamp(
        crate::constants::JXL_MIN_DISTANCE,
        crate::constants::JXL_MAX_DISTANCE,
    ))
}

/// Non-negative finite ratio (e.g. initial output / input size).
#[must_use]
pub(crate) fn seal_non_negative_ratio(value: f64) -> Option<f64> {
    seal_non_negative_finite(value)
}

fn reject_loop_non_finite(field: &'static str, value: f64) {
    tracing::warn!(
        target: "mfb.algorithm",
        pipeline = "loop_intent",
        branch = "loop_non_finite_rejected",
        field,
        value,
        "loop contract rejected non-finite value"
    );
}

/// Loop tree / Layer 6 unit probability.
#[must_use]
pub(crate) fn loop_unit_probability(value: f64) -> Option<f64> {
    if !value.is_finite() {
        reject_loop_non_finite("unit_probability", value);
        return None;
    }
    seal_unit_probability(value)
}

/// Loop log-odds and unbounded scalars.
#[must_use]
pub(crate) fn loop_finite_scalar(value: f64) -> Option<f64> {
    if !value.is_finite() {
        reject_loop_non_finite("finite_scalar", value);
        return None;
    }
    if crate::algorithm_runtime::loop_intent_algorithm_seal_enabled() {
        seal_finite_scalar(value)
    } else {
        Some(value)
    }
}

/// Loop KNN keep + confidence pair before returning `SampleMatch`.
#[must_use]
pub(crate) fn loop_seal_probability_pair(score: f64, confidence: f64) -> Option<(f64, f64)> {
    if !score.is_finite() || !confidence.is_finite() {
        reject_loop_non_finite("keep_probability", score);
        reject_loop_non_finite("confidence", confidence);
        return None;
    }
    seal_probability_pair(score, confidence)
}

fn reject_quality_non_finite(field: &'static str, value: f64) {
    tracing::warn!(
        target: "mfb.algorithm",
        pipeline = "quality_score",
        branch = "quality_non_finite_rejected",
        field,
        value,
        "quality contract rejected non-finite value"
    );
}

/// Static/scenario quality unit probability.
#[must_use]
pub(crate) fn quality_unit_probability(value: f64) -> Option<f64> {
    if !value.is_finite() {
        reject_quality_non_finite("unit_probability", value);
        return None;
    }
    seal_unit_probability(value)
}

/// Quality score + confidence before exposing `QualityScore` or writing inference logs.
#[must_use]
pub(crate) fn quality_probability_pair(score: f64, confidence: f64) -> Option<(f64, f64)> {
    if !score.is_finite() || !confidence.is_finite() {
        reject_quality_non_finite("score", score);
        reject_quality_non_finite("confidence", confidence);
        return None;
    }
    seal_probability_pair(score, confidence)
}

/// Quality feature scalars (BPP, regression features, heuristic intermediates).
#[must_use]
pub(crate) fn quality_finite_scalar(value: f64) -> Option<f64> {
    if !value.is_finite() {
        reject_quality_non_finite("finite_scalar", value);
        return None;
    }
    if crate::algorithm_runtime::quality_algorithm_seal_enabled() {
        seal_finite_scalar(value)
    } else {
        Some(value)
    }
}

fn reject_exploration_non_finite(field: &'static str, value: f64) {
    tracing::warn!(
        target: "mfb.algorithm",
        pipeline = "video_exploration",
        branch = "explore_non_finite_rejected",
        field,
        value,
        "exploration contract rejected non-finite value"
    );
}

/// Video explore / confidence detail overall (opt-in exploration seal).
#[must_use]
pub(crate) fn exploration_unit_probability(value: f64) -> Option<f64> {
    if !value.is_finite() {
        reject_exploration_non_finite("unit_probability", value);
        return None;
    }
    seal_unit_probability(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_rejects_nan() {
        assert!(seal_unit_probability(f64::NAN).is_none());
    }

    #[test]
    fn seal_clamps_high() {
        assert_eq!(seal_unit_probability(1.5), Some(1.0));
    }

    #[test]
    fn seal_finite_scalar_rejects_nan() {
        assert_eq!(seal_finite_scalar(f64::NAN), None);
    }

    #[test]
    fn jxl_distance_rejects_nan() {
        assert!(seal_jxl_distance(f32::NAN).is_none());
    }

    #[test]
    fn jxl_distance_clamps_out_of_range() {
        assert_eq!(
            seal_jxl_distance(999.0),
            Some(crate::constants::JXL_MAX_DISTANCE)
        );
    }
}

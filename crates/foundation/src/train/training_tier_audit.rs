//! Static-image training tier rules — must stay aligned with
//! `crates/dev/src/config/training_rules.json` → `static_image.{high,low}_quality}.rules`.
//!
//! Collection (`run_training.py`) and ingest (`analyze_image`) both use this module so
//! entropy/geometry tier decisions match the values stored in `image_quality_samples`.
//! Training C-API probes are gated by [`crate::training_entry_guard`].

use crate::image_analyzer::{ImageAnalysis, analyze_image};
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::path::Path;

/// `high_quality` tier: logic **ANY** — `entropy_ge` **or** guarded `pixel_min_dim_ge` (social stills).
pub const HIGH_ENTROPY_GE: f64 = 7.7;
pub const HIGH_PIXEL_MIN_DIM_GE: u32 = 1080;
/// Corpus ingest ceiling: neither dimension may exceed 4K (training DB hard limit).
pub const STATIC_CORPUS_MAX_PIXEL_DIM: u32 = 4096;
/// Short-side ≥1080 alone cannot mark high below this entropy (blocks flat plates in dead band).
pub const HIGH_DIMENSION_ENTROPY_FLOOR: f64 = 5.5;

/// `low_quality` tier: logic **ANY** — `entropy_le` **or** guarded `pixel_max_dim_le` (one hit enough).
pub const LOW_ENTROPY_LE: f64 = 2.8;
pub const LOW_PIXEL_MAX_DIM_LE: u32 = 512;
/// ≤512px alone cannot mark low above this entropy (blocks sharp midsize thumbs).
pub const LOW_DIMENSION_ENTROPY_CEIL: f64 = 5.5;

/// Open interval `(DEAD_ZONE_LO, DEAD_ZONE_HI)`: no tier inside the band even if rules partially match.
pub const TIER_ENTROPY_DEAD_ZONE_LO: f64 = LOW_ENTROPY_LE;
pub const TIER_ENTROPY_DEAD_ZONE_HI: f64 = HIGH_ENTROPY_GE;

pub const HIGH_TIER_RULE_COUNT: usize = 2;
pub const LOW_TIER_RULE_COUNT: usize = 2;

pub const TRAINING_TIER_AUDIT_SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierRuleLogic {
    Any,
    All,
}

impl TierRuleLogic {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Any => "ANY",
            Self::All => "ALL",
        }
    }

    #[must_use]
    pub const fn combine(self, hit_count: usize, rule_count: usize) -> bool {
        match self {
            Self::Any => hit_count > 0,
            Self::All => hit_count >= rule_count && rule_count > 0,
        }
    }
}

/// Committed combiners (must match `training_rules.json` `static_image.*.logic`).
pub const HIGH_TIER_LOGIC: TierRuleLogic = TierRuleLogic::Any;
pub const LOW_TIER_LOGIC: TierRuleLogic = TierRuleLogic::Any;

/// Production collect/ingest/C-API policy — always **exclude**; not overridden by env.
/// Python may set `MFB_TIER_AMBIGUOUS_POLICY` for documentation/audit only; Rust never reads it.
pub const COMMITTED_TIER_AMBIGUOUS_POLICY: TierAmbiguousPolicy = TierAmbiguousPolicy::Exclude;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssignedTrainingTier {
    High,
    Low,
}

impl AssignedTrainingTier {
    #[must_use]
    pub fn parse_label(label: &str) -> Option<Self> {
        let normalized = label.trim().to_ascii_lowercase();
        if normalized == "high"
            || normalized.ends_with("-high")
            || normalized == "png-high"
            || normalized == "modern-high"
        {
            Some(Self::High)
        } else if normalized == "low"
            || normalized.ends_with("-low")
            || normalized == "png-low"
            || normalized == "modern-low"
        {
            Some(Self::Low)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StaticTierEvaluation {
    pub high_tier: bool,
    pub low_tier: bool,
    pub high_rule_hits: Vec<&'static str>,
    pub low_rule_hits: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaticStillProbe {
    pub width: u32,
    pub height: u32,
    pub entropy: f64,
    pub format: String,
    pub tier: StaticTierEvaluation,
}

/// Evaluate committed JSON tier rules on scalar geometry + entropy.
#[must_use]
pub fn evaluate_static_tier(entropy: f64, width: u32, height: u32) -> StaticTierEvaluation {
    if width > STATIC_CORPUS_MAX_PIXEL_DIM || height > STATIC_CORPUS_MAX_PIXEL_DIM {
        return StaticTierEvaluation {
            high_tier: false,
            low_tier: false,
            high_rule_hits: Vec::new(),
            low_rule_hits: Vec::new(),
        };
    }

    let min_dim = width.min(height);
    let max_dim = width.max(height);

    let mut high_rule_hits = Vec::new();
    if entropy >= HIGH_ENTROPY_GE {
        high_rule_hits.push("entropy_ge");
    }
    if min_dim >= HIGH_PIXEL_MIN_DIM_GE && entropy >= HIGH_DIMENSION_ENTROPY_FLOOR {
        high_rule_hits.push("pixel_min_dim_ge");
    }

    let mut low_rule_hits = Vec::new();
    if entropy <= LOW_ENTROPY_LE {
        low_rule_hits.push("entropy_le");
    }
    if max_dim <= LOW_PIXEL_MAX_DIM_LE && entropy <= LOW_DIMENSION_ENTROPY_CEIL {
        low_rule_hits.push("pixel_max_dim_le");
    }

    let in_entropy_dead_zone =
        entropy > TIER_ENTROPY_DEAD_ZONE_LO && entropy < TIER_ENTROPY_DEAD_ZONE_HI;
    let high_tier = match HIGH_TIER_LOGIC {
        TierRuleLogic::All => {
            if in_entropy_dead_zone {
                false
            } else {
                HIGH_TIER_LOGIC.combine(high_rule_hits.len(), HIGH_TIER_RULE_COUNT)
            }
        }
        TierRuleLogic::Any => {
            // ANY: dead zone must not veto dimension-qualified highs (M159 corpus).
            !high_rule_hits.is_empty()
        }
    };
    let low_tier = match LOW_TIER_LOGIC {
        TierRuleLogic::All => {
            if in_entropy_dead_zone {
                false
            } else {
                LOW_TIER_LOGIC.combine(low_rule_hits.len(), LOW_TIER_RULE_COUNT)
            }
        }
        TierRuleLogic::Any => {
            // ANY: one rule hit qualifies; dead zone must not veto dimension-only lows (M157).
            !low_rule_hits.is_empty()
        }
    };

    StaticTierEvaluation {
        high_tier,
        low_tier,
        high_rule_hits,
        low_rule_hits,
    }
}

/// Reject animated assets before static tier / `image_quality` ingest.
///
/// # Errors
///
/// Returns an error when format or animation detection fails, or the asset is animated.
pub fn assert_non_animated_static_asset(path: &Path) -> Result<()> {
    let format = crate::image_detection::detect_format_from_bytes(path)
        .with_context(|| format!("format detection failed: {}", path.display()))?;
    let (is_animated, _, _) = crate::image_detection::detect_animation(path, &format)
        .with_context(|| format!("animation detection failed: {}", path.display()))?;
    if is_animated {
        anyhow::bail!(
            "animated asset excluded from static tier probe: {}",
            path.display()
        );
    }
    Ok(())
}

/// Same analysis path as ingest: reject animated assets, use `analyze_image` entropy.
///
/// # Errors
/// Returns an error when the file is animated, unreadable, or entropy is missing/non-finite.
pub fn probe_static_still_image(path: &Path) -> Result<StaticStillProbe> {
    assert_non_animated_static_asset(path)?;

    let analysis =
        analyze_image(path).with_context(|| format!("analyze_image failed: {}", path.display()))?;
    probe_from_analysis(&analysis)
}

/// # Errors
///
/// Returns an error when entropy is missing or non-finite after analysis.
pub fn probe_from_analysis(analysis: &ImageAnalysis) -> Result<StaticStillProbe> {
    let entropy = analysis
        .features
        .entropy
        .filter(|v| v.is_finite())
        .ok_or_else(|| anyhow::anyhow!("missing or non-finite entropy after analyze_image"))?;

    Ok(StaticStillProbe {
        width: analysis.width,
        height: analysis.height,
        entropy,
        format: analysis.format.clone(),
        tier: evaluate_static_tier(entropy, analysis.width, analysis.height),
    })
}

/// How to handle assets that match **both** high and low tier rules (ANY on each side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierAmbiguousPolicy {
    /// Strict contraction: drop from collect/ingest (no silent prefer-high).
    Exclude,
    /// Legacy: assign `high` when both tiers match.
    PreferHigh,
    /// Assign `low` when both tiers match.
    PreferLow,
}

impl TierAmbiguousPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exclude => "exclude",
            Self::PreferHigh => "prefer_high",
            Self::PreferLow => "prefer_low",
        }
    }
}

/// Resolve collect-time label under the given ambiguous-tier policy.
#[must_use]
pub const fn resolve_collect_tier_label_with_policy(
    tier: &StaticTierEvaluation,
    policy: TierAmbiguousPolicy,
) -> Option<AssignedTrainingTier> {
    let ambiguous = tier.high_tier && tier.low_tier;
    if ambiguous {
        return match policy {
            TierAmbiguousPolicy::Exclude => None,
            TierAmbiguousPolicy::PreferHigh => Some(AssignedTrainingTier::High),
            TierAmbiguousPolicy::PreferLow => Some(AssignedTrainingTier::Low),
        };
    }
    if tier.high_tier {
        Some(AssignedTrainingTier::High)
    } else if tier.low_tier {
        Some(AssignedTrainingTier::Low)
    } else {
        None
    }
}

/// Resolve collect-time label using the committed policy (**exclude**).
#[must_use]
pub const fn resolve_collect_tier_label(
    tier: &StaticTierEvaluation,
) -> Option<AssignedTrainingTier> {
    resolve_collect_tier_label_with_policy(tier, COMMITTED_TIER_AMBIGUOUS_POLICY)
}

/// Reject `image_quality` ingest when the assigned label disagrees with tier rules or dead zone.
///
/// # Errors
///
/// Returns an error when the label is unknown, entropy is missing, no tier resolves, the
/// label does not match the resolved tier, analysis recorded an error, or (when `path` is a
/// regular file) animation detection flags the asset.
pub fn verify_training_tier_for_ingest(
    analysis: &ImageAnalysis,
    training_label: &str,
    path: &Path,
) -> Result<()> {
    if path.is_file() {
        assert_non_animated_static_asset(path)?;
    }
    if let Some(err) = analysis.analysis_error.as_deref() {
        anyhow::bail!(
            "analysis_error blocks image_quality ingest for {}: {err}",
            path.display()
        );
    }
    let assigned = AssignedTrainingTier::parse_label(training_label).with_context(|| {
        format!("invalid training label for image_quality ingest: {training_label}")
    })?;
    let probe = probe_from_analysis(analysis)?;
    if !tier_label_matches_assignment(&probe.tier, assigned) {
        anyhow::bail!(
            "training tier rules do not match assigned label '{training_label}' \
             (entropy={}, {}x{}, high_tier={}, low_tier={})",
            probe.entropy,
            probe.width,
            probe.height,
            probe.tier.high_tier,
            probe.tier.low_tier
        );
    }
    let resolved =
        resolve_collect_tier_label_with_policy(&probe.tier, COMMITTED_TIER_AMBIGUOUS_POLICY);
    let Some(resolved) = resolved else {
        anyhow::bail!(
            "training tier unresolved in entropy dead zone or partial rule hits \
             (entropy={}, {}x{}, label={training_label})",
            probe.entropy,
            probe.width,
            probe.height
        );
    };
    if assigned != resolved {
        anyhow::bail!(
            "training tier inconsistent: assigned={training_label}, resolved={}, entropy={}",
            match resolved {
                AssignedTrainingTier::High => "high",
                AssignedTrainingTier::Low => "low",
            },
            probe.entropy
        );
    }
    Ok(())
}

#[must_use]
pub const fn tier_label_matches_assignment(
    tier: &StaticTierEvaluation,
    assigned: AssignedTrainingTier,
) -> bool {
    match assigned {
        AssignedTrainingTier::High => tier.high_tier,
        AssignedTrainingTier::Low => tier.low_tier,
    }
}

/// JSON object for `image_quality_samples.metadata.training_tier_audit`.
#[must_use]
pub fn build_training_tier_audit_value(
    probe: &StaticStillProbe,
    assigned_label: Option<&str>,
) -> Value {
    let policy = COMMITTED_TIER_AMBIGUOUS_POLICY;
    let assigned_tier = assigned_label.and_then(AssignedTrainingTier::parse_label);
    let resolved_from_rules = resolve_collect_tier_label_with_policy(&probe.tier, policy);
    let tier_consistent = assigned_tier
        .zip(resolved_from_rules)
        .is_some_and(|(a, r)| a == r);

    let mut high_rules = Map::new();
    high_rules.insert(
        "logic".into(),
        Value::String(HIGH_TIER_LOGIC.as_str().into()),
    );
    high_rules.insert(
        "dimension_entropy_floor".into(),
        Value::from(HIGH_DIMENSION_ENTROPY_FLOOR),
    );
    high_rules.insert("entropy_ge".into(), Value::from(HIGH_ENTROPY_GE));
    high_rules.insert(
        "pixel_min_dim_ge".into(),
        Value::from(HIGH_PIXEL_MIN_DIM_GE),
    );
    high_rules.insert(
        "matched".into(),
        Value::Array(
            probe
                .tier
                .high_rule_hits
                .iter()
                .map(|s| Value::String((*s).to_string()))
                .collect(),
        ),
    );

    let mut low_rules = Map::new();
    low_rules.insert(
        "logic".into(),
        Value::String(LOW_TIER_LOGIC.as_str().into()),
    );
    low_rules.insert(
        "dimension_entropy_ceil".into(),
        Value::from(LOW_DIMENSION_ENTROPY_CEIL),
    );
    low_rules.insert("entropy_le".into(), Value::from(LOW_ENTROPY_LE));
    low_rules.insert("pixel_max_dim_le".into(), Value::from(LOW_PIXEL_MAX_DIM_LE));
    low_rules.insert(
        "matched".into(),
        Value::Array(
            probe
                .tier
                .low_rule_hits
                .iter()
                .map(|s| Value::String((*s).to_string()))
                .collect(),
        ),
    );

    json!({
        "schema_version": TRAINING_TIER_AUDIT_SCHEMA_VERSION,
        "entropy_engine": "rust_analyze_image",
        "rules_source": "training_rules.json:static_image",
        "assigned_label": assigned_label,
        "assigned_tier": assigned_tier.map(|t| match t {
            AssignedTrainingTier::High => "high",
            AssignedTrainingTier::Low => "low",
        }),
        "resolved_tier_from_rules": resolved_from_rules.map(|t| match t {
            AssignedTrainingTier::High => "high",
            AssignedTrainingTier::Low => "low",
        }),
        "tier_consistent": tier_consistent,
        "tier_ambiguous_policy": policy.as_str(),
        "ambiguous_both_tiers": probe.tier.high_tier && probe.tier.low_tier,
        "entropy_dead_zone": {
            "lo": TIER_ENTROPY_DEAD_ZONE_LO,
            "hi": TIER_ENTROPY_DEAD_ZONE_HI,
        },
        "width": probe.width,
        "height": probe.height,
        "min_dim": probe.width.min(probe.height),
        "max_dim": probe.width.max(probe.height),
        "entropy": probe.entropy,
        "format": probe.format,
        "high_tier": probe.tier.high_tier,
        "low_tier": probe.tier.low_tier,
        "high_rules": high_rules,
        "low_rules": low_rules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluate_high_any_rule_suffices() {
        let tier = evaluate_static_tier(6.15, 1200, 1080);
        assert!(
            tier.high_tier,
            "1080px short side at entropy 6.15 qualifies under ANY (social still)"
        );
        assert!(tier.high_rule_hits.contains(&"pixel_min_dim_ge"));

        let tier = evaluate_static_tier(7.7, 800, 600);
        assert!(
            tier.high_tier,
            "entropy_ge alone qualifies under ANY even when short side < 1080"
        );

        let tier = evaluate_static_tier(6.0, 2000, 800);
        assert!(
            !tier.high_tier,
            "entropy in dead band with short side < 1080 must not qualify high under ANY"
        );
    }

    #[test]
    fn evaluate_low_any_rule_suffices() {
        let tier = evaluate_static_tier(2.7, 1920, 1080);
        assert!(tier.low_tier, "entropy_le alone qualifies under ANY");
        assert!(tier.low_rule_hits.contains(&"entropy_le"));

        let tier = evaluate_static_tier(2.8, 180, 120);
        assert!(tier.low_tier);
        assert!(tier.low_rule_hits.contains(&"entropy_le"));
        assert!(tier.low_rule_hits.contains(&"pixel_max_dim_le"));

        let tier = evaluate_static_tier(2.8, 200, 120);
        assert!(
            tier.low_tier,
            "entropy_le alone qualifies even when max side > 180px (ANY)"
        );

        let tier = evaluate_static_tier(4.0, 150, 100);
        assert!(
            tier.low_tier,
            "small frame with entropy in dead zone still low via pixel_max_dim_le (ANY)"
        );
        assert!(tier.low_rule_hits.contains(&"pixel_max_dim_le"));
    }

    #[test]
    fn corpus_rejects_over_4k_pixels() {
        let tier = evaluate_static_tier(8.0, 4097, 1080);
        assert!(!tier.high_tier && !tier.low_tier);
        let tier = evaluate_static_tier(8.0, 3840, 4097);
        assert!(!tier.high_tier && !tier.low_tier);
        let tier = evaluate_static_tier(8.0, 4096, 4096);
        assert!(tier.high_tier);
    }

    #[test]
    fn dead_zone_excludes_mid_entropy_high_but_not_dimension_low() {
        let tier = evaluate_static_tier(5.0, 4000, 3000);
        assert!(!tier.high_tier && !tier.low_tier);

        let tier = evaluate_static_tier(4.0, 120, 90);
        assert!(!tier.high_tier);
        assert!(
            tier.low_tier,
            "dead zone must not block dimension-only low under ANY (entropy≤4.1 guard)"
        );
    }

    #[test]
    fn large_frame_low_entropy_is_low_under_any() {
        let tier = evaluate_static_tier(2.8, 4000, 4000);
        assert!(!tier.high_tier);
        assert!(
            tier.low_tier,
            "entropy_le qualifies without pixel rule under ANY"
        );
        assert_eq!(
            resolve_collect_tier_label_with_policy(&tier, TierAmbiguousPolicy::Exclude),
            Some(AssignedTrainingTier::Low)
        );
    }

    #[test]
    fn prefer_high_only_applies_when_both_tiers_match() {
        let tier = StaticTierEvaluation {
            high_tier: true,
            low_tier: true,
            high_rule_hits: vec!["entropy_ge"],
            low_rule_hits: vec!["entropy_le"],
        };
        assert_eq!(
            resolve_collect_tier_label_with_policy(&tier, TierAmbiguousPolicy::PreferHigh),
            Some(AssignedTrainingTier::High)
        );
    }

    #[test]
    fn tier_consistent_requires_assigned_equals_resolved() {
        let tier = evaluate_static_tier(7.7, 4000, 3000);
        let probe = StaticStillProbe {
            width: 4000,
            height: 3000,
            entropy: 7.7,
            format: "JPEG".into(),
            tier,
        };
        let audit = build_training_tier_audit_value(&probe, Some("high"));
        assert_eq!(
            audit
                .get("tier_consistent")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn verify_ingest_rejects_dead_zone_label() {
        use crate::image_analyzer::{ImageAnalysis, ImageFeatures};
        let analysis = ImageAnalysis {
            width: 4000,
            height: 3000,
            features: ImageFeatures {
                entropy: Some(5.0),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            verify_training_tier_for_ingest(&analysis, "high", Path::new("dead_zone.png")).is_err()
        );
    }

    #[test]
    fn verify_ingest_rejects_mismatched_label() {
        use crate::image_analyzer::{ImageAnalysis, ImageFeatures};
        let analysis = ImageAnalysis {
            width: 4000,
            height: 3000,
            features: ImageFeatures {
                entropy: Some(7.7),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            verify_training_tier_for_ingest(&analysis, "low", Path::new("mismatch.png")).is_err()
        );
        assert!(verify_training_tier_for_ingest(&analysis, "high", Path::new("match.png")).is_ok());
    }

    #[test]
    fn verify_ingest_rejects_analysis_error() {
        use crate::image_analyzer::{ImageAnalysis, ImageFeatures};
        let analysis = ImageAnalysis {
            width: 4000,
            height: 3000,
            features: ImageFeatures {
                entropy: Some(7.7),
                ..Default::default()
            },
            analysis_error: Some("animation probe failed".into()),
            ..Default::default()
        };
        assert!(verify_training_tier_for_ingest(&analysis, "high", Path::new("err.png")).is_err());
    }

    #[test]
    fn collect_resolve_uses_committed_exclude_not_prefer_high() {
        let tier = StaticTierEvaluation {
            high_tier: true,
            low_tier: true,
            high_rule_hits: vec!["entropy_ge"],
            low_rule_hits: vec!["entropy_le"],
        };
        assert_eq!(resolve_collect_tier_label(&tier), None);
        assert_eq!(
            resolve_collect_tier_label_with_policy(&tier, TierAmbiguousPolicy::PreferHigh),
            Some(AssignedTrainingTier::High)
        );
    }
}

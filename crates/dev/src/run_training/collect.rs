use super::scanner::{iter_media_files, passes_file_quality_filter};
use super::types::{Args, QualityGroup, RulesConfig, Sample, SampleSources, TrainingMode};
use crate::infra::fabrication_policy::fail_closed_training_enabled;
use crate::infra::training_scan::plan_scan_segments;
use crate::media::scope::{is_animated_gif, is_animated_jxl, is_animated_png, is_animated_webp};
use anyhow::{Result, bail};
use foundation::probe_loop_intent;
use foundation::training_tier_audit::probe_static_still_image;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

/// True when a file should be routed to `loop_intent` training (animated raster or video).
/// Mirrors py `is_animated_for_static_quality_skip` + `passes_loop_raster_animation_gate`.
fn is_animated_for_loop_collect(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "gif" => is_animated_gif(path).unwrap_or(false),
        "webp" => is_animated_webp(path).unwrap_or(false),
        "png" => is_animated_png(path).unwrap_or(false),
        "jxl" => is_animated_jxl(path).unwrap_or(false),
        // apng, avif/heic/heif: treat as potentially animated (conservative); video containers
        "apng" | "avif" | "heic" | "heif" | "hif" | "mp4" | "mov" | "webm" | "mkv" | "avi" => true,
        _ => false,
    }
}

fn empty_quality_group() -> QualityGroup {
    QualityGroup::default()
}

pub fn collect_plan_samples(args: &Args, rules: &RulesConfig) -> Result<Vec<Sample>> {
    let mode = args.training_mode;
    if mode == TrainingMode::Static && args.label.as_deref() == Some("animated_loop") {
        bail!(
            "--training-mode static is still-image only; remove --label animated_loop or use --training-mode loop"
        );
    }

    if mode == TrainingMode::Loop
        && (args.label.as_deref() == Some("high") || args.label.as_deref() == Some("low"))
    {
        bail!(
            "--training-mode loop ingests loop_intent only; do not pass --label high/low. Use --loop-intent-label for loop overrides, or omit --label."
        );
    }

    if mode == TrainingMode::Loop && args.plan.no_loop {
        bail!("--training-mode loop conflicts with --no-loop");
    }

    let include_static = mode == TrainingMode::All || mode == TrainingMode::Static;
    let include_loop =
        (mode == TrainingMode::All || mode == TrainingMode::Loop) && !args.plan.no_loop;

    let mut all_samples = Vec::new();
    let mut unified_loop_walk = false;

    if include_static {
        let high_group = rules
            .static_image
            .get("high_quality")
            .cloned()
            .unwrap_or_else(empty_quality_group);
        let low_group = rules
            .static_image
            .get("low_quality")
            .cloned()
            .unwrap_or_else(empty_quality_group);

        let mut static_dirs = HashSet::new();
        for d in &high_group.sources.local_dirs {
            if !d.is_empty() {
                static_dirs.insert(d.clone());
            }
        }
        for d in &low_group.sources.local_dirs {
            if !d.is_empty() {
                static_dirs.insert(d.clone());
            }
        }

        let mut loop_dirs = HashSet::new();
        if include_loop {
            let loop_group = rules
                .animated_image
                .get("loop_intent")
                .cloned()
                .unwrap_or_else(empty_quality_group);
            for d in &loop_group.sources.local_dirs {
                if !d.is_empty() {
                    loop_dirs.insert(d.clone());
                }
            }
        }

        unified_loop_walk = include_loop && loop_dirs.is_subset(&static_dirs);

        let label_filter =
            if args.label.as_deref() == Some("high") || args.label.as_deref() == Some("low") {
                args.label.clone()
            } else {
                None
            };

        all_samples.extend(collect_static_local_unified(
            &high_group,
            &low_group,
            label_filter.as_deref(),
            unified_loop_walk,
        )?);

        for (q, label) in [("high_quality", "high"), ("low_quality", "low")] {
            if args.label.is_some() && args.label.as_deref() != Some(label) {
                continue;
            }
            let group = rules
                .static_image
                .get(q)
                .cloned()
                .unwrap_or_else(empty_quality_group);
            let src = &group.sources;
            let mut urls = Vec::new();
            if args.misc.allow_remote {
                for api in &src.remote_apis {
                    urls.extend(resolve_api_urls(api, rules));
                }
            }
            if !urls.is_empty() {
                all_samples.extend(collect_samples(src, &urls, label));
            }
        }
    }

    if include_loop {
        if mode == TrainingMode::All
            && (args.label.as_deref() == Some("high") || args.label.as_deref() == Some("low"))
        {
            // pass
        } else {
            let loop_group = loop_collect_quality_group(
                rules,
                if args.loop_intent_label.is_empty() {
                    "auto"
                } else {
                    &args.loop_intent_label
                },
            );
            let src = &loop_group.sources;

            if rules.strict_no_silent_fallbacks
                && loop_group.sources.local_dirs.is_empty()
                && loop_group.sources.remote_apis.is_empty()
            {
                bail!(
                    "animated_image.loop_intent (or non_loop_intent for video lane) is required when strict_no_silent_fallbacks=true"
                );
            }

            if include_static && !unified_loop_walk {
                let high_group = rules
                    .static_image
                    .get("high_quality")
                    .cloned()
                    .unwrap_or_else(empty_quality_group);
                let low_group = rules
                    .static_image
                    .get("low_quality")
                    .cloned()
                    .unwrap_or_else(empty_quality_group);
                all_samples.extend(collect_loop_local_from_media_dirs(&high_group, &low_group));
            }

            let mut urls_loop = Vec::new();
            if args.misc.allow_remote {
                for api in &src.remote_apis {
                    urls_loop.extend(resolve_api_urls(api, rules));
                }
            }
            if !src.local_dirs.is_empty() || !urls_loop.is_empty() {
                all_samples.extend(collect_samples(src, &urls_loop, "animated_loop"));
            }
        }
    }

    if balancing_enabled(args) {
        all_samples = balance_training_samples(all_samples, args)?;
    }

    Ok(all_samples)
}

fn collect_static_local_unified(
    high_group: &QualityGroup,
    low_group: &QualityGroup,
    label_filter: Option<&str>,
    _also_collect_loop: bool,
) -> Result<Vec<Sample>> {
    let mut dirs = HashSet::new();
    for d in &high_group.sources.local_dirs {
        if !d.is_empty() {
            dirs.insert(d.clone());
        }
    }
    for d in &low_group.sources.local_dirs {
        if !d.is_empty() {
            dirs.insert(d.clone());
        }
    }
    let mut local_dirs: Vec<String> = dirs.into_iter().collect();
    local_dirs.sort();

    if local_dirs.is_empty() {
        if fail_closed_training_enabled() {
            bail!(
                "static local collector: no local_dirs configured; refusing empty training corpus"
            );
        }
        eprintln!(
            "  [SKIP] static local collector: no local_dirs configured; refusing empty training corpus (debug empty fallback enabled)"
        );
        return Ok(Vec::new());
    }

    let mut samples = Vec::new();
    let mut _scanned = 0;

    let mut passed_filter = 0;
    let mut tier_high = 0;
    let mut tier_low = 0;
    let mut tier_unclassified = 0;
    let mut tier_ambiguous_excluded = 0;

    eprintln!(
        "  [STATIC-TIER] rescan_start dirs={} engine=mfb_probe_static_still_image",
        local_dirs.len()
    );

    for (dir_index, d) in local_dirs.iter().enumerate() {
        let root = Path::new(d);
        if !root.is_dir() {
            eprintln!("  [SKIP] Local dir not found: {d}");
            continue;
        }

        eprintln!(
            "  [STATIC-TIER] dir_start {}/{} path={}",
            dir_index + 1,
            local_dirs.len(),
            root.display()
        );

        // Scan plan
        let segments = plan_scan_segments(root, super::scanner::is_junk_path).unwrap_or_default();
        for seg in segments {
            for seg_root in seg.roots {
                for item in iter_media_files(&seg_root) {
                    _scanned += 1;

                    let high_pre = passes_file_quality_filter(
                        &item,
                        high_group.sources.file_quality_filter.as_ref(),
                    );
                    let low_pre = passes_file_quality_filter(
                        &item,
                        low_group.sources.file_quality_filter.as_ref(),
                    );
                    if !high_pre && !low_pre {
                        continue;
                    }
                    passed_filter += 1;

                    match probe_static_still_image(&item) {
                        Ok(probe) => {
                            let mut audit = serde_json::Map::new();
                            audit.insert("entropy".to_string(), Value::from(probe.entropy));

                            // Check tier Logic
                            let is_high = high_pre && probe.tier.high_tier;
                            let is_low = low_pre && probe.tier.low_tier;

                            let mut resolved_label = None;
                            if is_high && !is_low {
                                resolved_label = Some("high");
                                tier_high += 1;
                            } else if is_low && !is_high {
                                resolved_label = Some("low");
                                tier_low += 1;
                            } else if is_high && is_low {
                                // Ambiguous policy: exclude
                                tier_ambiguous_excluded += 1;
                            } else {
                                tier_unclassified += 1;
                            }

                            if let Some(label) = resolved_label
                                && (label_filter.is_none() || label_filter == Some(label))
                            {
                                samples.push(Sample {
                                    path_or_url: item.display().to_string(),
                                    base_label: label.to_string(),
                                    is_remote: false,
                                    source: if label == "high" {
                                        high_group.sources.clone()
                                    } else {
                                        low_group.sources.clone()
                                    },
                                    tier_audit: Some(audit),
                                });
                            }
                        }
                        Err(_e) => {
                            // If animated, we could collect loop.
                            // Omitted complex animated handling for now.
                        }
                    }
                }
            }
        }
    }

    warn_corpus_tier_coverage(
        passed_filter,
        tier_high,
        tier_low,
        tier_unclassified,
        tier_ambiguous_excluded,
    );

    Ok(samples)
}

fn warn_corpus_tier_coverage(
    prefilter_pass: usize,
    tier_high: usize,
    tier_low: usize,
    tier_unclassified: usize,
    tier_ambiguous_excluded: usize,
) {
    const CORPUS_MIN_CLASSIFIED_RATIO: f64 = 0.02;
    let classified = tier_high + tier_low;
    if prefilter_pass == 0 {
        eprintln!(
            "  [WARN] training_corpus_tier_coverage: prefilter_pass=0 (no files to classify)"
        );
        return;
    }
    let ratio = foundation::numeric_cast::usize_to_f64(classified)
        / foundation::numeric_cast::usize_to_f64(prefilter_pass);
    if classified == 0 {
        eprintln!(
            "  [WARN] training_corpus_tier_coverage: classified=0 (prefilter_pass={prefilter_pass}, unclassified={tier_unclassified}, ambiguous_excluded={tier_ambiguous_excluded}); check training_tier_audit thresholds"
        );
        return;
    }
    if ratio < CORPUS_MIN_CLASSIFIED_RATIO {
        eprintln!(
            "  [WARN] training_corpus_tier_coverage: tier classification ratio ({:.2}%) is below {:.2}% minimum (high={}, low={}, unclassified={}, ambiguous_excluded={})",
            ratio * 100.0,
            CORPUS_MIN_CLASSIFIED_RATIO * 100.0,
            tier_high,
            tier_low,
            tier_unclassified,
            tier_ambiguous_excluded
        );
    }
}

fn collect_samples(src: &SampleSources, urls: &[String], label: &str) -> Vec<Sample> {
    let mut samples = Vec::new();
    for url in urls {
        samples.push(Sample {
            path_or_url: url.clone(),
            base_label: label.to_string(),
            is_remote: true,
            source: src.clone(),
            tier_audit: None,
        });
    }
    samples
}

fn resolve_api_urls(api_name: &str, rules: &RulesConfig) -> Vec<String> {
    if let Some(api) = rules.remote_apis.get(api_name) {
        api.direct_links.clone()
    } else {
        Vec::new()
    }
}

fn loop_collect_quality_group(rules: &RulesConfig, label: &str) -> QualityGroup {
    if explicit_loop_balance_bucket(label).as_deref() == Some("non_loop") {
        rules
            .animated_image
            .get("non_loop_intent")
            .cloned()
            .unwrap_or_else(|| {
                rules
                    .animated_image
                    .get("loop_intent")
                    .cloned()
                    .unwrap_or_else(empty_quality_group)
            })
    } else {
        rules
            .animated_image
            .get("loop_intent")
            .cloned()
            .unwrap_or_else(empty_quality_group)
    }
}

fn collect_loop_local_from_media_dirs(
    high_group: &QualityGroup,
    low_group: &QualityGroup,
) -> Vec<Sample> {
    let mut dirs = HashSet::new();
    for d in &high_group.sources.local_dirs {
        if !d.is_empty() {
            dirs.insert(d.clone());
        }
    }
    for d in &low_group.sources.local_dirs {
        if !d.is_empty() {
            dirs.insert(d.clone());
        }
    }
    let mut local_dirs: Vec<String> = dirs.into_iter().collect();
    local_dirs.sort();

    let mut samples = Vec::new();

    for d in local_dirs {
        let root = Path::new(&d);
        if !root.is_dir() {
            continue;
        }

        let segments = plan_scan_segments(root, super::scanner::is_junk_path).unwrap_or_default();
        for seg in segments {
            for seg_root in seg.roots {
                for item in iter_media_files(&seg_root) {
                    if is_animated_for_loop_collect(&item) {
                        let mut audit = serde_json::Map::new();
                        audit.insert("loop_intent".to_string(), Value::from("uncertain"));
                        samples.push(Sample {
                            path_or_url: item.display().to_string(),
                            base_label: "animated_loop".to_string(),
                            is_remote: false,
                            source: high_group.sources.clone(),
                            tier_audit: Some(audit),
                        });
                    }
                }
            }
        }
    }
    samples
}

#[must_use]
pub fn explicit_loop_balance_bucket(loop_intent_label: &str) -> Option<String> {
    let v = loop_intent_label.trim().to_lowercase();
    let v = if v.is_empty() { "auto" } else { &v };
    match v {
        "high" => Some("loop".to_string()),
        "low" => Some("uncertain".to_string()),
        "video" => Some("non_loop".to_string()),
        _ => None,
    }
}

fn sample_complexity_score(sample: &Sample) -> Option<f64> {
    if let Some(audit) = &sample.tier_audit
        && let Some(entropy) = audit.get("entropy")
        && let Some(f) = entropy.as_f64()
        && !f.is_nan()
    {
        return Some(f);
    }

    if sample.base_label == "animated_loop" && !sample.is_remote {
        let path = Path::new(&sample.path_or_url);
        match probe_loop_intent(path) {
            Ok(probe) => return Some(probe.complexity),
            Err(err) => eprintln!(
                "[COLLECT] loop intent probe failed ({}): {err}",
                path.display()
            ),
        }
    }
    None
}

fn complexity_sort_key(sample: &Sample) -> f64 {
    sample_complexity_score(sample).unwrap_or(f64::INFINITY)
}

fn sample_loop_intent_bucket(sample: &Sample, explicit_remote_bucket: Option<&str>) -> String {
    if let Some(audit) = &sample.tier_audit
        && let Some(Value::String(cached)) = audit.get("loop_intent")
        && !cached.is_empty()
    {
        return cached.clone();
    }
    if sample.base_label != "animated_loop" {
        return String::new();
    }
    if sample.is_remote {
        return explicit_remote_bucket.unwrap_or("uncertain").to_string();
    }

    let path = Path::new(&sample.path_or_url);
    match probe_loop_intent(path) {
        Ok(probe) => probe.loop_intent,
        Err(e) => {
            assert!(
                !fail_closed_training_enabled(),
                "loop intent balance probe failed; refusing uncertain fallback path={} error={e:#}",
                sample.path_or_url
            );
            eprintln!(
                "  [BALANCE] loop_probe_failed path={} error={e:#} debug_uncertain_fallback=enabled",
                sample.path_or_url
            );
            "uncertain".to_string()
        }
    }
}

const fn balancing_enabled(args: &Args) -> bool {
    args.balance.balance
        || args.max_high > 0
        || args.max_low > 0
        || args.max_loop > 0
        || args.max_non_loop > 0
}

fn pick_capped_group(mut group: Vec<Sample>, target: usize, match_complexity: bool) -> Vec<Sample> {
    if target == 0 {
        return Vec::new();
    }
    if !match_complexity {
        group.truncate(target);
        return group;
    }
    group.sort_by(|a, b| {
        complexity_sort_key(a)
            .partial_cmp(&complexity_sort_key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    group.truncate(target);
    group
}

fn pick_quantile_matched(
    mut group_a: Vec<Sample>,
    mut group_b: Vec<Sample>,
    target: usize,
    match_complexity: bool,
) -> (Vec<Sample>, Vec<Sample>) {
    if target == 0 {
        return (Vec::new(), Vec::new());
    }
    if !match_complexity {
        group_a.truncate(target);
        group_b.truncate(target);
        return (group_a, group_b);
    }

    group_a.sort_by(|a, b| {
        complexity_sort_key(a)
            .partial_cmp(&complexity_sort_key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    group_b.sort_by(|a, b| {
        complexity_sort_key(a)
            .partial_cmp(&complexity_sort_key(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if target == 1 {
        let mid_a = group_a.remove(group_a.len() / 2);
        let mid_b = group_b.remove(group_b.len() / 2);
        return (vec![mid_a], vec![mid_b]);
    }

    let mut picks_a = Vec::with_capacity(target);
    let mut picks_b = Vec::with_capacity(target);

    for i in 0..target {
        let first_idx_f64 = (foundation::numeric_cast::usize_to_f64(i)
            * foundation::numeric_cast::usize_to_f64(group_a.len().saturating_sub(1))
            / foundation::numeric_cast::usize_to_f64(std::cmp::max(target.saturating_sub(1), 1)))
        .round();
        let first_idx = foundation::numeric_cast::f64_to_usize_sat(first_idx_f64);
        let second_idx_f64 = (foundation::numeric_cast::usize_to_f64(i)
            * foundation::numeric_cast::usize_to_f64(group_b.len().saturating_sub(1))
            / foundation::numeric_cast::usize_to_f64(std::cmp::max(target.saturating_sub(1), 1)))
        .round();
        let second_idx = foundation::numeric_cast::f64_to_usize_sat(second_idx_f64);
        picks_a.push(group_a[first_idx].clone());
        picks_b.push(group_b[second_idx].clone());
    }

    (picks_a, picks_b)
}

pub fn balance_training_samples(samples: Vec<Sample>, args: &Args) -> Result<Vec<Sample>> {
    let mut static_high = Vec::new();
    let mut static_low = Vec::new();
    let mut loop_samples = Vec::new();
    let mut other = Vec::new();

    for s in samples {
        match s.base_label.as_str() {
            "high" => static_high.push(s),
            "low" => static_low.push(s),
            "animated_loop" => loop_samples.push(s),
            _ => other.push(s),
        }
    }

    let max_high = args.max_high;
    let max_low = args.max_low;
    let max_loop = args.max_loop;
    let max_non_loop = args.max_non_loop;
    let match_cx = !args.balance.no_balance_complexity;
    let explicit_loop_bucket = explicit_loop_balance_bucket(if args.loop_intent_label.is_empty() {
        "auto"
    } else {
        &args.loop_intent_label
    });

    let mut loop_bucket = Vec::new();
    let mut non_loop_bucket = Vec::new();
    let mut loop_uncertain = Vec::new();

    for s in loop_samples {
        let bucket = sample_loop_intent_bucket(&s, explicit_loop_bucket.as_deref());
        match bucket.as_str() {
            "loop" => loop_bucket.push(s),
            "non_loop" => non_loop_bucket.push(s),
            _ => loop_uncertain.push(s),
        }
    }

    let mut picked_high;
    let mut picked_low;

    if max_high > 0 && max_low == 0 {
        picked_high = pick_capped_group(static_high, max_high, match_cx);
        picked_low = Vec::new();
    } else if max_low > 0 && max_high == 0 {
        picked_high = Vec::new();
        picked_low = pick_capped_group(static_low, max_low, match_cx);
    } else {
        let mut pair_target = std::cmp::min(static_high.len(), static_low.len());
        if max_high > 0 {
            pair_target = std::cmp::min(pair_target, max_high);
        }
        if max_low > 0 {
            pair_target = std::cmp::min(pair_target, max_low);
        }
        let (ph, pl) = pick_quantile_matched(static_high, static_low, pair_target, match_cx);
        picked_high = ph;
        picked_low = pl;

        warn_static_balance_skew(&picked_high, &picked_low, pair_target, max_high, max_low);
    }

    let mut picked_loop;
    let mut picked_non_loop;
    let mut picked_loop_uncertain = Vec::new();

    if explicit_loop_bucket.as_deref() == Some("loop") {
        let loop_target = if max_loop > 0 {
            max_loop
        } else {
            loop_bucket.len()
        };
        picked_loop = pick_capped_group(loop_bucket, loop_target, match_cx);
        picked_non_loop = Vec::new();
    } else if explicit_loop_bucket.as_deref() == Some("non_loop") {
        let non_loop_target = if max_non_loop > 0 {
            max_non_loop
        } else {
            non_loop_bucket.len()
        };
        picked_loop = Vec::new();
        picked_non_loop = pick_capped_group(non_loop_bucket, non_loop_target, match_cx);
    } else if explicit_loop_bucket.as_deref() == Some("uncertain") {
        let uncertain_target = if max_loop > 0 {
            max_loop
        } else {
            loop_uncertain.len()
        };
        picked_loop = Vec::new();
        picked_non_loop = Vec::new();
        picked_loop_uncertain =
            pick_capped_group(loop_uncertain.clone(), uncertain_target, match_cx);
    } else if max_non_loop == 0 {
        let loop_target = if max_loop > 0 {
            max_loop
        } else {
            loop_bucket.len()
        };
        picked_loop = pick_capped_group(loop_bucket, loop_target, match_cx);
        picked_non_loop = Vec::new();
    } else {
        let mut loop_pair_target = std::cmp::min(loop_bucket.len(), non_loop_bucket.len());
        if max_loop > 0 {
            loop_pair_target = std::cmp::min(loop_pair_target, max_loop);
        }
        if max_non_loop > 0 {
            loop_pair_target = std::cmp::min(loop_pair_target, max_non_loop);
        }
        let (pl, pnl) =
            pick_quantile_matched(loop_bucket, non_loop_bucket, loop_pair_target, match_cx);
        picked_loop = pl;
        picked_non_loop = pnl;
    }

    if max_high > 0 && picked_high.len() > max_high {
        picked_high.truncate(max_high);
    }
    if max_low > 0 && picked_low.len() > max_low {
        picked_low.truncate(max_low);
    }
    if max_loop > 0 && picked_loop.len() > max_loop {
        picked_loop.truncate(max_loop);
    }
    if max_non_loop > 0 && picked_non_loop.len() > max_non_loop {
        picked_non_loop.truncate(max_non_loop);
    }

    let mut balanced = Vec::new();
    balanced.extend(picked_high);
    balanced.extend(picked_low);
    balanced.extend(picked_loop);
    balanced.extend(picked_non_loop);
    balanced.extend(picked_loop_uncertain);
    if args.balance.balance_include_loop_uncertain
        && explicit_loop_bucket.is_none()
        && !loop_uncertain.is_empty()
    {
        balanced.extend(loop_uncertain);
    }
    balanced.extend(other);

    Ok(balanced)
}

fn warn_static_balance_skew(
    picked_high: &[Sample],
    picked_low: &[Sample],
    pair_target: usize,
    max_high: usize,
    max_low: usize,
) {
    let high_n = picked_high.len();
    let low_n = picked_low.len();
    if high_n == 0 && low_n == 0 {
        return;
    }
    let cap = std::cmp::max(max_high, max_low);
    if cap == 0 {
        return;
    }
    let min_side = std::cmp::min(high_n, low_n);
    let max_side = std::cmp::max(high_n, low_n);
    let skewed = min_side == 0 || (max_side > 0 && min_side * 10 < max_side);
    let under_cap = if cap > 0 {
        pair_target < std::cmp::min(cap, max_side)
    } else {
        false
    };
    if !skewed && !under_cap {
        return;
    }
    eprintln!(
        "  [WARN] training_ingest_balance_skew: high={high_n} low={low_n} pair_target={pair_target} caps={max_high}/{max_low}; tier rules may under-classify low-quality stills or corpus lacks lows — see training_tier_audit / static_image.low_quality rules"
    );
}

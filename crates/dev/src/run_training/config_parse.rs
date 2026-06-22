use super::types::{ApiInfo, QualityGroup, RulesConfig, SampleSources};
use crate::infra::config_load::{ensure_allowed_keys, load_consumer_json};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

const RULES_SCHEMA_VERSION: i64 = 1;

fn as_string(val: Option<&Value>) -> String {
    if let Some(Value::String(s)) = val {
        s.clone()
    } else {
        String::new()
    }
}

fn as_object(val: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    if let Some(Value::Object(map)) = val {
        Some(map)
    } else {
        None
    }
}

fn as_object_list(val: Option<&Value>) -> Vec<&Value> {
    if let Some(Value::Array(arr)) = val {
        arr.iter().collect()
    } else {
        Vec::new()
    }
}

fn as_string_list(val: Option<&Value>) -> Vec<String> {
    as_object_list(val)
        .into_iter()
        .filter_map(|v| {
            if let Value::String(s) = v {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect()
}

fn parse_api_info(raw: Option<&Value>) -> ApiInfo {
    let mut api = ApiInfo::default();
    if let Some(obj) = as_object(raw) {
        api.direct_links = as_string_list(obj.get("direct_links"));
        api.url_template = as_string(obj.get("url_template"));
        api.media_field = as_string(obj.get("media_field"));
    }
    api
}

fn parse_sample_sources(raw: Option<&Value>) -> SampleSources {
    let mut sources = SampleSources::default();
    if let Some(obj) = as_object(raw) {
        sources.local_dirs = as_string_list(obj.get("local_dirs"));
        sources.remote_apis = as_string_list(obj.get("remote_apis"));
        sources.selection_strategy = as_string(obj.get("selection_strategy"));
        if let Some(filter) = as_object(obj.get("file_quality_filter")) {
            sources.file_quality_filter = Some(filter.clone());
        }
    }
    sources
}

fn validate_source_rules(group_name: &str, sources: &SampleSources) -> Result<()> {
    if sources.local_dirs.is_empty() && sources.remote_apis.is_empty() {
        bail!(
            "{}: must specify at least one local_dir or remote_api",
            group_name
        );
    }
    if let Some(filter) = &sources.file_quality_filter {
        let logic = as_string(filter.get("logic")).to_uppercase();
        if logic != "ALL" && logic != "ANY" {
            bail!(
                "{}: file_quality_filter logic must be ALL or ANY",
                group_name
            );
        }
        for raw_rule in as_object_list(filter.get("rules")) {
            let rule_obj = as_object(Some(raw_rule)).context("rule must be an object")?;
            let rule_name = as_string(rule_obj.get("rule"));
            if rule_name.is_empty() {
                bail!(
                    "{}: file_quality_filter rule must contain non-empty 'rule'",
                    group_name
                );
            }
        }
    }
    Ok(())
}

fn validate_quality_group_rules(group_name: &str, group: &QualityGroup) -> Result<()> {
    validate_source_rules(group_name, &group.sources)?;
    if group.tier_rules.is_empty() {
        return Ok(());
    }
    let logic = group.tier_logic.to_uppercase();
    if logic != "ALL" && logic != "ANY" {
        bail!("{}: logic must be ALL or ANY", group_name);
    }
    for rule_obj in &group.tier_rules {
        let rule_name = as_string(rule_obj.get("rule"));
        if rule_name.is_empty() {
            bail!(
                "{}: each tier rule must contain non-empty 'rule'",
                group_name
            );
        }
        let known_rules = [
            "file_size_kb_ge",
            "file_size_kb_le",
            "extension_not_in",
            "filename_not_matches_regex",
            "path_not_contains_any",
            "is_supported_image_file",
            "is_supported_animated_image_file",
            "is_supported_loop_intent_media_file",
            "is_supported_non_loop_media_file",
        ];
        if !known_rules.contains(&rule_name.as_str()) {
            bail!("{}: unknown tier rule '{}'", group_name, rule_name);
        }
        if rule_obj.contains_key("value") {
            let val = rule_obj.get("value").unwrap();
            if rule_name.contains("_ge") || rule_name.contains("_le") {
                if !val.is_number() {
                    bail!("{} requires numeric value", rule_name);
                }
            } else if rule_name.contains("_regex") && !val.is_string() {
                bail!("{} requires string value", rule_name);
            }
        }
    }
    Ok(())
}

fn validate_rust_tier_contract(static_image: &HashMap<String, QualityGroup>) -> Result<()> {
    // high_quality
    if let Some(group) = static_image.get("high_quality") {
        if group.tier_logic.to_uppercase() != "ALL" {
            bail!(
                "static_image.high_quality.logic must be ALL (Rust tier combiner), got {}",
                group.tier_logic
            );
        }
        let expected_rules = vec![
            "file_size_kb_ge",
            "filename_not_matches_regex",
            "path_not_contains_any",
        ];
        for rule_obj in &group.tier_rules {
            let rule_name = as_string(rule_obj.get("rule"));
            if !expected_rules.contains(&rule_name.as_str()) {
                bail!(
                    "static_image.high_quality tier rule mismatch: expected one of {:?}, got {}",
                    expected_rules,
                    rule_name
                );
            }
        }
    }
    // low_quality
    if let Some(group) = static_image.get("low_quality") {
        if group.tier_logic.to_uppercase() != "ANY" {
            bail!(
                "static_image.low_quality.logic must be ANY (Rust tier combiner), got {}",
                group.tier_logic
            );
        }
        let expected_rules = vec![
            "file_size_kb_le",
            "filename_not_matches_regex",
            "path_not_contains_any",
        ];
        for rule_obj in &group.tier_rules {
            let rule_name = as_string(rule_obj.get("rule"));
            if !expected_rules.contains(&rule_name.as_str()) {
                bail!(
                    "static_image.low_quality tier rule mismatch: expected one of {:?}, got {}",
                    expected_rules,
                    rule_name
                );
            }
        }
    }
    Ok(())
}

fn validate_video_section(value: Option<&Value>) -> Result<()> {
    let video_obj = match as_object(value) {
        Some(obj) => obj,
        None => return Ok(()),
    };

    let section_names = [
        "contrast_fast_silent_loop",
        "prefer_grey_zone_loop_low",
        "contrast_with_audio",
        "contrast_silent_anim",
        "deprioritize_grey_zone",
        "keep_with_audio",
        "keep_silent",
        "reject",
    ];
    let allowed_top: Vec<String> = section_names
        .iter()
        .map(|s| s.to_string())
        .chain(
            video_obj
                .keys()
                .filter(|k| k.starts_with("_comment"))
                .cloned(),
        )
        .collect();
    let allowed_top_refs: Vec<&str> = allowed_top.iter().map(AsRef::as_ref).collect();
    ensure_allowed_keys(video_obj, &allowed_top_refs, "video", None)?;

    let allowed_video_rules = [
        "has_audio AND duration_in_grey_zone",
        "no_audio AND duration_in_grey_zone",
        "has_audio AND duration_lt",
        "has_audio AND duration_gt",
        "no_audio AND duration_lt",
        "no_audio AND duration_gt",
        "duration_lt",
        "duration_gt",
    ];

    for section in section_names {
        if let Some(section_val) = video_obj.get(section) {
            let section_obj = as_object(Some(section_val))
                .context(format!("video.{} is required to be an object", section))?;
            ensure_allowed_keys(
                section_obj,
                &["logic", "rules"],
                &format!("video.{}", section),
                None,
            )?;
            let logic = as_string(section_obj.get("logic")).to_uppercase();
            if logic != "ANY" && logic != "ALL" {
                bail!("video.{}.logic must be ALL or ANY", section);
            }
            let rules = as_object_list(section_obj.get("rules"));
            if rules.is_empty() {
                bail!("video.{}.rules must be non-empty", section);
            }
            for (idx, raw_rule) in rules.iter().enumerate() {
                let rule_obj = as_object(Some(*raw_rule)).context("rule must be an object")?;
                ensure_allowed_keys(
                    rule_obj,
                    &["rule", "value", "grey_zone_secs", "desc"],
                    &format!("video.{}.rules[{}]", section, idx),
                    None,
                )?;
                let rule_name = as_string(rule_obj.get("rule"));
                if rule_name.is_empty() {
                    bail!("video.{}.rules[{}].rule must be non-empty", section, idx);
                }
                if !allowed_video_rules.contains(&rule_name.trim()) {
                    bail!(
                        "video.{}.rules[{}].rule is unsupported: {}",
                        section,
                        idx,
                        rule_name
                    );
                }
                let desc = as_string(rule_obj.get("desc"));
                if desc.is_empty() {
                    bail!("video.{}.rules[{}].desc must be non-empty", section, idx);
                }
                if [
                    "has_audio AND duration_in_grey_zone",
                    "no_audio AND duration_in_grey_zone",
                ]
                .contains(&rule_name.trim())
                {
                    let grey = as_object(rule_obj.get("grey_zone_secs")).context(format!(
                        "video.{}.rules[{}] requires grey_zone_secs",
                        section, idx
                    ))?;
                    ensure_allowed_keys(
                        grey,
                        &["min", "max"],
                        &format!("video.{}.rules[{}].grey_zone_secs", section, idx),
                        None,
                    )?;
                    let min_v = grey
                        .get("min")
                        .and_then(|v| v.as_f64())
                        .context("grey_zone_secs.min must be numeric")?;
                    let max_v = grey
                        .get("max")
                        .and_then(|v| v.as_f64())
                        .context("grey_zone_secs.max must be numeric")?;
                    if min_v >= max_v {
                        bail!(
                            "video.{}.rules[{}].grey_zone_secs requires min < max",
                            section,
                            idx
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

fn parse_sample_source_section(
    raw: Option<&Value>,
    section_name: &str,
) -> Result<HashMap<String, QualityGroup>> {
    let mut groups = HashMap::new();
    if let Some(obj) = as_object(raw) {
        let allowed = vec![
            "high_quality",
            "low_quality",
            "loop_intent",
            "non_loop_intent",
        ];
        let _ = ensure_allowed_keys(obj, &allowed, section_name, None);
        for key in allowed {
            if let Some(group_obj) = as_object(obj.get(key)) {
                let _ = ensure_allowed_keys(
                    group_obj,
                    &["sources", "logic", "rules"],
                    &format!("{}.{}", section_name, key),
                    None,
                );
                let sources = parse_sample_sources(group_obj.get("sources"));
                let mut logic = as_string(group_obj.get("logic"));
                if logic.is_empty() {
                    logic = "ANY".to_string();
                }

                let mut rules = Vec::new();
                for rule_val in as_object_list(group_obj.get("rules")) {
                    if let Some(r) = as_object(Some(rule_val)) {
                        rules.push(r.clone());
                    }
                }

                let qg = QualityGroup {
                    sources,
                    tier_logic: logic,
                    tier_rules: rules,
                };
                validate_quality_group_rules(&format!("{}.{}", section_name, key), &qg)?;
                groups.insert(key.to_string(), qg);
            }
        }
    }
    Ok(groups)
}

pub fn load_rules(rules_file: &Path, local_rules_file: Option<&Path>) -> Result<RulesConfig> {
    let root = load_consumer_json(rules_file, "run_training.py")?;

    ensure_allowed_keys(
        &root,
        &[
            "_comment",
            "_consumer",
            "rule_engine",
            "remote_apis",
            "static_image",
            "animated_image",
            "video",
        ],
        "training_rules.json",
        None,
    )?;

    let rule_engine = as_object(root.get("rule_engine")).context("Missing rule_engine")?;
    ensure_allowed_keys(
        rule_engine,
        &[
            "schema_version",
            "strict_no_silent_fallbacks",
            "strict_unknown_rules",
            "tier_ambiguous_policy",
        ],
        "rule_engine",
        None,
    )?;

    let schema_version = match rule_engine.get("schema_version").and_then(|v| v.as_i64()) {
        Some(v) => v,
        None => {
            eprintln!("[CONFIG] rule_engine.schema_version missing; defaulting to 0");
            0
        }
    };
    if schema_version != RULES_SCHEMA_VERSION {
        bail!(
            "rule_engine.schema_version mismatch: expected {}, got {}",
            RULES_SCHEMA_VERSION,
            schema_version
        );
    }

    let strict_no_silent_fallbacks = rule_engine
        .get("strict_no_silent_fallbacks")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let strict_unknown_rules = rule_engine
        .get("strict_unknown_rules")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let mut tier_ambiguous_policy = as_string(rule_engine.get("tier_ambiguous_policy"));
    if tier_ambiguous_policy.is_empty() {
        tier_ambiguous_policy = "exclude".to_string();
    }
    tier_ambiguous_policy = tier_ambiguous_policy.trim().to_lowercase();

    let mut remote_apis = HashMap::new();
    if let Some(apis) = as_object(root.get("remote_apis")) {
        for (k, v) in apis {
            remote_apis.insert(k.clone(), parse_api_info(Some(v)));
        }
    }

    let mut static_image = parse_sample_source_section(root.get("static_image"), "static_image")?;
    let mut animated_image =
        parse_sample_source_section(root.get("animated_image"), "animated_image")?;
    validate_video_section(root.get("video"))?;
    let mut ingest = None;

    if let Some(local_path) = local_rules_file
        && local_path.is_file()
    {
        let local_root = load_consumer_json(local_path, "run_training.py")?;
        ensure_allowed_keys(
            &local_root,
            &[
                "_comment",
                "_consumer",
                "rule_engine",
                "ingest",
                "static_image",
                "animated_image",
            ],
            "training_rules.local.json",
            None,
        )?;

        if let Some(raw_ingest) = as_object(local_root.get("ingest")) {
            ingest = Some(raw_ingest.clone());
        }

        let local_engine = as_object(local_root.get("rule_engine"))
            .context("training_rules.local.json.rule_engine.schema_version is required")?;
        ensure_allowed_keys(
            local_engine,
            &["schema_version"],
            "training_rules.local.json.rule_engine",
            None,
        )?;
        let local_schema_version = match local_engine.get("schema_version").and_then(|v| v.as_i64())
        {
            Some(v) => v,
            None => {
                eprintln!("[CONFIG] local rule_engine.schema_version missing; defaulting to 0");
                0
            }
        };
        if local_schema_version != schema_version {
            bail!("training_rules.local.json.rule_engine.schema_version mismatch");
        }

        // merge_local_sample_dirs: overlay local_dirs only, preserve all other fields
        let local_static =
            parse_sample_source_section(local_root.get("static_image"), "static_image")?;
        for (k, local_group) in local_static {
            if let Some(existing) = static_image.get_mut(&k) {
                // Only append local_dirs; do not overwrite strategy/filter/tier rules
                let mut merged_dirs = existing.sources.local_dirs.clone();
                for d in &local_group.sources.local_dirs {
                    if !merged_dirs.contains(d) {
                        merged_dirs.push(d.clone());
                    }
                }
                existing.sources.local_dirs = merged_dirs;
            }
            // unknown keys in local are silently ignored (py: raises ValueError)
        }

        let local_animated =
            parse_sample_source_section(local_root.get("animated_image"), "animated_image")?;
        for (k, local_group) in local_animated {
            if let Some(existing) = animated_image.get_mut(&k) {
                let mut merged_dirs = existing.sources.local_dirs.clone();
                for d in &local_group.sources.local_dirs {
                    if !merged_dirs.contains(d) {
                        merged_dirs.push(d.clone());
                    }
                }
                existing.sources.local_dirs = merged_dirs;
            }
        }
    }

    validate_rust_tier_contract(&static_image)?;

    Ok(RulesConfig {
        strict_unknown_rules,
        strict_no_silent_fallbacks,
        tier_ambiguous_policy,
        remote_apis,
        static_image,
        animated_image,
        ingest,
    })
}

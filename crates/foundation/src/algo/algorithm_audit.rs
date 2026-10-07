//! Static audit for algorithm-layer numeric fallbacks.
//!
//! Fails CI if forbidden `unwrap_or` / `map_or` injection patterns reappear in
//! inference modules (see `algorithm_seal` module docs).

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use walkdir::WalkDir;

    fn map_module_rel(rel: &str) -> String {
        if rel.starts_with("video_explorer/") {
            return format!("video/{rel}");
        }
        for (prefix, dir) in [
            ("algorithm_", "algo"),
            ("loop_intent", "image"),
            ("image_", "image"),
            ("jxl_", "image"),
            ("video_", "video"),
            ("gpu_accel", "video"),
            ("explore_strategy", "convert"),
            ("conversion", "convert"),
            ("analysis_cache", "convert"),
            ("media_conversion_gate", "convert"),
            ("database", "db"),
            ("multi_scenario_db", "db"),
            ("scenario", "db"),
            ("quality_", "quality"),
            ("ssim_mapping", "quality"),
            ("hdr", "media"),
            ("logging", "infra"),
            ("training_", "train"),
            ("c_api", "train"),
        ] {
            if rel.starts_with(prefix) {
                return format!("{dir}/{rel}");
            }
        }
        rel.to_string()
    }

    fn resolve_src_path(src_root: &std::path::Path, rel: &str) -> PathBuf {
        let direct = src_root.join(rel);
        if direct.is_file() {
            return direct;
        }
        let mapped = src_root.join(map_module_rel(rel));
        if mapped.is_file() {
            return mapped;
        }
        if let Some(name) = std::path::Path::new(rel).file_name()
            && let Some(found) = WalkDir::new(src_root)
                .into_iter()
                .filter_map(Result::ok)
                .map(walkdir::DirEntry::into_path)
                .find(|p| p.is_file() && p.file_name() == Some(name))
        {
            return found;
        }
        direct
    }

    const MODULES: &[&str] = &[
        "algorithm_seal.rs",
        "algorithm_runtime.rs",
        "loop_intent.rs",
        "database.rs",
        "image_quality_db.rs",
        "quality_regression_model.rs",
        "scenario_quality_lookup.rs",
        "video_explorer.rs",
        "video_explorer/gpu_coarse_search.rs",
        "gpu_accel.rs",
        "jxl_explorer.rs",
        "quality_matcher.rs",
        "multi_scenario_db.rs",
        "image_quality_detector.rs",
        "image_detection.rs",
        "image_jpeg_analysis.rs",
        "video_detection.rs",
        "video_quality_detector.rs",
        "explore_strategy.rs",
        "hdr.rs",
        "conversion.rs",
        "database_vector.rs",
        "animated_image_quality_features.rs",
        "video_quality_features.rs",
        "analysis_cache.rs",
        "convert/batch.rs",
        "training_tier_audit.rs",
        "c_api.rs",
        "logging.rs",
        "media_conversion_gate.rs",
        "scenario.rs",
        "video_explorer/stream_analysis.rs",
        "video_explorer/precision.rs",
        "image_metrics.rs",
        "image_analyzer.rs",
        "quality_verifier_enhanced.rs",
    ];

    /// Sources that may reference algorithm symbols but are not
    /// substring-audited here.
    const AUDIT_EXEMPT: &[&str] = &["lib.rs", "metadata/macos.rs"];

    const FORBIDDEN_SUBSTRINGS: &[&str] = &[
        "unwrap_or(0.5",
        "unwrap_or(f64::NAN",
        "unwrap_or(knn",
        "unwrap_or_else(|| 0.5",
        "unwrap_or_else(|| 0.0",
        "unwrap_or_else(|| {\n            crate::constants::LOOP_INTENT_DEFAULT",
        "_probability_raw",
        "_finite_scalar_raw",
        "0.5 neutral prior",
        "using 0.5 neutral",
        "preserving raw",
        "LOOP_INTENT_DEFAULT_KNN_CONFIDENCE",
        "seal_unit_probability_or",
        "quality_probability_or",
        "confidence: 0.0,",
        "confidence: 0.5,",
        "map_or(0.5,",
        "Verdict::LoopStrong(_) => Some(1.0)",
        "Verdict::LoopWeak(_) | Verdict::Error(_) => Some(0.0)",
        "return Some(1.0);",
        "return Some(0.0);",
        "confidence: Some(0.85)",
        "confidence: Some(0.75)",
        "confidence: Some(0.7)",
        "confidence: Some(0.65)",
        "confidence: Some(0.6)",
        "confidence: Some(0.3)",
        "confidence: Some(0.8)",
        "confidence: Some(0.9)",
        "confidence: Some(0.95)",
        "confidence: Some(1.0)",
        "EXPLORE_CONFIDENCE_HIGH",
        "EXPLORE_CONFIDENCE_NORMAL",
        "EXPLORE_CONFIDENCE_MEDIUM",
        "EXPLORE_CONFIDENCE_LOW",
        "unwrap_or(1.0_f64)",
        "unwrap_or(1.0)",
    ];

    #[test]
    fn algorithm_modules_reject_forbidden_numeric_fallbacks() {
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        for module in MODULES {
            let path = resolve_src_path(&src_root, module);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for pattern in FORBIDDEN_SUBSTRINGS {
                if content.contains(pattern) {
                    violations.push(format!("{module}: contains `{pattern}`"));
                }
            }
        }
        assert!(
            violations.is_empty(),
            "algorithm audit failed:\n{}",
            violations.join("\n")
        );
    }

    fn line_is_comment_or_doc(trimmed: &str) -> bool {
        trimmed.starts_with("//")
            || trimmed.starts_with("///")
            || trimmed.starts_with('*')
            || trimmed.starts_with("/*")
    }

    /// Runtime gates must not read legacy `MODERN_FORMAT_ENABLE_*` algorithm
    /// env keys.
    #[test]
    fn algorithm_runtime_rejects_legacy_enable_env_keys() {
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let path = resolve_src_path(&src_root, "algorithm_runtime.rs");
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for (line_no, line) in content.lines().enumerate() {
            if line_is_comment_or_doc(line.trim()) {
                continue;
            }
            assert!(
                !line.contains("constants::ENV_ENABLE_"),
                "algorithm_runtime.rs:{} must not gate on legacy ENV_ENABLE_*: {line}",
                line_no + 1
            );
        }
    }

    /// Audited inference modules must route gates through `algorithm_runtime`
    /// (`DISABLE_*`), not legacy `ENABLE_*`.
    #[test]
    fn algorithm_modules_reject_legacy_enable_env_reads() {
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        for module in MODULES {
            let path = resolve_src_path(&src_root, module);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            for (line_no, line) in content.lines().enumerate() {
                if line_is_comment_or_doc(line.trim()) {
                    continue;
                }
                if line.contains("ENV_ENABLE_") || line.contains("MODERN_FORMAT_ENABLE_") {
                    violations.push(format!(
                        "{module}:{} references legacy ENABLE env: {}",
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        }
        assert!(
            violations.is_empty(),
            "legacy ENABLE env reads in algorithm modules:\n{}",
            violations.join("\n")
        );
    }

    const ENV_MUTATION_TEST_FILES: &[&str] = &[
        "loop_intent.rs",
        "database.rs",
        "gpu_accel.rs",
        "quality_matcher.rs",
        "multi_scenario_db.rs",
        "image_quality_db.rs",
        "metadata/exif.rs",
        "checkpoint.rs",
        "process_lock.rs",
        "common_utils.rs",
        "tests/video_detection.rs",
    ];

    /// Every `src/**/*.rs` file that calls `algorithm_runtime` /
    /// `algorithm_seal` must be listed in `MODULES` or `AUDIT_EXEMPT`.
    #[test]
    fn algorithm_audit_modules_cover_runtime_callers() {
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut uncovered = Vec::new();
        for entry in walkdir::WalkDir::new(&src_root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
            })
        {
            let path = entry.path();
            let rel = path
                .strip_prefix(&src_root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            let canonical_rel = if let Some(tail) = rel.strip_prefix("video/video_explorer/") {
                format!("video_explorer/{tail}")
            } else {
                rel.rsplit('/').next().unwrap_or(&rel).to_string()
            };
            if rel.starts_with("bin/")
                || rel.starts_with("tests/")
                || rel.ends_with("algorithm_audit.rs")
                || matches!(
                    rel.as_str(),
                    "metadata/macos.rs"
                        | "metadata/linux.rs"
                        | "metadata/windows.rs"
                        | "metadata/network.rs"
                )
            {
                continue;
            }
            let content = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let uses_algorithm =
                content.contains("algorithm_runtime::") || content.contains("algorithm_seal::");
            if !uses_algorithm {
                continue;
            }
            let covered = MODULES.contains(&rel.as_str())
                || MODULES.contains(&canonical_rel.as_str())
                || AUDIT_EXEMPT.contains(&rel.as_str())
                || AUDIT_EXEMPT.contains(&canonical_rel.as_str());
            if !covered {
                uncovered.push(rel);
            }
        }
        assert!(
            uncovered.is_empty(),
            "algorithm_runtime/algorithm_seal callers must be in MODULES or AUDIT_EXEMPT:\n{}",
            uncovered.join("\n")
        );
    }

    /// Files that mutate process env in tests must use `serial_test` or an
    /// internal mutex.
    #[test]
    fn env_mutation_test_modules_declare_serial_isolation() {
        let src_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        for module in ENV_MUTATION_TEST_FILES {
            let path = resolve_src_path(&src_root, module);
            let content = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let mutates_env = content.contains("EnvGuard::set")
                || content.contains("std::env::set_var")
                || content.contains("std::env::remove_var");
            if !mutates_env {
                continue;
            }
            let isolated = content.contains("serial_test::serial")
                || content.contains("#[serial]")
                || content.contains("_ENV_LOCK")
                || content.contains("ENV_LOCK:")
                || content.contains("TEST_LOCK")
                || content.contains("LOOP_INTENT_ENV_LOCK");
            if !isolated {
                violations.push(format!(
                    "{module}: mutates env in tests without serial_test or ENV_LOCK"
                ));
            }
        }
        assert!(
            violations.is_empty(),
            "env isolation audit failed:\n{}",
            violations.join("\n")
        );
    }
}

use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|err| panic!("failed to resolve workspace root: {err:?}"))
}

fn production_rust_files(root: &Path) -> Vec<PathBuf> {
    [
        "crates/shared_utils/src",
        "crates/img/src",
        "crates/vid/src",
    ]
    .into_iter()
    .flat_map(|dir| {
        WalkDir::new(root.join(dir))
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == std::ffi::OsStr::new("rs"))
            })
            .map(|entry| entry.path().to_path_buf())
    })
    .collect()
}

fn offending_lines(root: &Path, files: &[PathBuf], patterns: &[&str]) -> Vec<String> {
    let mut offenders = Vec::new();
    for file in files {
        let content = fs::read_to_string(file)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", file.display()));
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if patterns.iter().any(|pattern| line.contains(pattern)) {
                // Skip matches that are likely inside unit tests or test modules by
                // scanning a small window of previous lines for test annotations.
                let start = idx.saturating_sub(20);
                let in_test_context = lines[start..idx].iter().any(|l|
                    l.contains("#[test]")
                        || l.contains("#[cfg(test)]")
                        || l.contains("mod tests")
                        || l.contains("proptest!")
                        || l.trim_start().starts_with("fn test_")
                );
                if in_test_context {
                    continue;
                }
                let rel = file.strip_prefix(root).unwrap_or(file);
                offenders.push(format!("{}:{}: {}", rel.display(), idx + 1, line.trim()));
            }
        }
    }
    offenders
}

#[test]
fn production_code_has_no_numeric_forgery_fallbacks() {
    let root = workspace_root();
    let files = production_rust_files(&root);
    let offenders = offending_lines(
        &root,
        &files,
        &[
            "unwrap_or(0)",
            "unwrap_or(0.0)",
            "unwrap_or(&0.0",
            "unwrap_or(&0.0_f64",
            "unwrap_or(0usize)",
            "unwrap_or(0u32)",
            "unwrap_or(0u64)",
            "unwrap_or(1)",
            "unwrap_or(1.0)",
            "unwrap_or(0.5)",
            "unwrap_or(85)",
            "unwrap_or(35)",
            "unwrap_or(0x",
            "unwrap_or(u16::MAX",
            "unwrap_or(usize::MAX",
        ],
    );

    assert!(
        offenders.is_empty(),
        "numeric metadata must not be forged with 0/1 defaults:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn release_workflow_does_not_publish_partial_artifacts() {
    let root = workspace_root();
    let release = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow must be readable");

    for forbidden in [
        "continue-on-error: ${{ matrix.optional == true }}",
        "if: always() && !cancelled()",
        "fail_on_unmatched_files: false",
    ] {
        assert!(
            !release.contains(forbidden),
            "release workflow still contains partial-success pattern: {forbidden}"
        );
    }
}

#[test]
fn dependency_installation_is_not_silenced_in_release_workflows() {
    let root = workspace_root();
    for workflow in [
        ".github/workflows/release.yml",
        ".github/workflows/nightly-release.yml",
    ] {
        let content = fs::read_to_string(root.join(workflow))
            .unwrap_or_else(|err| panic!("read {workflow}: {err:?}"));
        let offenders: Vec<_> = content
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains("brew install") && line.contains("|| true"))
            .map(|(idx, line)| format!("{workflow}:{}: {}", idx + 1, line.trim()))
            .collect();
        assert!(
            offenders.is_empty(),
            "release dependency installation must fail loudly:\n{}",
            offenders.join("\n")
        );
    }
}

#[test]
fn release_packaging_does_not_swallow_copy_failures() {
    let root = workspace_root();
    let release = fs::read_to_string(root.join(".github/workflows/release.yml"))
        .expect("release workflow must be readable");
    let offenders: Vec<_> = release
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("cp ") && line.contains("|| true"))
        .map(|(idx, line)| format!(".github/workflows/release.yml:{}: {}", idx + 1, line.trim()))
        .collect();

    assert!(
        offenders.is_empty(),
        "release packaging copy steps must fail loudly:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn obsolete_blocking_exit_guard_is_not_present() {
    let root = workspace_root();
    assert!(
        !root.join("scripts/terminal_exit_guard.py").exists(),
        "terminal_exit_guard.py reintroduces blocking GUI exit confirmation"
    );
    assert!(
        !root.join(".tmp_lib/libstdc++.tbd").exists(),
        "tracked .tmp_lib stubs are CI scratch artifacts, not source"
    );
}

#[test]
fn audit_tests_are_real_harness_tests() {
    let root = workspace_root();
    for audit_file in [
        "crates/dev/tests/test_real_silent_fallbacks.rs",
        "crates/dev/tests/test_silent_numeric_fallbacks.rs",
    ] {
        let content = fs::read_to_string(root.join(audit_file))
            .unwrap_or_else(|err| panic!("read {audit_file}: {err:?}"));
        assert!(
            content.contains("#[test]"),
            "{audit_file} must contain real Cargo test functions"
        );
        let old_always_passes_phrase = ["always", " passes"].concat();
        let old_report_only_phrase = ["check output", " for details"].concat();
        assert!(
            !content.contains(&old_always_passes_phrase)
                && !content.contains(&old_report_only_phrase),
            "{audit_file} must not be a report-only pseudo-test"
        );
    }
}

#[test]
fn dev_test_targets_are_not_zero_test_placeholders() {
    let root = workspace_root();
    let test_dir = root.join("crates/dev/tests");
    let mut offenders = Vec::new();

    for entry in fs::read_dir(&test_dir).expect("dev tests directory must be readable") {
        let entry = entry.expect("dev test directory entry must be readable");
        let path = entry.path();
        if path
            .extension()
            .is_none_or(|ext| ext != std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let content = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err:?}", path.display()));
        if !content.contains("#[test]") && !content.contains("proptest!") {
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            offenders.push(rel.display().to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "dev integration test targets must contain real tests or move to src/bin:\n{}",
        offenders.join("\n")
    );
}

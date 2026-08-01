use foundation::anyhow::Result;
use foundation::cli_runner::{CliProcessingResult, Config as CliRunnerConfig, run_auto_command};
use foundation::unified_error::UnifiedError;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[derive(Debug)]
struct MockResult;

impl CliProcessingResult for MockResult {
    fn is_skipped(&self) -> bool {
        false
    }
    fn is_success(&self) -> bool {
        true
    }
    fn skip_reason(&self) -> Option<&str> {
        None
    }
    fn input_path(&self) -> &'static str {
        ""
    }
    fn output_path(&self) -> Option<&str> {
        None
    }
    fn input_size(&self) -> u64 {
        0
    }
    fn output_size(&self) -> Option<u64> {
        None
    }
    fn message(&self) -> &'static str {
        ""
    }
    fn blake3(&self) -> Option<&str> {
        None
    }
}

#[test]
fn smoke_semantic_integrity_skips_vs_errors() -> Result<()> {
    // Tiny synthetic fixtures; disk headroom preflight is out of scope for this semantic test.
    unsafe { std::env::set_var(foundation::constants::ENV_MFB_SKIP_DISK_PRECHECK, "1") };

    let input_dir = tempdir()?;
    let output_dir = tempdir()?;
    let input_dir_path = input_dir.path().canonicalize()?;
    let output_dir_path = output_dir.path().canonicalize()?;

    // Create two test files with valid MP4 headers (ftyp) so they are collected by the runner
    let valid_mp4_header = b"\x00\x00\x00\x18ftypisom\x00\x00\x00\x00isomiso2avc1mp41";

    let file1_path = input_dir_path.join("skip_me.mp4");
    fs::write(&file1_path, valid_mp4_header)?;

    let file2_path = input_dir_path.join("error_me.mp4");
    fs::write(&file2_path, valid_mp4_header)?;

    let config = CliRunnerConfig {
        input: input_dir_path.clone(),
        output: Some(output_dir_path),
        recursive: false,
        label: "integrity-test".to_string(),
        base_dir: Some(input_dir_path),
        resume: false,
        protect_destructive_dirs: false,
        error_mode: foundation::BatchErrorMode::LogAndContinue,
    };

    // Run batch with mock converter that injects specific error types
    let run_res = run_auto_command(&config, |path: &Path| -> Result<MockResult> {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name == "skip_me.mp4" {
            // Optimization failure (IterationLimitExceeded) -> SHOULD BE SKIPPED AND COPIED
            Err(
                UnifiedError::IterationLimitExceeded(foundation::IterationError {
                    current: 10,
                    max: 10,
                    context: "search".to_string(),
                })
                .into(),
            )
        } else {
            // Recoverable error (e.g. AnalysisError) -> SHOULD BE FAILED AND NOT COPIED
            // (We use a non-fatal error so the batch doesn't stop immediately)
            Err(UnifiedError::AnalysisError("simulated non-skip failure".to_string()).into())
        }
    });

    assert!(
        run_res.is_err(),
        "Expected run_auto_command to return Err because error_me.mp4 is a hard failure"
    );

    // Verify file1 (Skip/Optimization Failure) WAS copied to the output directory
    let copied_file1 = output_dir.path().join("skip_me.mp4");
    assert!(
        copied_file1.exists(),
        "ERROR: Optimization failures (IterationLimitExceeded) must be treated as Skips and trigger an automatic copy to the output directory. Semantic integrity violated."
    );

    // Verify file2 (Non-skip Error) WAS NOT copied to the output directory
    let copied_file2 = output_dir.path().join("error_me.mp4");
    assert!(
        !copied_file2.exists(),
        "ERROR: Hard/Recoverable errors (AnalysisError) must NOT result in a copy to the output directory. They should remain as honest errors. Semantic integrity violated."
    );

    println!(
        "✅ Semantic integrity test passed: Skips correctly copied, Errors correctly withheld."
    );

    Ok(())
}

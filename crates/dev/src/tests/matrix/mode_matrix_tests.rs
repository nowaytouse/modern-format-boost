use anyhow::{Context, Result, bail};
use foundation::common_utils::EnvGuard;
use foundation::conversion_types::{
    ConfigFlags, ConversionConfig, ConversionOutput, SelectedCodec, TargetVideoFormat,
};
use serial_test::serial;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use vid::{auto_convert_with_cache, detect_video, determine_strategy_with_apple_compat};

fn edge_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("edge")
}

fn video_dir() -> PathBuf {
    edge_dir().join("videos")
}

fn video_file(name: &str) -> PathBuf {
    let path = video_dir().join(name);
    // ensure parent dirs exist so ffmpeg can write fixture into repo test tree
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("failed to create fixture parent dirs");
    }
    if path.exists() {
        return path;
    }
    // ponytail: generate minimal fixture via ffmpeg if missing to keep tests
    // hermetic
    let duration = if name.contains("10s") {
        "10"
    } else if name.contains("8s") {
        "8"
    } else if name.contains("5s") {
        "5"
    } else if name.contains("2s") {
        "2"
    } else {
        "6"
    };

    // build ffmpeg command depending on requested codec hinted by filename
    let output = if name.ends_with(".webm") || name.contains("vp9") {
        Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000",
                "-t",
                duration,
                "-c:v",
                "libvpx-vp9",
                "-b:v",
                "0",
                "-crf",
                "30",
                "-c:a",
                "libopus",
                path.to_str().unwrap(),
            ])
            .output()
            .expect("failed to spawn ffmpeg for fixture generation")
    } else if name.contains("hevc") {
        Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000",
                "-t",
                duration,
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-x265-params",
                "crf=20",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                path.to_str().unwrap(),
            ])
            .output()
            .expect("failed to spawn ffmpeg for fixture generation")
    } else {
        Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=880:sample_rate=48000",
                "-t",
                duration,
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-crf",
                "4",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "128k",
                path.to_str().unwrap(),
            ])
            .output()
            .expect("failed to spawn ffmpeg for fixture generation")
    };

    if !output.status.success() {
        panic!(
            "failed to generate fixture {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    path
}

fn create_runtime_matrix_source() -> Result<(TempDir, PathBuf)> {
    let temp = tempfile::tempdir().context("failed to create synthetic source temp dir")?;
    let source = temp.path().join("matrix_source_h264.mp4");

    let output = Command::new("ffmpeg")
        .arg("-y")
        .arg("-v")
        .arg("error")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg("testsrc2=size=320x180:rate=30")
        .arg("-f")
        .arg("lavfi")
        .arg("-i")
        .arg("sine=frequency=880:sample_rate=48000")
        .arg("-t")
        .arg("6.2")
        .arg("-shortest")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("ultrafast")
        .arg("-crf")
        .arg("4")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("128k")
        .arg(&source)
        .output()
        .context("failed to spawn ffmpeg for synthetic mode-matrix source")?;

    if !output.status.success() {
        bail!(
            "failed to generate synthetic runtime source: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok((temp, source))
}

fn require_ffmpeg_filter(filter: &str) -> Result<()> {
    let output = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-filters")
        .output()
        .context("failed to query ffmpeg filters")?;

    if !output.status.success() {
        bail!(
            "ffmpeg -filters failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(filter))
    {
        Ok(())
    } else {
        bail!("ffmpeg lacks required {filter} filter")
    }
}

fn matrix_config(
    output_dir: &Path,
    base_dir: &Path,
    codec: SelectedCodec,
    apple_compat: bool,
    ultimate: bool,
) -> ConversionConfig {
    let mut flags = ConfigFlags::EXPLORE_SMALLER
        | ConfigFlags::MATCH_QUALITY
        | ConfigFlags::REQUIRE_COMPRESSION
        | ConfigFlags::USE_GPU
        | ConfigFlags::ALLOW_SIZE_TOLERANCE;

    if apple_compat {
        flags |= ConfigFlags::APPLE_COMPAT;
    }
    if ultimate {
        flags |= ConfigFlags::ULTIMATE_MODE;
    }

    ConversionConfig {
        output_dir: Some(output_dir.to_path_buf()),
        base_dir: Some(base_dir.to_path_buf()),
        flags,
        min_ssim: foundation::constants::MIN_SSIM_DEFAULT,
        child_threads: foundation::thread_manager::get_balanced_thread_config(
            foundation::thread_manager::WorkloadType::Video,
        )
        .child_threads,
        codec,
    }
}

fn run_case(
    input: &Path,
    codec: SelectedCodec,
    apple_compat: bool,
    ultimate: bool,
) -> Result<(TempDir, ConversionOutput)> {
    let temp = tempfile::tempdir().context("failed to create mode-matrix temp dir")?;
    let temp_root = temp.path().join("mfb_home");
    let temp_root_str = temp_root.to_string_lossy().to_string();
    let _guard = EnvGuard::set(foundation::constants::ENV_MFB_HOME_ROOT, &temp_root_str);
    let output_dir = temp.path().join("output");
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output dir {}", output_dir.display()))?;
    let base_dir = input
        .parent()
        .ok_or_else(|| anyhow::anyhow!("input has no parent directory: {}", input.display()))?;

    let config = matrix_config(&output_dir, base_dir, codec, apple_compat, ultimate);
    let output = auto_convert_with_cache(input, &config, None)
        .with_context(|| format!("conversion failed for {}", input.display()))?;
    Ok((temp, output))
}

fn assert_converted(output: &ConversionOutput, expected: TargetVideoFormat) -> Result<()> {
    if output.ignored {
        bail!("expected conversion, got ignored output: {output:?}");
    }
    if !output.success {
        bail!("expected successful conversion, got: {output:?}");
    }
    if output.strategy.target != expected {
        bail!(
            "expected target {expected:?}, got {actual:?}: {output:?}",
            actual = output.strategy.target
        );
    }
    if output.output_path.is_empty() {
        bail!("expected non-empty output path for successful conversion: {output:?}");
    }
    let output_path = Path::new(&output.output_path);
    if !output_path.exists() {
        bail!("expected output file to exist: {}", output_path.display());
    }
    if output.exploration_attempts == 0 {
        bail!("expected real exploration attempts for runtime conversion: {output:?}");
    }
    Ok(())
}

fn assert_truthful_skip(output: &ConversionOutput) -> Result<()> {
    if output.ignored {
        bail!("expected skip, got ignored output: {output:?}");
    }
    if output.strategy.target != TargetVideoFormat::Skip {
        bail!(
            "expected skip target, got {:?}: {output:?}",
            output.strategy.target
        );
    }
    if output.exploration_attempts == 0 {
        bail!("expected skip to come from a real explored path: {output:?}");
    }
    if !output.message.to_ascii_lowercase().contains("skipped") {
        bail!("skip output should say it skipped honestly: {output:?}");
    }
    Ok(())
}

fn assert_honest_hevc_ultimate_result(output: &ConversionOutput) -> Result<()> {
    match output.strategy.target {
        TargetVideoFormat::HevcMp4 => assert_converted(output, TargetVideoFormat::HevcMp4),
        TargetVideoFormat::Skip => {
            assert_truthful_skip(output)?;
            let reason_lower = output.strategy.reason.to_ascii_lowercase();
            if !reason_lower.contains("quality gate failed")
                && !reason_lower.contains("quality validation failed")
            {
                bail!(
                    "expected ultimate HEVC skip to disclose quality-gate reason honestly: \
                     {output:?}"
                );
            }
            Ok(())
        }
        actual => bail!("unexpected ultimate HEVC runtime outcome {actual:?}: {output:?}"),
    }
}

#[test]
#[serial]
fn real_strategy_matrix_covers_supported_mode_routes() -> Result<()> {
    struct Case {
        file: &'static str,
        codec: SelectedCodec,
        apple_compat: bool,
        expected: TargetVideoFormat,
    }

    let cases = [
        Case {
            file: "test_h264_10s.mp4",
            codec: SelectedCodec::Hevc,
            apple_compat: false,
            expected: TargetVideoFormat::HevcMp4,
        },
        Case {
            file: "test_h264_10s.mp4",
            codec: SelectedCodec::Hevc,
            apple_compat: true,
            expected: TargetVideoFormat::HevcMov,
        },
        Case {
            file: "test_h264_10s.mp4",
            codec: SelectedCodec::Av1,
            apple_compat: false,
            expected: TargetVideoFormat::Av1Mp4,
        },
        Case {
            file: "test_vp9_5s.webm",
            codec: SelectedCodec::Hevc,
            apple_compat: false,
            expected: TargetVideoFormat::Skip,
        },
        Case {
            file: "test_hevc_8s.mp4",
            codec: SelectedCodec::Hevc,
            apple_compat: false,
            expected: TargetVideoFormat::Skip,
        },
        Case {
            file: "test_hevc_8s.mp4",
            codec: SelectedCodec::Hevc,
            apple_compat: true,
            expected: TargetVideoFormat::Skip,
        },
        Case {
            file: "test_short_2s.mp4",
            codec: SelectedCodec::Hevc,
            apple_compat: false,
            expected: TargetVideoFormat::HevcMp4,
        },
    ];

    for case in cases {
        let input = video_file(case.file);
        let detection = detect_video(&input)
            .with_context(|| format!("failed to detect {}", input.display()))?;
        let strategy = determine_strategy_with_apple_compat(
            &detection,
            &input,
            case.apple_compat,
            false,
            case.codec,
        );
        assert_eq!(
            strategy.target, case.expected,
            "route mismatch for {} codec={:?} apple_compat={}",
            case.file, case.codec, case.apple_compat
        );
    }

    Ok(())
}

#[test]
#[serial]
fn runtime_matrix_executes_generated_h264_across_supported_modes() -> Result<()> {
    enum ExpectedOutcome {
        Converted(TargetVideoFormat),
        HevcUltimateHonest,
    }

    struct Case {
        codec: SelectedCodec,
        apple_compat: bool,
        ultimate: bool,
        expected: ExpectedOutcome,
    }

    require_ffmpeg_filter("libvmaf")?;

    let (_source_temp, input) = create_runtime_matrix_source()?;
    let cases = [
        Case {
            codec: SelectedCodec::Hevc,
            apple_compat: false,
            ultimate: false,
            expected: ExpectedOutcome::Converted(TargetVideoFormat::HevcMp4),
        },
        Case {
            codec: SelectedCodec::Hevc,
            apple_compat: false,
            ultimate: true,
            expected: ExpectedOutcome::HevcUltimateHonest,
        },
        Case {
            codec: SelectedCodec::Hevc,
            apple_compat: true,
            ultimate: false,
            expected: ExpectedOutcome::Converted(TargetVideoFormat::HevcMov),
        },
        Case {
            codec: SelectedCodec::Av1,
            apple_compat: false,
            ultimate: false,
            expected: ExpectedOutcome::Converted(TargetVideoFormat::Av1Mp4),
        },
    ];

    for case in cases {
        let (_temp, output) = run_case(&input, case.codec, case.apple_compat, case.ultimate)?;
        match case.expected {
            ExpectedOutcome::Converted(target) => {
                assert_converted(&output, target).with_context(|| {
                    format!(
                        "runtime conversion failed for codec={:?} apple_compat={} ultimate={}",
                        case.codec, case.apple_compat, case.ultimate
                    )
                })?;
            }
            ExpectedOutcome::HevcUltimateHonest => {
                assert_honest_hevc_ultimate_result(&output).with_context(|| {
                    format!(
                        "runtime HEVC ultimate semantics failed for codec={:?} apple_compat={} \
                         ultimate={}",
                        case.codec, case.apple_compat, case.ultimate
                    )
                })?;
            }
        }
    }

    Ok(())
}

#[test]
#[serial]
fn runtime_matrix_handles_hevc_skip_honestly() -> Result<()> {
    let input = video_file("test_hevc_8s.mp4");

    let (skip_temp, skipped) = run_case(&input, SelectedCodec::Hevc, false, false)?;
    assert!(
        skipped.success,
        "skip should still be a truthful success: {skipped:?}"
    );
    assert_eq!(skipped.strategy.target, TargetVideoFormat::Skip);
    assert!(
        skipped.output_path.is_empty(),
        "skip should not forge output path"
    );
    let copied = skip_temp.path().join("output").join("test_hevc_8s.mp4");
    assert!(
        copied.exists(),
        "skip should preserve the original in output tree: {}",
        copied.display()
    );
    Ok(())
}

#[test]
#[serial]
fn hevc_skip_fixture_metadata_copy_succeeds() -> Result<()> {
    let input = video_file("test_hevc_8s.mp4");
    let temp = tempfile::tempdir().context("failed to create metadata-copy temp dir")?;
    let copied = temp.path().join("test_hevc_8s.mp4");
    std::fs::copy(&input, &copied).with_context(|| {
        format!(
            "failed to prepare metadata-copy destination {}",
            copied.display()
        )
    })?;

    foundation::metadata::copy(&input, &copied).with_context(|| {
        format!(
            "metadata copy failed for skipped HEVC fixture {} -> {}",
            input.display(),
            copied.display()
        )
    })?;

    Ok(())
}

#[test]
#[serial]
fn library_rejects_av1_apple_compat_as_error_instead_of_exiting() {
    let input = video_file("test_short_2s.mp4");
    let temp = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir failed: {e}"));
    let temp_root = temp.path().join("mfb_home");
    let temp_root_str = temp_root.to_string_lossy().to_string();
    let _guard = EnvGuard::set(foundation::constants::ENV_MFB_HOME_ROOT, &temp_root_str);
    let base_dir = input
        .parent()
        .unwrap_or_else(|| panic!("input has no parent: {}", input.display()));
    let config = matrix_config(temp.path(), base_dir, SelectedCodec::Av1, true, false);

    let err = auto_convert_with_cache(&input, &config, None)
        .expect_err("library should return Err for AV1 + apple-compat");

    let message = err.to_string();
    assert!(
        message.contains("AV1 strategy does not support Apple compatibility"),
        "unexpected error message: {message}"
    );
}

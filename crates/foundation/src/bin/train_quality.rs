use anyhow::{Context, Result};
use clap::Parser;
use foundation::modern_ui::symbols;
use foundation::multi_scenario_db::ScenarioSample;
use foundation::scenario::ScenarioType;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(name = "train_quality")]
#[command(
    about = "Ingest quality regression training samples for images, animated images, or videos",
    long_about = None
)]
struct Cli {
    /// Directory containing sample media
    input: PathBuf,

    /// Semantic quality tier for these samples (`high` or `low`)
    ///
    /// For `image_quality`, the storage family (`png-*` vs `modern-*`) is
    /// resolved from the detected asset format during ingestion.
    #[arg(short, long)]
    #[arg(required = true)] // 🛡️ HARDENED: Quality label is mandatory for valid training data
    label: Option<String>,

    /// Target scenario: `image_quality` (default), `animated_image_quality`, or
    /// `video_quality`
    #[arg(long, default_value = "image_quality")]
    scenario: ScenarioType,

    /// `PostgreSQL` connection string
    #[arg(short, long)]
    conn: Option<String>,

    /// Permit a zero exit when some candidate files failed ingestion
    #[arg(long)]
    allow_partial: bool,
}

#[allow(clippy::too_many_lines)]
fn main() -> Result<()> {
    foundation::training_entry_guard::assert_train_quality_entry()
        .context("train_quality entry guard")?;
    foundation::progress_mode::configure_terminal_ux(false);
    let cli = Cli::parse();

    // 🛡️ FAIL FAST: LoopIntent is not a quality regression scenario.
    if !cli.scenario.is_quality_regression() {
        anyhow::bail!(
            "{} belongs to the {} task family. Use train_knn for loop clustering instead.",
            cli.scenario,
            cli.scenario.task_family()
        );
    }

    let conn = foundation::media_conversion_gate::delivery_training_pg_connstr_or_default(cli.conn);

    let mut client = foundation::database::connect_pg_with_str(&conn)
        .context("Failed to connect to PostgreSQL")?;

    foundation::ui_stderr::line(
        symbols::PALETTE,
        symbols::plain::PALETTE,
        "Ingesting Quality Regression Samples...",
    );
    foundation::ui_stderr::line(
        symbols::FOLDER,
        symbols::plain::FOLDER,
        format!("Input: {}", cli.input.display()),
    );
    foundation::ui_stderr::line(
        symbols::TARGET,
        symbols::plain::TARGET,
        format!(
            "Scenario: {} [{}]",
            cli.scenario.description(),
            cli.scenario.task_family()
        ),
    );
    if let Some(label) = cli.label.as_deref() {
        foundation::ui_stderr::line(
            symbols::LABEL_TAG,
            symbols::plain::LABEL_TAG,
            format!("Label: {label}"),
        );
    }

    foundation::multi_scenario_db::init_multi_scenario_schema(&mut client)?;

    // 🛡️ PRE-LOOP VALIDATION: Resolve the quality tier once, then let the
    // shared ingestion layer canonicalize image-family labels from real media.
    let label = cli
        .label
        .ok_or_else(|| anyhow::anyhow!("BUG: label must be provided (enforced by clap)"))?;
    let quality_tier = foundation::scenario::QualityTier::parse_strict(label.as_str())?;
    if cli.scenario != ScenarioType::ImageQuality
        && foundation::scenario::ImageQualityLabel::from_label(label.as_str()).is_some()
    {
        anyhow::bail!(
            "{} only accepts generic labels `high`/`low`; image-family labels belong to \
             image_quality",
            cli.scenario
        );
    }
    let quality_score = quality_tier.to_score();

    let supported_extensions = match cli.scenario {
        ScenarioType::ImageQuality => vec![
            "jpg", "jpeg", "jpe", "png", "webp", "tiff", "tif", "bmp", "ico", "avif", "heic",
            "heif", "hif", "jxl",
        ],
        ScenarioType::AnimatedImageQuality => {
            vec![
                "gif", "apng", "png", "webp", "avif", "heic", "heif", "hif", "jxl",
            ]
        }
        ScenarioType::VideoQuality => vec![
            "mp4", "mov", "avi", "mkv", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "ts", "mts",
            "m2ts", "m2v", "3gp", "3g2", "ogv", "f4v", "asf", "vob", "svi", "m2p", "m2t", "tp",
            "trp", "divx", "xvid", "rm", "rmvb", "amv", "nsv", "roq", "mxf", "dv", "drc",
        ],
        ScenarioType::LoopIntent => unreachable!(),
    };

    let input_paths = collect_input_files(&cli.input)?;

    let mut stats = IngestStats::default();
    let mut failures = Vec::new();
    for path in input_paths {
        stats.scanned += 1;
        let ext = detect_media_extension(&path);
        let stored_source_path = foundation::common_utils::training_source_path_for(&path);

        if !supported_extensions.contains(&ext.as_str()) {
            stats.skipped_unsupported += 1;
            continue;
        }
        stats.candidates += 1;

        if cli.scenario == ScenarioType::ImageQuality
            && let Err(e) = reject_animated_image_quality_asset(&path)
        {
            record_ingest_failure(&mut stats, &mut failures, &path, &e);
            continue;
        }

        let blake3_hash = match foundation::common_utils::calculate_blake3_hash_bytes(&path) {
            Ok(h) => h,
            Err(e) => {
                record_ingest_failure(&mut stats, &mut failures, &path, &e);
                continue;
            }
        };

        // 🛡️ ENCAPSULATED RECOVERY: Closure to handle individual file failures
        let res: Result<()> = (|| -> Result<()> {
            match cli.scenario {
                ScenarioType::ImageQuality => {
                    use foundation::image_analyzer::analyze_image;
                    let analysis = analyze_image(&path).context("Analysis failed")?;
                    let embedding = foundation::image_quality_db::get_quality_features(&analysis)?;

                    let mut sample = ScenarioSample::new(blake3_hash, cli.scenario)
                        .with_path(stored_source_path.to_string_lossy().to_string())
                        .with_label(label.clone())
                        .with_embedding(embedding)
                        .with_dimensions(
                            i32::try_from(analysis.width).context("Image width exceeds i32")?,
                            i32::try_from(analysis.height).context("Image height exceeds i32")?,
                        )
                        .with_size(
                            i64::try_from(analysis.file_size).context("Image size exceeds i64")?,
                        )
                        .with_format(analysis.format.clone())
                        .with_entropy(analysis.features.entropy)
                        .with_compression_ratio(analysis.features.compression_ratio)
                        .with_lossless(analysis.is_lossless)
                        .with_quality_score(quality_score);
                    sample.metadata =
                        foundation::image_quality_db::build_image_quality_ingest_metadata(
                            &analysis,
                            Some(label.as_str()),
                            &path,
                        )?;

                    foundation::multi_scenario_db::ingest_image_quality_sample(&mut client, &sample)
                }
                ScenarioType::AnimatedImageQuality => {
                    use foundation::animated_image_quality_features::AnimatedImageQualityFeatures;
                    let features = AnimatedImageQualityFeatures::from_path(&path)?;
                    let vec_data = features.to_embedding_vector();
                    if vec_data.len() != cli.scenario.embedding_dimension() {
                        anyhow::bail!(
                            "Animated-image embedding dim {} != expected {}",
                            vec_data.len(),
                            cli.scenario.embedding_dimension()
                        );
                    }

                    let mut sample = ScenarioSample::new(blake3_hash, cli.scenario)
                        .with_path(stored_source_path.to_string_lossy().to_string())
                        .with_format(features.format.as_str().to_string())
                        .with_embedding(pgvector::Vector::from(vec_data))
                        .with_dimensions(
                            i32::try_from(features.width)
                                .context("Animated image width exceeds i32")?,
                            i32::try_from(features.height)
                                .context("Animated image height exceeds i32")?,
                        )
                        .with_size(
                            i64::try_from(features.file_size_bytes)
                                .context("Animated image size exceeds i64")?,
                        )
                        .with_frame_count(i64::from(features.frame_count))
                        .with_duration_secs(features.duration_secs)
                        .with_fps(features.fps)
                        .with_animation_smoothness(features.animation_smoothness)
                        .with_frame_delay_variation(features.frame_delay_variation)
                        .with_is_meme(features.content_flags.is_meme_suspected)
                        .with_quality_score(quality_score); // 🛡️ RESTORED: Map label to float score
                    if let Some(palette_size) = features.palette_size {
                        sample = sample.with_palette_size(i32::from(palette_size));
                    }
                    if let Some(palette_depth) = features.palette_depth {
                        sample = sample.with_palette_depth(palette_depth);
                    }
                    sample.metadata = serde_json::json!({
                        "scenario_semantics": "animated_image_quality",
                        "storage_table": "animated_image_quality_samples",
                        "container_format": features.format.as_str(),
                        "has_alpha": features.render_flags.has_alpha,
                        "is_lossless": features.render_flags.is_lossless,
                        "reference_entropy": features.reference_entropy
                    });

                    foundation::multi_scenario_db::ingest_animated_image_quality_sample(
                        &mut client,
                        &sample,
                    )
                }
                ScenarioType::VideoQuality => {
                    use foundation::video_quality_features::VideoQualityFeatures;
                    let features = VideoQualityFeatures::from_path(&path)?;
                    let vec_data = features.to_embedding_vector();
                    if vec_data.len() != cli.scenario.embedding_dimension() {
                        anyhow::bail!(
                            "Video embedding dim {} != expected {}",
                            vec_data.len(),
                            cli.scenario.embedding_dimension()
                        );
                    }

                    let sample = ScenarioSample::new(blake3_hash, cli.scenario)
                        .with_path(stored_source_path.to_string_lossy().to_string())
                        .with_embedding(pgvector::Vector::from(vec_data))
                        .with_dimensions(
                            i32::try_from(features.width).context("Video width exceeds i32")?,
                            i32::try_from(features.height).context("Video height exceeds i32")?,
                        )
                        .with_size(
                            i64::try_from(features.file_size_bytes)
                                .context("Video size exceeds i64")?,
                        )
                        .with_frame_count(
                            i64::try_from(features.frame_count)
                                .context("Video frame count exceeds i64")?,
                        )
                        .with_format(features.codec.clone())
                        .with_duration_secs(features.duration_secs)
                        .with_fps(features.fps)
                        .with_bitrate_mbps(features.bitrate_mbps)
                        .with_bit_depth_opt(features.bit_depth)
                        .with_has_audio(features.has_audio)
                        .with_is_variable_frame_rate(features.is_variable_frame_rate)
                        .with_is_hdr(features.is_hdr)
                        .with_motion_intensity(features.motion_intensity)
                        .with_temporal_stability(features.temporal_stability)
                        .with_quality_score(quality_score); // 🛡️ RESTORED: Map label to float score
                    let mut sample = sample;
                    sample.metadata = serde_json::json!({
                        "scenario_semantics": "video_quality",
                        "container_codec": features.codec,
                        "bit_depth": features.bit_depth,
                        "has_audio": features.has_audio,
                        "is_variable_frame_rate": features.is_variable_frame_rate,
                        "is_hdr": features.is_hdr
                    });
                    foundation::multi_scenario_db::ingest_video_quality_sample(&mut client, &sample)
                }
                ScenarioType::LoopIntent => unreachable!(),
            }
        })();

        if let Err(e) = res {
            record_ingest_failure(&mut stats, &mut failures, &path, &e);
        } else {
            stats.ingested += 1;
        }
    }

    foundation::ui_stderr::line(
        symbols::SUCCESS,
        symbols::plain::SUCCESS,
        format!(
            "Finished scan: ingested {} / {} candidate samples ({} scanned, {} unsupported \
             skipped, {} failed).",
            stats.ingested,
            stats.candidates,
            stats.scanned,
            stats.skipped_unsupported,
            stats.failed
        ),
    );

    if stats.ingested == 0 {
        anyhow::bail!(
            "No {} samples were ingested. scanned={}, candidates={}, unsupported_skipped={}, \
             failed={}.{}",
            cli.scenario,
            stats.scanned,
            stats.candidates,
            stats.skipped_unsupported,
            stats.failed,
            format_failure_summary(&failures)
        );
    }
    if stats.failed > 0 && !cli.allow_partial {
        anyhow::bail!(
            "{} candidate(s) failed during {} ingestion; {} sample(s) were inserted, but the \
             command refuses to report clean success without --allow-partial.{}",
            stats.failed,
            cli.scenario,
            stats.ingested,
            format_failure_summary(&failures)
        );
    }

    if cli.scenario == ScenarioType::ImageQuality && stats.ingested > 0 {
        foundation::ui_stderr::line(
            symbols::BRAIN,
            symbols::plain::BRAIN,
            "Next: run `cargo run --locked -p dev --bin training_pipeline -- \
             train-image-quality-model` once both high and low image_quality samples are populated.",
        );
    }
    Ok(())
}

#[derive(Default)]
struct IngestStats {
    scanned: usize,
    candidates: usize,
    skipped_unsupported: usize,
    ingested: usize,
    failed: usize,
}

fn record_ingest_failure(
    stats: &mut IngestStats,
    failures: &mut Vec<String>,
    path: &Path,
    error: &anyhow::Error,
) {
    stats.failed += 1;
    emit_ingest_error_chain(path, error);
    failures.push(format!("{}: {error}", path.display()));
}

fn emit_ingest_error_chain(path: &Path, error: &anyhow::Error) {
    use std::fmt::Write as _;

    let icon = foundation::modern_ui::symbols::pick(
        foundation::modern_ui::symbols::WARNING,
        foundation::modern_ui::symbols::plain::WARNING,
    );
    let mut message = format!("{icon} Failed to ingest {}: {}", path.display(), error);
    for cause in error.chain().skip(1) {
        let _ = write!(message, "\n    Caused by: {cause}");
    }
    foundation::progress_mode::emit_stderr(&message);
}

fn format_failure_summary(failures: &[String]) -> String {
    if failures.is_empty() {
        return String::new();
    }

    let mut summary = String::from("\nFailures:");
    for failure in failures.iter().take(5) {
        summary.push_str("\n    - ");
        summary.push_str(failure);
    }
    if failures.len() > 5 {
        use std::fmt::Write as _;
        let _ = write!(summary, "\n    - ... {} more", failures.len() - 5);
    }
    summary
}

fn collect_input_files(input: &Path) -> Result<Vec<PathBuf>> {
    if input.is_file() {
        return Ok(vec![input.to_path_buf()]);
    }
    if !input.is_dir() {
        anyhow::bail!(
            "Input path is neither a file nor a directory: {}",
            input.display()
        );
    }

    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(input) {
        let entry =
            entry.with_context(|| format!("Failed to walk input directory {}", input.display()))?;
        if entry.file_type().is_file() {
            files.push(entry.into_path());
        }
    }
    Ok(files)
}

fn detect_media_extension(path: &Path) -> String {
    if let Some(ext) = foundation::common_utils::detect_real_extension(path) {
        return ext.to_string();
    }

    match infer::get_from_path(path) {
        Ok(Some(kind)) => match kind.mime_type() {
            "image/jpeg" => return "jpeg".to_string(),
            "image/png" => return "png".to_string(),
            "image/gif" => return "gif".to_string(),
            "image/webp" => return "webp".to_string(),
            "image/tiff" => return "tiff".to_string(),
            "image/bmp" => return "bmp".to_string(),
            "image/x-icon" => return "ico".to_string(),
            "image/avif" => return "avif".to_string(),
            "image/heic" => return "heic".to_string(),
            "image/heif" => return "heif".to_string(),
            "image/jxl" => return "jxl".to_string(),
            "video/mp4" => return "mp4".to_string(),
            "video/quicktime" => return "mov".to_string(),
            "video/webm" => return "webm".to_string(),
            "video/x-matroska" => return "mkv".to_string(),
            "video/x-msvideo" => return "avi".to_string(),
            "video/x-flv" | "video/flv" => return "flv".to_string(),
            _ => {}
        },
        Ok(None) => {}
        Err(e) => {
            foundation::media_conversion_gate::probe_layer_batch_audit(
                "train_quality_format_detect",
                format!("failed to infer media type for {}: {e}", path.display()),
            );
        }
    }

    String::new()
}

fn reject_animated_image_quality_asset(path: &Path) -> Result<()> {
    foundation::training_tier_audit::assert_non_animated_static_asset(path).with_context(|| {
        format!(
            "Animated asset is not valid for image_quality scenario: {}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as _;

    #[test]
    fn allow_partial_is_an_explicit_flag_not_default_behavior() -> Result<()> {
        let cli = Cli::try_parse_from([
            "train_quality",
            "/tmp/samples",
            "--label",
            "high",
            "--allow-partial",
        ])?;

        assert!(cli.allow_partial);
        assert_eq!(cli.scenario, ScenarioType::ImageQuality);

        let strict_cli = Cli::try_parse_from(["train_quality", "/tmp/samples", "--label", "low"])?;
        assert!(!strict_cli.allow_partial);
        Ok(())
    }

    #[test]
    fn missing_input_path_fails_before_reporting_success() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let missing = temp.path().join("missing");

        let err = match collect_input_files(&missing) {
            Ok(files) => anyhow::bail!("missing input unexpectedly returned {files:?}"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("Input path is neither a file nor a directory"),
            "{err:#}"
        );
        Ok(())
    }

    #[test]
    fn directory_walk_returns_only_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let nested = temp.path().join("nested");
        fs::create_dir(&nested)?;
        let a = temp.path().join("a.jpg");
        let b = nested.join("b.png");
        fs::write(&a, b"jpeg")?;
        fs::write(&b, b"png")?;

        let mut files = collect_input_files(temp.path())?;
        files.sort();

        assert_eq!(files, vec![a, b]);
        Ok(())
    }

    #[test]
    fn content_detection_precedes_misleading_extension() -> Result<()> {
        let mut sample = tempfile::NamedTempFile::new()?;
        sample.write_all(b"GIF89a\x01\x00\x01\x00\x00\x00\x00")?;

        assert_eq!(detect_media_extension(sample.path()), "gif");
        Ok(())
    }

    #[test]
    fn unknown_content_does_not_fall_back_to_suffix() -> Result<()> {
        let sample = tempfile::Builder::new().suffix(".gif").tempfile()?;
        std::fs::write(sample.path(), b"not media")?;

        assert!(detect_media_extension(sample.path()).is_empty());
        Ok(())
    }

    #[test]
    fn failure_summary_is_bounded_but_keeps_first_causes() {
        let failures: Vec<String> = (0..7)
            .map(|idx| format!("/sample/{idx}.jpg: broken metadata"))
            .collect();

        let summary = format_failure_summary(&failures);

        assert!(summary.contains("/sample/0.jpg: broken metadata"));
        assert!(summary.contains("/sample/4.jpg: broken metadata"));
        assert!(!summary.contains("/sample/5.jpg: broken metadata"));
        assert!(summary.contains("... 2 more"));
    }
}

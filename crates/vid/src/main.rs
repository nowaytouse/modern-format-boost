#![allow(clippy::too_many_lines)]

use anyhow::Context;
use clap::{Parser, Subcommand};
use foundation::ToolBuilder;
use foundation::log_detail;
use std::path::{Path, PathBuf};

use foundation::analysis_cache::AnalysisCache;
use foundation::conversion_types::SelectedCodec;
use foundation::delivery_codec_strategy::DeliveryProduct;
use vid::{
    ConfigFlags, ConversionConfig, VidQualityError, auto_convert_with_cache, detect_video,
    determine_strategy_with_apple_compat,
};

#[derive(Parser)]
#[command(name = "vid")]
#[command(version, about = "High-performance video and animated media converter", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    #[command(name = "run")]
    Run {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short, long)]
        force: bool,
        #[arg(short, long, default_value_t = true)]
        recursive: bool,
        #[arg(long)]
        delete_original: bool,
        #[arg(long)]
        in_place: bool,
        #[arg(long, default_value_t = true)]
        explore: bool,
        #[arg(long, default_value_t = true)]
        match_quality: bool,
        #[arg(long, default_value_t = true)]
        apple_compat: bool,
        #[arg(long)]
        no_apple_compat: bool,
        #[arg(long, default_value_t = true)]
        compress: bool,
        #[arg(long, default_value_t = false)]
        force_ms_ssim_long: bool,
        #[arg(long, default_value_t = false)]
        ultimate: bool,
        #[arg(long, default_value_t = false)]
        archive: bool,
        #[arg(long)]
        base_dir: Option<PathBuf>,
        #[arg(long, default_value_t = false)]
        allow_size_tolerance: bool,
        #[arg(long)]
        no_allow_size_tolerance: bool,
        /// Explicitly allow HDR10+ encodes to continue as static HDR10 if dynamic metadata extraction fails.
        #[arg(long, default_value_t = false)]
        allow_hdr10plus_static_fallback: bool,
        #[arg(short, long, default_value_t = true)]
        verbose: bool,
        /// ASCII symbols, no decorative ANSI (also respects `NO_COLOR` / `MODERN_FORMAT_PLAIN_UI=1`).
        #[arg(long, default_value_t = false)]
        plain: bool,
        #[arg(long, default_value_t = false)]
        resume: bool,
        #[arg(long)]
        no_resume: bool,
        #[arg(long, default_value = "default")]
        strategy: String,
        #[arg(long, value_parser = ["hevc", "av1"], default_value = "hevc")]
        codec: String,
    },

    Strategy {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(long, value_parser = ["hevc", "av1"], default_value = "hevc")]
        codec: String,
    },

    #[command(
        name = "ingest-samples",
        about = "Batch ingest unannotated GIF samples into SQLite database for Active Learning"
    )]
    IngestSamples {
        #[arg(value_name = "INPUT_DIR")]
        input: PathBuf,
        #[arg(short, long)]
        label: Option<String>,
    },

    #[command(
        name = "db-health",
        about = "Perform deep diagnostic scan of the database infrastructure and data integrity"
    )]
    DbHealth,

    /// Fast GIF-only mode: classify loop intent and output GIFs only.
    #[command(name = "fast-gif")]
    FastGif {
        #[arg(value_name = "INPUT")]
        input: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        originals: Option<PathBuf>,
        #[arg(short, long, default_value_t = true)]
        recursive: bool,
        #[arg(short, long, default_value_t = false)]
        force: bool,
        #[arg(long = "shortest-path", default_value_t = false)]
        shortest_path: bool,
        #[arg(long, default_value_t = false)]
        auto_import: bool,
        #[arg(long, default_value_t = false)]
        apple_compat: bool,
        #[arg(long, default_value = "default")]
        strategy: String,
    },
}

const fn command_requires_database(command: &Commands) -> bool {
    !matches!(command, Commands::FastGif { .. })
}

#[cfg(test)]
const fn fast_gif_shortest_path_supported() -> bool {
    true
}

#[derive(Debug, Clone)]
struct FastGifDelivery {
    input: PathBuf,
    output: PathBuf,
}

fn fast_gif_default_output_dir(input: &Path) -> anyhow::Result<PathBuf> {
    fast_gif_adjacent_dir(input, "gif")
}

fn fast_gif_default_originals_dir(input: &Path) -> anyhow::Result<PathBuf> {
    fast_gif_adjacent_dir(input, "originals")
}

// FIX-REVIEW: fast-gif default dirs have no collision/resume policy, unlike
// fast-img (resolve_working_copy_dir: suffix bump unless a resume marker exists)
// and Python _unique_adjacent_dir (always bumps). Launcher always passes
// --output/--originals so defaults only matter for direct CLI runs — should
// they adopt the fast-img marker-aware policy?
fn fast_gif_adjacent_dir(input: &Path, suffix: &str) -> anyhow::Result<PathBuf> {
    let naming_path = if input.is_file() {
        input.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "fast-gif single-file input has no parent: {}",
                input.display()
            )
        })?
    } else {
        input
    };
    let name = naming_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "fast-gif input has no valid directory name: {}",
                naming_path.display()
            )
        })?;
    Ok(naming_path.with_file_name(format!("{name}_{suffix}")))
}

fn fast_gif_input_root(input: &Path) -> anyhow::Result<PathBuf> {
    if input.is_file() {
        let parent = input.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "fast-gif single-file input has no parent: {}",
                input.display()
            )
        })?;
        return Ok(parent.to_path_buf());
    }
    Ok(input.to_path_buf())
}

#[cfg(test)]
fn fast_gif_output_path_for(
    input: &Path,
    input_root: &Path,
    output_root: &Path,
) -> anyhow::Result<PathBuf> {
    let relative = input.strip_prefix(input_root).with_context(|| {
        format!(
            "fast-gif input {} is outside root {}",
            input.display(),
            input_root.display()
        )
    })?;
    let mut output = output_root.join(relative);
    output.set_extension("GIF");
    Ok(output)
}

fn fast_gif_original_path_for(
    input: &Path,
    input_root: &Path,
    originals_root: &Path,
) -> anyhow::Result<PathBuf> {
    let relative = input.strip_prefix(input_root).with_context(|| {
        format!(
            "fast-gif input {} is outside root {}",
            input.display(),
            input_root.display()
        )
    })?;
    Ok(originals_root.join(relative))
}

fn fast_gif_candidate_files(input: &Path, recursive: bool) -> anyhow::Result<Vec<PathBuf>> {
    if input.is_file() {
        if fast_gif_is_supported_input(input)? {
            return Ok(vec![input.to_path_buf()]);
        }
        anyhow::bail!(
            "fast-gif input is not a true animated-image/video candidate: {}",
            input.display()
        );
    }

    let mut files = Vec::new();
    for path in fast_gif_scan_regular_files(input, recursive)? {
        if fast_gif_is_supported_input(&path)? {
            files.push(path);
        }
    }
    Ok(files)
}

fn fast_gif_is_supported_input(path: &Path) -> anyhow::Result<bool> {
    if let Some(codec) = foundation::quality_matcher::SourceCodec::identify_by_content(path)
        .with_context(|| format!("fast-gif failed to identify {}", path.display()))?
    {
        return Ok(codec.is_video() || codec.can_be_animated());
    }

    let format = foundation::image::format_detect::detect_true_format(path).with_context(|| {
        format!(
            "fast-gif failed to detect true format for {}",
            path.display()
        )
    })?;
    Ok(matches!(
        format,
        foundation::image::format_detect::FormatKind::Gif
            | foundation::image::format_detect::FormatKind::Mp4
            | foundation::image::format_detect::FormatKind::Mov
            | foundation::image::format_detect::FormatKind::Mkv
            | foundation::image::format_detect::FormatKind::Webm
            | foundation::image::format_detect::FormatKind::Jxl
            | foundation::image::format_detect::FormatKind::Avif
            | foundation::image::format_detect::FormatKind::Heic
            | foundation::image::format_detect::FormatKind::Heif
            | foundation::image::format_detect::FormatKind::WebP
    ))
}

fn fast_gif_scan_regular_files(root: &Path, recursive: bool) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending_dirs = vec![root.to_path_buf()];
    while let Some(dir) = pending_dirs.pop() {
        let entries = std::fs::read_dir(&dir)
            .with_context(|| format!("fast-gif scan read dir {}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.with_context(|| format!("fast-gif scan entry under {}", dir.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .with_context(|| format!("fast-gif scan file type {}", path.display()))?;
            if file_type.is_dir() {
                if recursive {
                    pending_dirs.push(path);
                }
                continue;
            }
            if file_type.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn fast_gif_convert_options(
    output_root: &Path,
    input_root: &Path,
    force: bool,
) -> foundation::conversion::ConvertOptions {
    foundation::conversion::ConvertOptions {
        output_dir: Some(output_root.to_path_buf()),
        base_dir: Some(input_root.to_path_buf()),
        flags: foundation::conversion::ConvertFlags::APPLE_COMPAT
            | foundation::conversion::ConvertFlags::ALLOW_SIZE_TOLERANCE
            | if force {
                foundation::conversion::ConvertFlags::FORCE
            } else {
                foundation::conversion::ConvertFlags::empty()
            },
        codec: SelectedCodec::Hevc,
        child_threads: foundation::thread_manager::get_balanced_thread_config(
            foundation::thread_manager::WorkloadType::Video,
        )
        .child_threads,
        input_format: None,
        quality_label: Some("Fast GIF".to_string()),
    }
}

fn fast_gif_verify_output(output: &Path) -> anyhow::Result<()> {
    let format = foundation::image::format_detect::detect_true_format(output)
        .with_context(|| format!("fast-gif failed to verify output {}", output.display()))?;
    if format != foundation::image::format_detect::FormatKind::Gif {
        anyhow::bail!(
            "fast-gif output is not a true GIF: {} (detected {:?})",
            output.display(),
            format
        );
    }
    Ok(())
}

fn fast_gif_delivery_output_path(
    result: &foundation::TaskResult,
) -> anyhow::Result<Option<PathBuf>> {
    let valid_delivery = result.success
        && !result.ignored
        && (!result.skipped || result.skip_reason.as_deref() == Some("already_gif"));
    if !valid_delivery {
        return Ok(None);
    }
    let output = result
        .output_path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("fast-gif delivery succeeded but output path is missing"))?;
    let output = PathBuf::from(output);
    fast_gif_verify_output(&output)?;
    Ok(Some(output))
}

fn fast_gif_strip_import_suffixes(folder_name: &str) -> String {
    let mut cleaned = folder_name;
    for suffix in [
        "_gif",
        "_optimized_collected",
        "_collected_optimized",
        "_optimized",
        "_collected",
    ] {
        if let Some(stripped) = cleaned.strip_suffix(suffix) {
            cleaned = stripped;
        }
    }
    cleaned.to_string()
}

fn fast_gif_import_album_name(output_root: &Path, rel_path: &str) -> String {
    let rel_parent_leaf = Path::new(rel_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    let output_root_leaf = output_root.file_name().and_then(|name| name.to_str());
    let cleaned = rel_parent_leaf
        .or(output_root_leaf)
        .map(fast_gif_strip_import_suffixes)
        .filter(|name| !name.is_empty());
    match cleaned {
        Some(name) if name.starts_with('✨') => name,
        Some(name) => format!("✨{name}"),
        None => "✨Imported".to_string(),
    }
}

fn fast_gif_photos_import_candidates(
    deliveries: &[FastGifDelivery],
    output_root: &Path,
) -> anyhow::Result<Vec<foundation::image::fast_img::PhotosImportCandidate>> {
    let mut candidates = Vec::with_capacity(deliveries.len());
    for delivery in deliveries {
        let rel = delivery.output.strip_prefix(output_root).with_context(|| {
            format!(
                "fast-gif output {} is outside output root {}",
                delivery.output.display(),
                output_root.display()
            )
        })?;
        let rel_path = rel.to_string_lossy().to_string();
        let blake3 =
            foundation::common_utils::calculate_blake3_hash(&delivery.output).map_err(|err| {
                anyhow::anyhow!(
                    "fast-gif failed to hash GIF output for Photos import {}: {err}",
                    delivery.output.display()
                )
            })?;
        candidates.push(foundation::image::fast_img::PhotosImportCandidate {
            album_name: fast_gif_import_album_name(output_root, &rel_path),
            rel_path,
            path: delivery.output.clone(),
            blake3,
        });
    }
    Ok(candidates)
}

fn fast_gif_move_original(
    input: &Path,
    input_root: &Path,
    originals_root: &Path,
) -> anyhow::Result<PathBuf> {
    let dest = fast_gif_original_path_for(input, input_root, originals_root)?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("fast-gif failed to create {}", parent.display()))?;
    }
    if dest.exists() {
        anyhow::bail!(
            "fast-gif originals destination already exists; refusing overwrite: {}",
            dest.display()
        );
    }

    let xmp_sidecar = fast_gif_find_xmp_sidecar(input);
    foundation::io_utils::robust_move(input, &dest).with_context(|| {
        format!(
            "fast-gif failed to move original {} to {}",
            input.display(),
            dest.display()
        )
    })?;
    if let Some(sidecar) = xmp_sidecar {
        let sidecar_name = sidecar.file_name().ok_or_else(|| {
            anyhow::anyhow!(
                "fast-gif XMP sidecar has no filename: {}",
                sidecar.display()
            )
        })?;
        let sidecar_dest = dest.with_file_name(sidecar_name);
        foundation::io_utils::robust_move(&sidecar, &sidecar_dest).with_context(|| {
            format!(
                "fast-gif failed to move XMP sidecar {} to {}",
                sidecar.display(),
                sidecar_dest.display()
            )
        })?;
    }
    Ok(dest)
}

fn fast_gif_find_xmp_sidecar(input: &Path) -> Option<PathBuf> {
    if let Some(ext) = input.extension().and_then(|ext| ext.to_str()) {
        let sidecar = input.with_extension(format!("{ext}.xmp"));
        if sidecar.exists() {
            return Some(sidecar);
        }
        let sidecar = input.with_extension(format!("{ext}.XMP"));
        if sidecar.exists() {
            return Some(sidecar);
        }
    }
    let sidecar = input.with_extension("xmp");
    if sidecar.exists() {
        return Some(sidecar);
    }
    let sidecar = input.with_extension("XMP");
    sidecar.exists().then_some(sidecar)
}

#[allow(clippy::fn_params_excessive_bools)]
fn run_fast_gif(
    input: &Path,
    output_dir: Option<&Path>,
    originals_dir: Option<&Path>,
    recursive: bool,
    force: bool,
    shortest_path: bool,
    auto_import: bool,
    apple_compat: bool,
    strategy: &str,
) -> anyhow::Result<()> {
    if auto_import && !shortest_path {
        anyhow::bail!("fast-gif --auto-import requires --shortest-path");
    }
    if let Err(err) =
        foundation::tools::require(&["ffmpeg", "ffprobe", "exiftool", "gifski", "djxl", "webpmux"])
    {
        foundation::log_fatal!(foundation::infra::static_logs::messages::LABEL_TOOLS, &err);
        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
    }

    let input_root = fast_gif_input_root(input)?;
    foundation::check_dangerous_directory(&input_root).map_err(|err| anyhow::anyhow!(err))?;
    let output_root = match output_dir {
        Some(path) => path.to_path_buf(),
        None => fast_gif_default_output_dir(input)?,
    };
    let originals_root = match originals_dir {
        Some(path) => path.to_path_buf(),
        None => fast_gif_default_originals_dir(input)?,
    };
    let files = fast_gif_candidate_files(input, recursive)?;
    println!(
        "[SCAN    ] Found {} animated/video candidates in {}",
        files.len(),
        input_root.display()
    );

    let options = fast_gif_convert_options(&output_root, &input_root, force);
    let mut converted = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    let mut deliveries = Vec::new();
    let effective_strategy = if apple_compat { "gif" } else { strategy };
    for file in files {
        if effective_strategy == "avif" {
            // AVIF Strategy (Meme Mode) bypasses loop intent judgment and just encodes to AVIF.
            let stem = file
                .file_stem()
                .ok_or_else(|| anyhow::anyhow!("missing file stem for {}", file.display()))?;
            let output = output_root.join(stem).with_extension("avif");

            let temp_output = foundation::path_safety::isolated_temp_path_for_search(&output)
                .map_err(|e| anyhow::anyhow!(e))?;
            let q = foundation::media_conversion_gate::avif_quality_or_fallback(None);

            let mut builder = foundation::AvifencBuilder::new();
            builder
                .speed(4)
                .jobs("all")
                .quality(q)
                .input(&file)
                .output(&temp_output);

            let output_cmd = builder.build().output()?;
            if !output_cmd.status.success() {
                failed += 1;
                println!(
                    "[FAIL    ] {} avifenc failed: {}",
                    file.display(),
                    String::from_utf8_lossy(&output_cmd.stderr)
                );
                continue;
            }

            std::fs::rename(&temp_output, &output)?;
            println!("[READY   ] {} -> {}", file.display(), output.display());
            deliveries.push(FastGifDelivery {
                input: file.clone(),
                output,
            });
            converted += 1;
            continue;
        }

        let verdict = vid::animated_image::assess_loop_intent_for_fast_gif(&file)?;
        if !verdict.is_keep_gif() {
            skipped += 1;
            println!(
                "[SKIP    ] {} loop-intent is not GIF: {}",
                file.display(),
                verdict.reason()
            );
            continue;
        }

        let result = vid::animated_image::convert_to_gif_apple_compat(&file, &options)
            .map_err(|err| anyhow::anyhow!(err))?;
        if let Some(output) = fast_gif_delivery_output_path(&result)? {
            println!("[READY   ] {} -> {}", file.display(), output.display());
            deliveries.push(FastGifDelivery {
                input: file.clone(),
                output,
            });
            converted += 1;
        } else if !result.success && result.skipped {
            // failed_with_fallback: encode failed and the original was copied as fallback.
            // This is a real failure, not a content-based skip.
            failed += 1;
            println!("[FAIL    ] {} {}", file.display(), result.message);
        } else {
            skipped += 1;
            println!("[SKIP    ] {} {}", file.display(), result.message);
        }
    }
    foundation::preserve_directory_with_log(&input_root, &output_root).with_context(|| {
        format!(
            "fast-gif failed to preserve output directory metadata {} -> {}",
            input_root.display(),
            output_root.display()
        )
    })?;
    if auto_import {
        let candidates = fast_gif_photos_import_candidates(&deliveries, &output_root)?;
        if !candidates.is_empty() {
            let library = foundation::image::fast_img::import_media_outputs_with_library_verifier(
                &candidates,
            )
            .map_err(|err| anyhow::anyhow!(err))?;
            if library.imported_assets.len() != candidates.len() {
                anyhow::bail!(
                    "fast-gif Photos import proof count mismatch: expected {} got {}",
                    candidates.len(),
                    library.imported_assets.len()
                );
            }
            println!(
                "[IMPORT  ] verified {} GIF output(s) in Photos library",
                library.imported_assets.len()
            );
        }
    }
    for delivery in deliveries {
        let moved = fast_gif_move_original(&delivery.input, &input_root, &originals_root)?;
        println!(
            "[DONE    ] {} -> {} | original moved to {}",
            delivery.input.display(),
            delivery.output.display(),
            moved.display()
        );
    }
    foundation::preserve_directory_with_log(&input_root, &originals_root).with_context(|| {
        format!(
            "fast-gif failed to preserve originals directory metadata {} -> {}",
            input_root.display(),
            originals_root.display()
        )
    })?;
    println!(
        "[DONE    ] fast-gif converted {converted} files to {} ({skipped} skipped, {failed} failed)",
        output_root.display()
    );
    if failed > 0 {
        anyhow::bail!("fast-gif failed to convert {failed} file(s); see [FAIL] lines above");
    }
    Ok(())
}

// Rationale: This function handles complex, sequential initialization or business logic where further fragmentation would hinder readability and maintainability.
#[allow(clippy::too_many_lines)]
fn main() -> anyhow::Result<()> {
    foundation::entry_guard::assert_product_cli_entry("vid").context("vid entry guard")?;
    foundation::init_ghost_mode().context("Failed to initialize ghost mode")?;

    foundation::logging::init("vid", &foundation::logging::LogConfig::default())
        .map_err(|e| e.context("Failed to initialize vid logging"))?;

    foundation::ctrlc_guard::init();

    let cli = Cli::parse();
    if command_requires_database(&cli.command) {
        // Enforce PostgreSQL dependency as mandatory for the DB-backed video toolchain.
        // Fast GIF mode is excluded: it performs content-aware scanning, LoopIntent
        // classification, GIF output, and source isolation without DB/cache state.
        if let Err(e) = foundation::database::open_pg_client() {
            foundation::log_fatal!(
                "Infrastructure",
                &format!(
                    "PostgreSQL database is mandatory for full feature availability. Connection failed: {e}"
                )
            );
            std::process::exit(foundation::constants::EXIT_CODE_ERROR);
        }
    }

    // --- Unified Directory Locking (Ghost Mode & Mutex) ---
    // Extract input path from relevant commands to lock the directory ONLY if it involves destructive or interactive shared state.
    let input_to_lock = match &cli.command {
        Commands::Run {
            input,
            in_place,
            delete_original,
            ..
        } if *in_place || *delete_original => Some(input),
        Commands::FastGif { input, .. } => Some(input),
        _ => None,
    };

    let _lock_guard = input_to_lock.and_then(|input| {
        let input_abs = foundation::media_conversion_gate::canonicalize_for_tool_input(input);
        if input_abs.is_dir() {
            match foundation::acquire_dir_lock(&input_abs) {
                Ok(guard) => Some(guard),
                Err(e) => {
                    foundation::log_fatal!(
                        foundation::infra::static_logs::messages::LABEL_LOCK,
                        &e.to_string()
                    );
                    std::process::exit(foundation::constants::EXIT_CODE_LOCK_FAILURE);
                }
            }
        } else {
            None
        }
    });
    // ------------------------------------------------------

    match cli.command {
        Commands::Run {
            input,
            output,
            force,
            recursive,
            delete_original,
            in_place,
            explore,
            match_quality,
            apple_compat,
            no_apple_compat,
            compress,
            force_ms_ssim_long,
            ultimate,
            archive,
            base_dir,
            allow_size_tolerance,
            no_allow_size_tolerance,
            allow_hdr10plus_static_fallback,
            verbose,
            plain,
            resume,
            no_resume,
            strategy,
            codec,
        } => {
            // Fail-fast if critical sub-tools are missing
            if let Err(e) = foundation::tools::require(&["ffmpeg", "ffprobe", "exiftool"]) {
                foundation::log_fatal!(foundation::infra::static_logs::messages::LABEL_TOOLS, &e);
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }

            let apple_compat = apple_compat && !no_apple_compat;
            let allow_size_tolerance = allow_size_tolerance && !no_allow_size_tolerance;
            let resume = resume && !no_resume;
            let selected_codec = match SelectedCodec::resolve_cli_delivery_codec(
                DeliveryProduct::Vid,
                &codec,
                apple_compat,
            ) {
                Ok(c) => c,
                Err(e) => {
                    foundation::log_fatal!(
                        foundation::infra::static_logs::messages::LABEL_CONFIG,
                        &e.to_string(),
                    );
                    std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                }
            };
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_STRATEGY,
                foundation::delivery_codec_strategy::vid_run_routing_summary(selected_codec)
            );
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_STRATEGY,
                selected_codec.delivery_policy_summary()
            );

            if let Err(e) =
                foundation::validate_flags_result_with_ultimate(foundation::FlagRequest {
                    base: foundation::FlagBase {
                        explore,
                        match_quality,
                        compress,
                    },
                    tier: foundation::FlagTier { ultimate },
                })
            {
                foundation::log_fatal!(foundation::infra::static_logs::messages::LABEL_CONFIG, &e);
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }

            let base_dir =
                foundation::cli_runner::resolve_video_run_base_dir(&input, recursive, base_dir);

            let config = build_conversion_config(
                output.clone(),
                base_dir.clone(),
                force,
                delete_original,
                explore,
                match_quality,
                in_place,
                apple_compat,
                compress,
                force_ms_ssim_long,
                ultimate,
                archive,
                allow_size_tolerance,
                allow_hdr10plus_static_fallback,
                plain,
                &strategy,
                selected_codec,
            );

            foundation::progress_mode::configure_terminal_ux(plain);
            foundation::progress_mode::set_verbose_mode(verbose);
            foundation::progress_mode::maybe_log_inference_analytics_hint(verbose);
            // Automatically created under LogConfig::unified_log_dir() as vid_run_<timestamp>.log.
            if let Err(e) = foundation::progress_mode::set_default_run_log_file("vid") {
                foundation::log_fatal!(
                    foundation::infra::static_logs::messages::LABEL_RUN_LOG,
                    &format!(
                        "{}: {}",
                        foundation::infra::static_logs::messages::RUN_LOG_OPEN_FAIL,
                        e
                    )
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
            let strategy_msg = if explore && match_quality {
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_MAPPING_LOSSY_MATCH
            } else if explore {
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_MAPPING_LOSSY_BASE
            } else {
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_MAPPING_LOSSLESS
            };

            let mut config_parts = Vec::new();
            config_parts.push(if selected_codec == SelectedCodec::Hevc {
                format!(
                    "codec=HEVC quality={}",
                    if match_quality { "match" } else { "18-20" }
                )
            } else {
                format!(
                    "codec=AV1 quality={}",
                    if match_quality { "match" } else { "30-32" }
                )
            });

            if explore {
                config_parts.push("explore=ON".to_string());
            }
            if apple_compat {
                config_parts.push("apple_compat=ON".to_string());
                unsafe { std::env::set_var("MODERN_FORMAT_BOOST_APPLE_COMPAT", "1") };
            }
            if recursive {
                config_parts.push("recursive=ON".to_string());
            }
            if ultimate {
                config_parts.push("ultimate=ON".to_string());
            }
            if archive {
                config_parts.push("archive=ON".to_string());
            }
            if force_ms_ssim_long {
                config_parts.push("ms_ssim=ON".to_string());
            }
            if allow_hdr10plus_static_fallback {
                config_parts.push("hdr10plus_static_fallback=ON".to_string());
            }

            let cache = match AnalysisCache::default_local() {
                Ok(cache) => {
                    config_parts.push("cache=ON".to_string());
                    match cache.cleanup_old_records(foundation::constants::CACHE_PRUNE_AGE_SECS) {
                        Ok(removed) if removed > 0 => {
                            foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                                "analysis_cache_age_prune_completed",
                                format!("removed={removed}"),
                            );
                        }
                        Err(e) => {
                            foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                                "analysis_cache_age_prune_failed",
                                format!("failed to prune aged cache rows: {e}"),
                            );
                        }
                        Ok(_) => {}
                    }
                    Some(cache)
                }
                Err(e) => {
                    foundation::media_conversion_gate::analysis_cache_lifecycle_batch_audit(
                        "analysis_cache_unavailable",
                        format!("failed to initialize persistent cache: {e}"),
                    );
                    None
                }
            };

            foundation::log_info!(
                foundation::infra::static_logs::messages::LABEL_CONFIG,
                &format!(
                    "Session Audit: {} | {}",
                    strategy_msg.replace("{}", &selected_codec.as_str().to_uppercase()),
                    config_parts.join(" | ")
                )
            );

            foundation::database::report_db_status();

            log_detail!("");

            foundation::cli_runner::run_auto_command(
                &foundation::cli_runner::Config {
                    input: input.clone(),
                    output: output.clone(),
                    recursive,
                    label: "HEVC Video".to_string(),
                    base_dir: foundation::media_conversion_gate::delivery_cli_base_dir_or_input_when_output(
                        base_dir,
                        output.as_deref(),
                        &input,
                    ),
                    resume,
                    protect_destructive_dirs: delete_original || in_place,
                },
                |file| {
                    auto_convert_with_cache(file, &config, cache.as_ref())
                        .map_err(|e: VidQualityError| anyhow::anyhow!(e))
                },
            )?;
            foundation::progress_mode::xmp_merge_finalize();
            foundation::progress_mode::flush_log_file();
        }

        Commands::FastGif {
            input,
            output,
            originals,
            recursive,
            force,
            shortest_path,
            auto_import,
            apple_compat,
            strategy,
        } => {
            run_fast_gif(
                &input,
                output.as_deref(),
                originals.as_deref(),
                recursive,
                force,
                shortest_path,
                auto_import,
                apple_compat,
                &strategy,
            )?;
        }

        Commands::Strategy { input, codec } => {
            let detection = detect_video(&input)?;
            let selected_codec =
                SelectedCodec::resolve_cli_delivery_codec(DeliveryProduct::Vid, &codec, false)
                    .map_err(|e| {
                        foundation::log_fatal!(
                            foundation::infra::static_logs::messages::LABEL_CONFIG,
                            &e.to_string(),
                        );
                        std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                    })?;
            let strategy = determine_strategy_with_apple_compat(
                &detection,
                &input,
                false,
                false,
                selected_codec,
            );

            foundation::log_summary_header!(
                foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_AUDIT
            );
            foundation::log_detail!(
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_ASSET
                    .replace("{}", &input.display().to_string())
            );
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_DETECTION,
                foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_DETECTION
                    .replacen("{}", detection.codec.as_str(), 1)
                    .replacen("{}", detection.compression.as_str(), 1)
            );
            foundation::log_stat!(
                foundation::infra::static_logs::messages::LABEL_STRATEGY,
                foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_TARGET
                    .replace("{}", strategy.target.as_str())
            );
            foundation::log_detail!(
                &foundation::infra::static_logs::messages::MSG_MAIN_VID_STRATEGY_BASIS
                    .replace("{}", &strategy.reason)
            );
        }

        Commands::IngestSamples { input, label } => {
            if !input.is_dir() {
                foundation::media_conversion_gate::probe_layer_batch_audit(
                    "ingest_path_not_directory",
                    format!(
                        "ingest-samples requires a directory; got {}",
                        input.display()
                    ),
                );
                std::process::exit(foundation::constants::EXIT_CODE_ERROR);
            }
            if let Some(lbl) = &label {
                log_detail!(format!(
                    "{save} Active Learning Audit: Ingesting labeled video samples [{lbl}] from {path}",
                    save = foundation::modern_ui::symbols::SAVE,
                    path = input.display(),
                ));
            } else {
                log_detail!(format!(
                    "{save} Active Learning Audit: Discovering video samples in {path}",
                    save = foundation::modern_ui::symbols::SAVE,
                    path = input.display(),
                ));
            }
            let conn_str = foundation::database::get_pg_conn_str();
            if conn_str.trim().is_empty() {
                anyhow::bail!("MFB_PG_CONNSTR must be set before ingest-samples");
            }
            match foundation::database::batch_ingest_loop_intent_samples(
                &input,
                label.as_deref(),
                &conn_str,
            ) {
                Ok(count) => {
                    crate::log_detail!(format!(
                        "{check} Active Learning Audit: Successfully ingested {count} video feature vectors",
                        check = foundation::modern_ui::symbols::CHECK,
                    ));
                }
                Err(e) => {
                    foundation::media_conversion_gate::probe_layer_audit(
                        "ingest_batch_failed",
                        &input,
                        format!("batch ingestion failed: {e}"),
                    );
                    std::process::exit(foundation::constants::EXIT_CODE_ERROR);
                }
            }
        }
        Commands::DbHealth => {
            foundation::log_info!(
                foundation::infra::static_logs::messages::LABEL_INFRASTRUCTURE_AUDIT,
                foundation::infra::static_logs::messages::DB_HEALTH_START
            );
            match foundation::database::check_database_health() {
                Ok(report) => {
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_SESSION_SUMMARY,
                        foundation::infra::static_logs::messages::DB_HEALTH_FINALIZED
                    );
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CONN.replacen(
                            "{}",
                            if report.connected {
                                foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CONN_OK
                            } else {
                                foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CONN_FAIL
                            },
                            1
                        )
                    );
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_ENGINE
                            .replace("{}", &report.pg_version)
                    );
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        format!(
                            "Infrastructure Audit: Required vector extensions: {}",
                            if report.has_vector_extension {
                                format!(
                                    "Infrastructure Audit: pgvector v{} discovered and verified",
                                    foundation::media_conversion_gate::infra_version_label_or_audit(
                                        "pgvector_version",
                                        report.vector_extension_version.as_deref(),
                                        "unknown",
                                    )
                                )
                            } else {
                                "Infrastructure Audit: Missing critical pgvector extension"
                                    .to_string()
                            }
                        )
                    );
                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_REPORT,
                        &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_MATURITY
                            .replace("{}", &report.maturity_status)
                    );

                    foundation::log_info!(
                        foundation::infra::static_logs::messages::LABEL_INVENTORY_AUDIT,
                        foundation::infra::static_logs::messages::DB_INVENTORY_CROSS_REF
                    );
                    let mut tables: Vec<_> = report.table_counts.iter().collect();
                    tables.sort_by_key(|(name, _)| *name);
                    for (name, count) in tables {
                        foundation::log_info!(
                            foundation::infra::static_logs::messages::LABEL_REPORT,
                            &foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_TABLE
                                .replace("{name:<20}", &format!("{name:<20}"))
                                .replace("{count:>8}", &format!("{count:>8}"))
                        );
                    }

                    if report.corruption_found {
                        foundation::media_conversion_gate::probe_layer_batch_audit(
                            "db_health_corruption",
                            foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CORRUPTION,
                        );
                        for detail in report.corruption_details {
                            foundation::media_conversion_gate::probe_layer_batch_audit(
                                "db_health_corruption_detail",
                                foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_CORRUPTION_ALERT
                                    .replace("{}", &detail),
                            );
                        }
                    } else {
                        foundation::log_info!(
                            foundation::infra::static_logs::messages::LABEL_REPORT,
                            foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_INTEGRITY_OK
                        );
                    }
                }
                Err(e) => {
                    foundation::media_conversion_gate::probe_layer_batch_audit(
                        "db_health_check_failed",
                        foundation::infra::static_logs::messages::MSG_MAIN_DB_HEALTH_ABORT
                            .replace("{}", &e.to_string()),
                    );
                    return Err(e).context("db-health failed");
                }
            }
        }
    }

    {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() {
            // Historically waited for macOS UI confirmation via foundation::macos_ui.
            // The foundation crate no longer exposes that module; keep this as a no-op.
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn build_conversion_config(
    output_dir: Option<PathBuf>,
    base_dir: Option<PathBuf>,
    force: bool,
    delete_original: bool,
    explore: bool,
    match_quality: bool,
    in_place: bool,
    apple_compat: bool,
    compress: bool,
    force_ms_ssim_long: bool,
    ultimate: bool,
    archive: bool,
    allow_size_tolerance: bool,
    allow_hdr10plus_static_fallback: bool,
    _plain: bool,
    _strategy: &str,
    codec: SelectedCodec,
) -> ConversionConfig {
    ConversionConfig {
        output_dir,
        base_dir,
        flags: ConfigFlags::empty()
            | if force {
                ConfigFlags::FORCE
            } else {
                ConfigFlags::empty()
            }
            | if delete_original {
                ConfigFlags::DELETE_ORIGINAL
            } else {
                ConfigFlags::empty()
            }
            | if explore {
                ConfigFlags::EXPLORE_SMALLER
            } else {
                ConfigFlags::empty()
            }
            | if match_quality {
                ConfigFlags::MATCH_QUALITY
            } else {
                ConfigFlags::empty()
            }
            | if in_place {
                ConfigFlags::IN_PLACE
            } else {
                ConfigFlags::empty()
            }
            | if apple_compat {
                ConfigFlags::APPLE_COMPAT
            } else {
                ConfigFlags::empty()
            }
            | if compress {
                ConfigFlags::REQUIRE_COMPRESSION
            } else {
                ConfigFlags::empty()
            }
            | ConfigFlags::USE_GPU
            | if force_ms_ssim_long {
                ConfigFlags::FORCE_MS_SSIM_LONG
            } else {
                ConfigFlags::empty()
            }
            | if ultimate {
                ConfigFlags::ULTIMATE_MODE
            } else {
                ConfigFlags::empty()
            }
            | if archive {
                ConfigFlags::ARCHIVE_MODE
            } else {
                ConfigFlags::empty()
            }
            | if allow_size_tolerance {
                ConfigFlags::ALLOW_SIZE_TOLERANCE
            } else {
                ConfigFlags::empty()
            }
            | if allow_hdr10plus_static_fallback {
                ConfigFlags::ALLOW_HDR10PLUS_STATIC_FALLBACK
            } else {
                ConfigFlags::empty()
            },
        min_ssim: foundation::constants::MIN_SSIM_DEFAULT,
        child_threads: foundation::thread_manager::get_balanced_thread_config(
            foundation::thread_manager::WorkloadType::Video,
        )
        .child_threads,
        codec,
    }
}

#[cfg(test)]
mod fast_gif_tests {
    use super::{
        Cli, Commands, FastGifDelivery, command_requires_database, fast_gif_candidate_files,
        fast_gif_delivery_output_path, fast_gif_original_path_for, fast_gif_output_path_for,
        fast_gif_photos_import_candidates, fast_gif_shortest_path_supported,
    };
    use clap::Parser;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_gif(path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            path,
            b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!",
        )?;
        Ok(())
    }

    fn write_jpeg(path: &std::path::Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, [0xff, 0xd8, 0xff, 0xe0, b'f', b'a', b'k', b'e'])?;
        Ok(())
    }

    #[test]
    fn fast_gif_command_accepts_output_originals_and_shortest_path_flags() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from([
            "vid",
            "fast-gif",
            "/media/in",
            "--output",
            "/media/in_gif",
            "--originals",
            "/media/in_originals",
            "--recursive",
            "--shortest-path",
            "--auto-import",
        ])?;

        let Commands::FastGif {
            input,
            output,
            originals,
            recursive,
            force,
            shortest_path,
            auto_import,
            apple_compat,
            strategy,
        } = parsed.command
        else {
            anyhow::bail!("expected fast-gif command");
        };
        assert_eq!(input, PathBuf::from("/media/in"));
        assert_eq!(output, Some(PathBuf::from("/media/in_gif")));
        assert_eq!(originals, Some(PathBuf::from("/media/in_originals")));
        assert!(recursive);
        assert!(!force);
        assert!(shortest_path);
        assert!(auto_import);
        assert!(!apple_compat);
        assert_eq!(strategy, "default");
        Ok(())
    }

    #[test]
    fn fast_gif_command_does_not_require_database_preflight() {
        let command = Commands::FastGif {
            input: PathBuf::from("/media/in"),
            output: Some(PathBuf::from("/media/in_gif")),
            originals: Some(PathBuf::from("/media/in_originals")),
            recursive: true,
            force: false,
            shortest_path: false,
            auto_import: false,
            apple_compat: false,
            strategy: "default".to_string(),
        };

        assert!(!command_requires_database(&command));
    }

    #[test]
    fn fast_gif_command_accepts_meme_mode_strategy() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["vid", "fast-gif", "/media/in", "--strategy", "avif"])?;

        let Commands::FastGif { strategy, .. } = parsed.command else {
            anyhow::bail!("expected fast-gif command");
        };
        assert_eq!(strategy, "avif");
        Ok(())
    }

    #[test]
    fn run_command_accepts_archive_flag() -> anyhow::Result<()> {
        let parsed = Cli::try_parse_from(["vid", "run", "/media/in", "--archive"])?;

        let Commands::Run { archive, .. } = parsed.command else {
            anyhow::bail!("expected run command");
        };
        assert!(archive);
        Ok(())
    }

    #[test]
    fn fast_gif_paths_preserve_nested_folder_structure() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let input_root = root.path().join("media");
        let output_root = root.path().join("media_gif");
        let originals_root = root.path().join("media_originals");
        let source = input_root.join("nested/day1/clip.mp4");

        let output = fast_gif_output_path_for(&source, &input_root, &output_root)?;
        let original = fast_gif_original_path_for(&source, &input_root, &originals_root)?;

        assert_eq!(output, output_root.join("nested/day1/clip.GIF"));
        assert_eq!(original, originals_root.join("nested/day1/clip.mp4"));
        Ok(())
    }

    #[test]
    fn fast_gif_candidates_are_content_aware_not_extension_only() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let input_root = root.path().join("media");
        let gif_without_extension = input_root.join("nested/loop_asset");
        let spoofed_gif = input_root.join("not_loop.gif");
        write_gif(&gif_without_extension)?;
        write_jpeg(&spoofed_gif)?;

        let files = fast_gif_candidate_files(&input_root, true)?;

        assert_eq!(files, vec![gif_without_extension]);
        Ok(())
    }

    #[test]
    fn fast_gif_shortest_path_import_is_supported_by_verified_photos_proof() {
        assert!(fast_gif_shortest_path_supported());
    }

    #[test]
    fn fast_gif_accepts_verified_successful_skipped_copy_delivery() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let output = root.path().join("media_gif/nested/clip.gif");
        write_gif(&output)?;
        let result = foundation::TaskResult {
            success: true,
            input_path: root
                .path()
                .join("media/nested/clip.gif")
                .display()
                .to_string(),
            output_path: Some(output.display().to_string()),
            input_size: 32,
            output_size: Some(32),
            size_reduction: None,
            message: "Skipped: Already GIF".to_string(),
            skipped: true,
            ignored: false,
            skip_reason: Some("already_gif".to_string()),
            blake3: None,
            explore_final_crf: None,
            explore_iterations: None,
        };

        assert_eq!(fast_gif_delivery_output_path(&result)?, Some(output));
        Ok(())
    }

    #[test]
    fn fast_gif_photos_import_candidates_use_gif_hash_and_nested_album() -> anyhow::Result<()> {
        let root = TempDir::new()?;
        let output_root = root.path().join("media_gif");
        let output = output_root.join("nested/day1/clip.GIF");
        write_gif(&output)?;
        let delivery = FastGifDelivery {
            input: root.path().join("media/nested/day1/clip.mp4"),
            output: output.clone(),
        };

        let candidates = fast_gif_photos_import_candidates(&[delivery], &output_root)?;

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].rel_path, "nested/day1/clip.GIF");
        assert_eq!(
            candidates[0].blake3,
            foundation::common_utils::calculate_blake3_hash(&output)?
        );
        assert_eq!(candidates[0].album_name, "✨day1");
        Ok(())
    }
}

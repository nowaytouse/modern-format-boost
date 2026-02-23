//! Conversion Utilities Module
//!
//! Provides common conversion functionality shared across all tools:
//! - ConversionResult: Unified result structure
//! - ConvertOptions: Common conversion options
//! - Anti-duplicate mechanism: Track processed files
//! - Result builders: Reduce boilerplate code
//! - Size formatting: Unified message formatting

#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

static PROCESSED_FILES: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn is_already_processed(path: &Path) -> bool {
    let canonical = path
        .canonicalize()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());

    let processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    processed.contains(&canonical)
}

pub fn mark_as_processed(path: &Path) {
    let canonical = path
        .canonicalize()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| path.display().to_string());

    let mut processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    processed.insert(canonical);
}

pub fn clear_processed_list() {
    let mut processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    processed.clear();
}


pub use crate::checkpoint::{safe_delete_original, verify_output_integrity};

pub fn load_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !list_path.exists() {
        return Ok(());
    }

    let file = fs::File::open(list_path)?;
    let reader = BufReader::new(file);
    let mut processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());

    for path in reader.lines().map_while(Result::ok) {
        processed.insert(path);
    }

    Ok(())
}

pub fn save_processed_list(list_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let processed = PROCESSED_FILES.lock().unwrap_or_else(|e| e.into_inner());
    let mut file = fs::File::create(list_path)?;

    for path in processed.iter() {
        writeln!(file, "{}", path)?;
    }

    Ok(())
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
    pub success: bool,
    pub input_path: String,
    pub output_path: Option<String>,
    pub input_size: u64,
    pub output_size: Option<u64>,
    pub size_reduction: Option<f64>,
    pub message: String,
    pub skipped: bool,
    pub skip_reason: Option<String>,
}

impl ConversionResult {
    pub fn skipped_duplicate(input: &Path) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size: fs::metadata(input).map(|m| m.len()).unwrap_or(0),
            output_size: None,
            size_reduction: None,
            message: "Skipped: Already processed".to_string(),
            skipped: true,
            skip_reason: Some("duplicate".to_string()),
        }
    }

    pub fn skipped_exists(input: &Path, output: &Path) -> Self {
        let input_size = fs::metadata(input).map(|m| m.len()).unwrap_or(0);
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: fs::metadata(output).map(|m| m.len()).ok(),
            size_reduction: None,
            message: "Skipped: Output file exists".to_string(),
            skipped: true,
            skip_reason: Some("exists".to_string()),
        }
    }

    pub fn skipped_custom(input: &Path, input_size: u64, reason: &str, skip_reason: &str) -> Self {
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: reason.to_string(),
            skipped: true,
            skip_reason: Some(skip_reason.to_string()),
        }
    }

    pub fn skipped_size_increase(input: &Path, input_size: u64, output_size: u64) -> Self {
        let increase_pct = (output_size as f64 / input_size as f64 - 1.0) * 100.0;
        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: None,
            input_size,
            output_size: None,
            size_reduction: None,
            message: format!("Skipped: Output would be larger (+{:.1}%)", increase_pct),
            skipped: true,
            skip_reason: Some("size_increase".to_string()),
        }
    }

    pub fn success(
        input: &Path,
        output: &Path,
        input_size: u64,
        output_size: u64,
        format_name: &str,
        extra_info: Option<&str>,
    ) -> Self {
        let reduction = 1.0 - (output_size as f64 / input_size as f64);
        let reduction_pct = reduction * 100.0;

        let message = if reduction >= 0.0 {
            match extra_info {
                Some(info) => format!(
                    "{} ({}): size reduced {:.1}%",
                    format_name, info, reduction_pct
                ),
                None => format!(
                    "{} conversion successful: size reduced {:.1}%",
                    format_name, reduction_pct
                ),
            }
        } else {
            match extra_info {
                Some(info) => format!(
                    "{} ({}): size increased {:.1}%",
                    format_name, info, -reduction_pct
                ),
                None => format!(
                    "{} conversion successful: size increased {:.1}%",
                    format_name, -reduction_pct
                ),
            }
        };

        Self {
            success: true,
            input_path: input.display().to_string(),
            output_path: Some(output.display().to_string()),
            input_size,
            output_size: Some(output_size),
            size_reduction: Some(reduction_pct),
            message,
            skipped: false,
            skip_reason: None,
        }
    }
}


#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub force: bool,
    pub output_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub delete_original: bool,
    pub in_place: bool,
    pub explore: bool,
    pub match_quality: bool,
    pub apple_compat: bool,
    pub compress: bool,
    pub use_gpu: bool,
    pub ultimate: bool,
    pub allow_size_tolerance: bool,
    pub verbose: bool,
    pub child_threads: usize,
    pub input_format: Option<String>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            force: false,
            output_dir: None,
            base_dir: None,
            delete_original: false,
            in_place: false,
            explore: false,
            match_quality: false,
            apple_compat: false,
            compress: false,
            use_gpu: true,
            ultimate: false,
            allow_size_tolerance: true,
            verbose: false,
            child_threads: 0,
            input_format: None,
        }
    }
}

impl ConvertOptions {
    pub fn should_delete_original(&self) -> bool {
        self.delete_original || self.in_place
    }

    pub fn flag_mode(&self) -> Result<crate::flag_validator::FlagMode, String> {
        crate::flag_validator::validate_flags_result_with_ultimate(
            self.explore,
            self.match_quality,
            self.compress,
            self.ultimate,
        )
    }

    pub fn explore_mode(&self) -> crate::video_explorer::ExploreMode {
        match self.flag_mode() {
            Ok(_) => crate::video_explorer::ExploreMode::PreciseQualityMatchWithCompression,
            Err(_) => crate::video_explorer::ExploreMode::PreciseQualityMatchWithCompression,
        }
    }
}


pub fn determine_output_path(
    input: &Path,
    extension: &str,
    output_dir: &Option<PathBuf>,
) -> Result<PathBuf, String> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let output = match output_dir {
        Some(dir) => {
            let _ = fs::create_dir_all(dir);
            dir.join(format!("{}.{}", stem, extension))
        }
        None => input.with_extension(extension),
    };

    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.clone())
    } else {
        output.clone()
    };

    if input_canonical == output_canonical || input == output {
        return Err(format!(
            "Input and output paths are identical: {}\n\
             Tip: use --output/-o for a different output dir, or --in-place to replace in place (deletes original)",
            input.display()
        ));
    }

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    Ok(output)
}

pub fn determine_output_path_with_base(
    input: &Path,
    base_dir: &Path,
    extension: &str,
    output_dir: &Option<PathBuf>,
) -> Result<PathBuf, String> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let output = match output_dir {
        Some(dir) => {
            let rel_path = input
                .strip_prefix(base_dir)
                .unwrap_or(input)
                .parent()
                .unwrap_or(Path::new(""));

            let out_subdir = dir.join(rel_path);
            let _ = fs::create_dir_all(&out_subdir);

            out_subdir.join(format!("{}.{}", stem, extension))
        }
        None => input.with_extension(extension),
    };

    let input_canonical = input.canonicalize().unwrap_or_else(|_| input.to_path_buf());
    let output_canonical = if output.exists() {
        output.canonicalize().unwrap_or_else(|_| output.clone())
    } else {
        output.clone()
    };

    if input_canonical == output_canonical || input == output {
        return Err(format!(
            "Input and output paths are identical: {}\n\
             Tip: use --output/-o for a different output dir, or --in-place to replace in place (deletes original)",
            input.display()
        ));
    }

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }

    Ok(output)
}


pub fn format_size_change(input_size: u64, output_size: u64) -> String {
    let reduction = 1.0 - (output_size as f64 / input_size as f64);
    let reduction_pct = reduction * 100.0;

    if reduction >= 0.0 {
        format!("size reduced {:.1}%", reduction_pct)
    } else {
        format!("size increased {:.1}%", -reduction_pct)
    }
}

pub fn calculate_size_reduction(input_size: u64, output_size: u64) -> f64 {
    (1.0 - (output_size as f64 / input_size as f64)) * 100.0
}


pub fn pre_conversion_check(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> Option<ConversionResult> {
    if !options.force && is_already_processed(input) {
        return Some(ConversionResult::skipped_duplicate(input));
    }

    if output.exists() && !options.force {
        return Some(ConversionResult::skipped_exists(input, output));
    }

    None
}


pub fn finalize_conversion(
    input: &Path,
    output: &Path,
    input_size: u64,
    format_name: &str,
    extra_info: Option<&str>,
    options: &ConvertOptions,
) -> std::io::Result<ConversionResult> {
    let output_size = std::fs::metadata(output)?.len();

    if let Err(e) = crate::preserve_metadata(input, output) {
        eprintln!("\u{26a0}\u{fe0f} Failed to preserve metadata: {}", e);
    }

    mark_as_processed(input);

    if options.should_delete_original() {
        let _ = safe_delete_original(input, output, 100);
    }

    Ok(ConversionResult::success(
        input,
        output,
        input_size,
        output_size,
        format_name,
        extra_info,
    ))
}

pub fn post_conversion_actions(
    input: &Path,
    output: &Path,
    options: &ConvertOptions,
) -> std::io::Result<()> {
    if let Err(e) = crate::preserve_metadata(input, output) {
        eprintln!("⚠️ Failed to preserve metadata: {}", e);
    }

    mark_as_processed(input);

    if options.should_delete_original() {
        safe_delete_original(input, output, 100)?;
    }

    Ok(())
}


/// Get image/video dimensions using ffprobe → image crate → ImageMagick fallback chain.
///
/// Returns (width, height) or an error if all methods fail.
pub fn get_input_dimensions(input: &Path) -> Result<(u32, u32), String> {
    // Method 1: ffprobe
    if let Ok(probe) = crate::probe_video(input) {
        if probe.width > 0 && probe.height > 0 {
            return Ok((probe.width, probe.height));
        }
    }

    // Method 2: image crate
    if let Ok((w, h)) = image::image_dimensions(input) {
        return Ok((w, h));
    }

    // Method 3: ImageMagick identify
    {
        use std::process::Command;
        let safe_path = crate::safe_path_arg(input);
        let output = Command::new("magick")
            .args(["identify", "-format", "%w %h\n"])
            .arg(safe_path.as_ref())
            .output()
            .or_else(|_| {
                Command::new("identify")
                    .args(["-format", "%w %h\n"])
                    .arg(safe_path.as_ref())
                    .output()
            });
        if let Ok(out) = output {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if let Some(line) = s.lines().next() {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let (Ok(w), Ok(h)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>())
                        {
                            if w > 0 && h > 0 {
                                return Ok((w, h));
                            }
                        }
                    }
                }
            }
        }
    }

    Err(format!(
        "Could not get file dimensions: {}\n\
         ffprobe, image crate, and ImageMagick identify all failed; check file integrity or install ffmpeg/ImageMagick",
        input.display(),
    ))
}


/// Check if output exceeds size tolerance and clean up if so.
///
/// Returns `Some(ConversionResult)` if the output is too large (caller should return it),
/// or `None` if the output passes the size check.
pub fn check_size_tolerance(
    input: &Path,
    output: &Path,
    input_size: u64,
    output_size: u64,
    options: &ConvertOptions,
    format_label: &str,
) -> Option<ConversionResult> {
    let tolerance_ratio = if options.allow_size_tolerance {
        1.01
    } else {
        1.0
    };
    let max_allowed_size = (input_size as f64 * tolerance_ratio) as u64;

    if output_size <= max_allowed_size {
        return None;
    }

    let size_increase_pct = ((output_size as f64 / input_size as f64) - 1.0) * 100.0;
    if let Err(e) = fs::remove_file(output) {
        eprintln!("⚠️ [cleanup] Failed to remove oversized output: {}", e);
    }
    if options.verbose {
        let mode = if options.allow_size_tolerance {
            "tolerance: 1.0%"
        } else {
            "strict mode: no tolerance"
        };
        eprintln!(
            "   ⏭️  Skipping: {} output larger than input by {:.1}% ({})",
            format_label, size_increase_pct, mode
        );
        eprintln!(
            "   📊 Size comparison: {} → {} bytes (+{:.1}%)",
            input_size, output_size, size_increase_pct
        );
    }

    let _ = crate::copy_on_skip_or_fail(
        input,
        options.output_dir.as_deref(),
        options.base_dir.as_deref(),
        false,
    );
    mark_as_processed(input);

    Some(ConversionResult::skipped_size_increase(
        input, input_size, output_size,
    ))
}
#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_strict_size_reduction_formula() {
        let test_cases = [
            (1000u64, 500u64, 50.0f64),
            (1000, 250, 75.0),
            (1000, 100, 90.0),
            (1000, 900, 10.0),
            (1000, 1000, 0.0),
            (1000, 2000, -100.0),
            (1000, 1500, -50.0),
        ];

        for (input, output, expected) in test_cases {
            let result = calculate_size_reduction(input, output);
            let expected_calc = (1.0 - (output as f64 / input as f64)) * 100.0;

            assert!(
                (result - expected).abs() < 0.001,
                "STRICT: {}->{}  expected {}, got {}",
                input,
                output,
                expected,
                result
            );
            assert!(
                (result - expected_calc).abs() < 0.0001,
                "STRICT: Formula mismatch for {}->{}",
                input,
                output
            );
        }
    }

    #[test]
    fn test_strict_large_file_sizes() {
        let reduction = calculate_size_reduction(10_000_000_000, 5_000_000_000);
        assert!(
            (reduction - 50.0).abs() < 0.001,
            "STRICT: 10GB->5GB should be exactly 50%, got {}",
            reduction
        );

        let reduction = calculate_size_reduction(100_000_000_000, 25_000_000_000);
        assert!(
            (reduction - 75.0).abs() < 0.001,
            "STRICT: 100GB->25GB should be exactly 75%, got {}",
            reduction
        );
    }

    #[test]
    fn test_strict_small_file_sizes() {
        let reduction = calculate_size_reduction(100, 50);
        assert!(
            (reduction - 50.0).abs() < 0.001,
            "STRICT: 100->50 bytes should be exactly 50%, got {}",
            reduction
        );
    }


    #[test]
    fn test_format_size_change_reduction() {
        let msg = format_size_change(1000, 500);
        assert!(
            msg.contains("reduced"),
            "Should say 'reduced' for smaller output"
        );
        assert!(msg.contains("50.0%"), "Should show 50.0% for half size");
    }

    #[test]
    fn test_format_size_change_increase() {
        let msg = format_size_change(500, 1000);
        assert!(
            msg.contains("increased"),
            "Should say 'increased' for larger output"
        );
        assert!(
            msg.contains("100.0%"),
            "Should show 100.0% for doubled size"
        );
    }

    #[test]
    fn test_format_size_change_no_change() {
        let msg = format_size_change(1000, 1000);
        assert!(msg.contains("reduced"), "Same size shows as 0% reduced");
        assert!(msg.contains("0.0%"), "Should show 0.0% for same size");
    }


    #[test]
    fn test_determine_output_path() {
        let input = Path::new("/path/to/image.png");
        let output = determine_output_path(input, "jxl", &None).unwrap();
        assert_eq!(output, Path::new("/path/to/image.jxl"));
    }

    #[test]
    fn test_determine_output_path_with_dir() {
        let input = Path::new("/path/to/image.png");
        let output_dir = Some(PathBuf::from("/output"));
        let output = determine_output_path(input, "avif", &output_dir).unwrap();
        assert_eq!(output, Path::new("/output/image.avif"));
    }

    #[test]
    fn test_determine_output_path_various_extensions() {
        let input = Path::new("/path/to/video.mp4");

        let webm = determine_output_path(input, "webm", &None).unwrap();
        assert_eq!(webm, Path::new("/path/to/video.webm"));

        let mkv = determine_output_path(input, "mkv", &None).unwrap();
        assert_eq!(mkv, Path::new("/path/to/video.mkv"));
    }


    #[test]
    fn test_conversion_result_success() {
        let input = Path::new("/test/input.png");
        let output = Path::new("/test/output.avif");

        let result = ConversionResult::success(input, output, 1000, 500, "AVIF", None);

        assert!(result.success);
        assert!(!result.skipped);
        assert_eq!(result.input_size, 1000);
        assert_eq!(result.output_size, Some(500));
        assert!((result.size_reduction.unwrap() - 50.0).abs() < 0.1);
        assert!(result.message.contains("reduced"));
    }

    #[test]
    fn test_conversion_result_size_increase() {
        let input = Path::new("/test/input.png");

        let result = ConversionResult::skipped_size_increase(input, 500, 1000);

        assert!(result.success);
        assert!(result.skipped);
        assert_eq!(result.skip_reason, Some("size_increase".to_string()));
        assert!(result.message.contains("larger"));
    }


    #[test]
    fn test_convert_options_default() {
        let opts = ConvertOptions::default();

        assert!(!opts.force);
        assert!(opts.output_dir.is_none());
        assert!(!opts.delete_original);
        assert!(!opts.in_place);
        assert!(!opts.should_delete_original());
    }

    #[test]
    fn test_convert_options_delete_original() {
        let mut opts = ConvertOptions::default();
        opts.delete_original = true;

        assert!(opts.should_delete_original());
    }

    #[test]
    fn test_convert_options_in_place() {
        let mut opts = ConvertOptions::default();
        opts.in_place = true;

        assert!(opts.should_delete_original());
    }


    #[test]
    fn test_flag_mode_with_gpu() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;
        opts.use_gpu = true;

        let mode = opts.flag_mode().unwrap();
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
        assert!(opts.use_gpu, "GPU should remain enabled");
    }

    #[test]
    fn test_flag_mode_with_cpu() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;
        opts.use_gpu = false;

        let mode = opts.flag_mode().unwrap();
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
        assert!(!opts.use_gpu, "CPU mode should remain disabled");
    }

    #[test]
    fn test_only_recommended_flags_valid_with_gpu_cpu() {
        let mut opts_gpu = ConvertOptions::default();
        opts_gpu.explore = true;
        opts_gpu.match_quality = true;
        opts_gpu.compress = true;
        opts_gpu.use_gpu = true;
        assert!(opts_gpu.flag_mode().is_ok());

        let mut opts_cpu = ConvertOptions::default();
        opts_cpu.explore = true;
        opts_cpu.match_quality = true;
        opts_cpu.compress = true;
        opts_cpu.use_gpu = false;
        assert!(opts_cpu.flag_mode().is_ok());

        assert_eq!(opts_gpu.flag_mode().unwrap(), opts_cpu.flag_mode().unwrap());
    }

    #[test]
    fn test_invalid_flag_combinations_rejected() {
        let invalid_combos = [
            (false, false, false),
            (false, false, true),
            (false, true, false),
            (true, false, false),
        ];

        for (explore, match_quality, compress) in invalid_combos {
            let mut opts = ConvertOptions::default();
            opts.explore = explore;
            opts.match_quality = match_quality;
            opts.compress = compress;
            assert!(
                opts.flag_mode().is_err(),
                "({}, {}, {}) should be invalid",
                explore,
                match_quality,
                compress
            );
        }
    }


    #[test]
    fn test_convert_options_all_flags_enabled() {
        let mut opts = ConvertOptions::default();
        opts.force = true;
        opts.delete_original = true;
        opts.in_place = true;
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;
        opts.apple_compat = true;
        opts.use_gpu = false;

        assert!(opts.force);
        assert!(opts.should_delete_original());
        assert!(opts.apple_compat);
        assert!(!opts.use_gpu);

        let mode = opts.flag_mode().unwrap();
        assert_eq!(
            mode,
            crate::flag_validator::FlagMode::PreciseQualityWithCompress
        );
    }

    #[test]
    fn test_convert_options_invalid_flag_combination() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = false;
        opts.compress = true;

        let result = opts.flag_mode();
        assert!(
            result.is_err(),
            "explore + compress without match_quality should be invalid"
        );
    }

    #[test]
    fn test_explore_mode_returns_precise_quality_with_compression() {
        let mut opts = ConvertOptions::default();
        opts.explore = true;
        opts.match_quality = true;
        opts.compress = true;

        assert_eq!(
            opts.explore_mode(),
            crate::video_explorer::ExploreMode::PreciseQualityMatchWithCompression,
        );
    }

}

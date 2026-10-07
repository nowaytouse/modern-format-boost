#![allow(unused_imports)]

use foundation::{
    log_anomaly, log_corruption, log_debug, log_detail, log_failure, log_fatal, log_hint,
    log_ignore, log_info, log_skip, log_success,
};

use img::lossless_converter::{ConvertOptions, convert_to_jxl};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn main() {
    log_detail!("Running test...");
    manual_debug_jxl_explorer_uses_copies_only();
}

fn debug_sample_paths(root: &Path) -> Vec<PathBuf> {
    [
        root.join("images/test_image_1080p.png"),
        root.join("images/IMG_0413.JPG"),
        root.join("images/IMG_8321.JPG"),
        root.join("images/poison_pill_grayscale_icc.jpg"),
    ]
    .into_iter()
    .filter(|path| path.exists() && path.is_file())
    .collect()
}

fn resolve_debug_root() -> PathBuf {
    let configured = std::env::var("MFB_DEBUG_DIR").unwrap_or_else(|_| "debug".into());
    let direct = PathBuf::from(&configured);
    if direct.exists() {
        return direct;
    }

    let workspace_relative = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(&configured);
    if workspace_relative.exists() {
        return workspace_relative;
    }

    direct
}

#[allow(clippy::too_many_lines)]
fn manual_debug_jxl_explorer_uses_copies_only() {
    if std::env::var("MFB_RUN_JXL_DEBUG_EXPLORER").is_err() {
        log_detail!(
            "Skipped manual JXL explorer debug test. To run set MFB_RUN_JXL_DEBUG_EXPLORER=1 and \
             optionally MFB_DEBUG_DIR=debug",
        );
        return;
    }

    let root = resolve_debug_root();
    if !root.exists() {
        log_detail!(
            "Debug path {} not found; set MFB_DEBUG_DIR to your local debug dir.",
            root.display(),
        );
        return;
    }

    let samples = debug_sample_paths(&root);
    if samples.is_empty() {
        log_detail!("No debug JXL samples found under {}.", root.display());
        return;
    }

    let temp = tempdir().unwrap_or_else(|e| panic!("failed to create temp dir: {e:?}"));
    let input_dir = temp.path().join("inputs");
    let output_dir = temp.path().join("outputs");
    let mfb_home = temp.path().join("mfb_home");
    fs::create_dir_all(&input_dir).unwrap_or_else(|e| panic!("failed to create inputs: {e:?}"));
    fs::create_dir_all(&output_dir).unwrap_or_else(|e| panic!("failed to create outputs: {e:?}"));
    fs::create_dir_all(&mfb_home).unwrap_or_else(|e| panic!("failed to create mfb_home: {e:?}"));
    unsafe { std::env::set_var("MFB_HOME_ROOT", &mfb_home) };
    foundation::init_ghost_mode().unwrap_or_else(|e| panic!("failed to init ghost mode: {e:?}"));

    let mut processed = 0usize;

    for sample in samples {
        let original_size = fs::metadata(&sample)
            .unwrap_or_else(|e| panic!("failed to read metadata: {e:?}"))
            .len();
        let copied_input = input_dir.join(
            sample
                .file_name()
                .unwrap_or_else(|| panic!("missing file name")),
        );
        fs::copy(&sample, &copied_input).unwrap_or_else(|e| panic!("failed to copy: {e:?}"));

        let mut working_input = copied_input.clone();
        let ext = if let Some(e) = copied_input
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
        {
            e
        } else {
            log_detail!(
                "debug sample {} has no extension; skipping format-specific handling",
                copied_input.display()
            );
            String::new()
        };

        if ext == "jpg" || ext == "jpeg" {
            let generated_png = input_dir.join(format!(
                "{}.generated.png",
                copied_input
                    .file_stem()
                    .unwrap_or(std::ffi::OsStr::new("sample"))
                    .to_string_lossy()
            ));
            foundation::image_detection::open_image_with_limits(&copied_input)
                .unwrap_or_else(|e| panic!("failed to open image: {e:?}"))
                .save(&generated_png)
                .unwrap_or_else(|e| panic!("failed to save png: {e:?}"));
            working_input = generated_png;
        }

        let analysis = foundation::image_analyzer::analyze_image(&working_input)
            .unwrap_or_else(|e| panic!("failed to analyze image: {e:?}"));

        let options = ConvertOptions {
            output_dir: Some(output_dir.clone()),
            flags: foundation::conversion::ConvertFlags::FORCE
                | foundation::conversion::ConvertFlags::EXPLORE
                | foundation::conversion::ConvertFlags::MATCH_QUALITY
                | foundation::conversion::ConvertFlags::COMPRESS
                | foundation::conversion::ConvertFlags::ULTIMATE,
            input_format: Some(analysis.format.clone()),
            ..Default::default()
        };

        let result = convert_to_jxl(
            &working_input,
            &options,
            foundation::constants::JXL_ULTIMATE_DISTANCE,
            analysis.conversion_color_context(),
        )
        .unwrap_or_else(|err| {
            panic!(
                "JXL debug exploration failed for {}: {err}",
                sample.display()
            )
        });

        log_detail!("debug sample {} -> {}", sample.display(), result.message);

        assert_eq!(
            fs::metadata(&sample)
                .unwrap_or_else(|e| panic!("failed to re-read metadata: {e:?}"))
                .len(),
            original_size,
            "original debug sample was modified: {}",
            sample.display()
        );

        processed += 1;
    }

    assert!(
        processed > 0,
        "expected at least one copied debug sample to run"
    );
    log_detail!(foundation::infra::static_logs::messages::VERIFICATION_COMPLETE);
}

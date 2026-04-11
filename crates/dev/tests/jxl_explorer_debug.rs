use img::lossless_converter::{convert_to_jxl, ConvertOptions};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn debug_sample_paths(root: &Path) -> Vec<PathBuf> {
    [
        root.join("inputs/test_image_1080p.png"),
        root.join("IMG_0413.JPG"),
        root.join("IMG_8321.JPG"),
        root.join("inputs/poison_pill_grayscale_icc.jpg"),
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

#[test]
fn manual_debug_jxl_explorer_uses_copies_only() {
    if std::env::var("MFB_RUN_JXL_DEBUG_EXPLORER").is_err() {
        eprintln!(
            "Skipped manual JXL explorer debug test. To run set MFB_RUN_JXL_DEBUG_EXPLORER=1 and optionally MFB_DEBUG_DIR=debug"
        );
        return;
    }

    let root = resolve_debug_root();
    if !root.exists() {
        eprintln!(
            "Debug path {} not found; set MFB_DEBUG_DIR to your local debug dir.",
            root.display()
        );
        return;
    }

    let samples = debug_sample_paths(&root);
    if samples.is_empty() {
        eprintln!("No debug JXL samples found under {}.", root.display());
        return;
    }

    let temp = tempdir().expect("create temp dir for copied debug media");
    let input_dir = temp.path().join("inputs");
    let output_dir = temp.path().join("outputs");
    let mfb_home = temp.path().join("mfb_home");
    fs::create_dir_all(&input_dir).expect("create copied-input dir");
    fs::create_dir_all(&output_dir).expect("create output dir");
    fs::create_dir_all(&mfb_home).expect("create isolated MFB home root");
    std::env::set_var("MFB_HOME_ROOT", &mfb_home);
    shared_utils::init_ghost_mode().expect("initialize MFB temp workspace for debug exploration");

    let mut processed = 0usize;

    for sample in samples {
        let original_size = fs::metadata(&sample)
            .expect("read original sample metadata")
            .len();
        let copied_input = input_dir.join(sample.file_name().expect("sample file name"));
        fs::copy(&sample, &copied_input).expect("copy debug sample into temp workspace");

        let mut working_input = copied_input.clone();
        let ext = copied_input
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .unwrap_or_default();

        if ext == "jpg" || ext == "jpeg" {
            let generated_png = input_dir.join(format!(
                "{}.generated.png",
                copied_input
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("sample")
            ));
            shared_utils::image_detection::open_image_with_limits(&copied_input)
                .expect("decode copied JPEG for generated PNG fixture")
                .save(&generated_png)
                .expect("save generated PNG fixture");
            working_input = generated_png;
        }

        let analysis = shared_utils::image_analyzer::analyze_image(&working_input)
            .expect("analyze working input");

        let options = ConvertOptions {
            output_dir: Some(output_dir.clone()),
            force: true,
            explore: true,
            match_quality: true,
            compress: true,
            ultimate: true,
            input_format: Some(analysis.format.clone()),
            ..Default::default()
        };

        let result = convert_to_jxl(
            &working_input,
            &options,
            shared_utils::constants::JXL_ULTIMATE_DISTANCE,
            analysis.hdr_info.as_ref(),
        )
        .unwrap_or_else(|err| {
            panic!(
                "JXL debug exploration failed for {}: {err}",
                sample.display()
            )
        });

        eprintln!("debug sample {} -> {}", sample.display(), result.message);

        assert_eq!(
            fs::metadata(&sample)
                .expect("re-read original sample metadata after conversion")
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
}

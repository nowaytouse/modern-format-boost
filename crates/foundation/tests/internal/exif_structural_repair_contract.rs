// CONTRACT: Apple iCloud on-demand structural repair (metadata/exif.rs v8.2.2).
// Pure gate + argv tests always run; do not add early-return skips here.

use super::{
    append_jxl_metadata_rehydrate_without_orientation_args, append_nuclear_repair_exiftool,
    append_source_metadata_copy_args, is_nuclear_format_extension, should_run_structural_repair,
    stderr_triggers_structural_repair,
};
use crate::builder_base::ToolBuilder;
use std::path::Path;

#[test]
fn contract_nuclear_format_extensions_locked() {
    for (ext, expected) in [
        ("jxl", true),
        ("jpg", true),
        ("jpeg", true),
        ("webp", true),
        ("png", false),
        ("mp4", false),
        ("", false),
    ] {
        assert_eq!(
            is_nuclear_format_extension(ext),
            expected,
            "CONTRACT nuclear ext gate failed for {ext:?}"
        );
    }
}

#[test]
fn contract_extension_fallback_stderr_triggers_locked() {
    use super::stderr_triggers_extension_fallback;
    assert!(stderr_triggers_extension_fallback(
        "Error: Not a valid JPEG (looks more like a PNG)"
    ));
    assert!(!stderr_triggers_extension_fallback("Warning: minor tag issue"));
}

#[test]
fn contract_stderr_triggers_locked() {
    assert!(stderr_triggers_structural_repair("Error: Not a valid JPEG"));
    assert!(stderr_triggers_structural_repair("file is corrupt"));
    assert!(stderr_triggers_structural_repair("invalid metadata"));
    assert!(stderr_triggers_structural_repair("truncated EXIF"));
    assert!(stderr_triggers_structural_repair("Not a valid JPG"));
    assert!(!stderr_triggers_structural_repair("Warning: minor tag issue"));
    assert!(!stderr_triggers_structural_repair(""));
}

#[test]
fn contract_should_run_structural_repair_gate_locked() {
    assert!(!should_run_structural_repair(
        false,
        "jpg",
        false,
        "Not a valid"
    ));
    assert!(!should_run_structural_repair(true, "png", false, "Error"));
    assert!(!should_run_structural_repair(
        true,
        "jpg",
        true,
        "Error"
    ));
    assert!(!should_run_structural_repair(
        true,
        "jpg",
        false,
        "Warning only"
    ));
    assert!(should_run_structural_repair(
        true,
        "jpeg",
        false,
        "Error: Not a valid JPG (looks more like a PNG)"
    ));
}

#[test]
fn contract_nuclear_repair_exiftool_argv_locked() {
    let mut builder = crate::ExiftoolBuilder::new();
    append_nuclear_repair_exiftool(&mut builder, Path::new("src.jpg"), "jpg");
    builder.input(Path::new("dst.jpg"));
    let args: Vec<String> = builder
        .build()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    let all_pos = args
        .iter()
        .position(|a| a == crate::constants::EXIFTOOL_ARG_ALL)
        .expect("CONTRACT: nuclear repair must include -all=");
    let at_pos = args
        .iter()
        .position(|a| a == "@")
        .expect("CONTRACT: nuclear repair must restore from @");
    let src_pos = args
        .iter()
        .position(|a| a.contains("src.jpg"))
        .expect("CONTRACT: nuclear repair must tagsfromfile source");

    assert!(
        all_pos < at_pos,
        "CONTRACT: -all= must precede @ restore"
    );
    assert!(
        at_pos < src_pos,
        "CONTRACT: @ restore must precede source tagsfromfile"
    );
    assert!(
        args.iter()
            .filter(|a| *a == crate::constants::EXIFTOOL_ARG_UNSAFE)
            .count()
            >= 2,
        "CONTRACT: nuclear repair must pass -unsafe twice"
    );
    assert!(
        !args.iter().any(|arg| arg.eq_ignore_ascii_case("MWG")),
        "CONTRACT: nuclear repair must copy physical tags without synthesizing MWG composites"
    );
}

#[test]
fn contract_jxl_source_metadata_copy_excludes_orientation() {
    let mut builder = crate::ExiftoolBuilder::new();
    append_source_metadata_copy_args(&mut builder, Path::new("src.jpg"), true);
    builder.input(Path::new("dst.jxl"));
    let args: Vec<String> = builder
        .build()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    let source_pos = args
        .iter()
        .position(|a| a.contains("src.jpg"))
        .expect("CONTRACT: metadata copy must read source");
    let strip_pos = args
        .iter()
        .position(|a| a == "-Orientation=")
        .expect("CONTRACT: JXL metadata copy must exclude Orientation upstream");

    assert!(
        source_pos < strip_pos,
        "CONTRACT: Orientation exclusion must be part of source copy argv"
    );
    assert!(
        args.iter().any(|a| a == "-all:Orientation="),
        "CONTRACT: JXL metadata copy must exclude grouped Orientation"
    );
}

#[test]
fn contract_jxl_rehydrate_uses_block_exclusion_not_tag_allowlist() {
    let mut builder = crate::ExiftoolBuilder::new();
    append_jxl_metadata_rehydrate_without_orientation_args(
        &mut builder,
        Path::new("source.jpg"),
        true,
    );
    builder.input(Path::new("output.jxl"));
    let args: Vec<String> = builder
        .build()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    let source_pos = args
        .iter()
        .position(|a| a.contains("source.jpg"))
        .expect("CONTRACT: JXL rehydrate must copy from source");
    let all_tags_pos = args
        .iter()
        .position(|a| a == "-all:all")
        .expect("CONTRACT: JXL rehydrate must copy all source tags");
    let orientation_exclusion_pos = args
        .iter()
        .position(|a| a == "--Orientation")
        .expect("CONTRACT: JXL rehydrate must block Orientation during source copy");

    assert!(
        source_pos < all_tags_pos && all_tags_pos < orientation_exclusion_pos,
        "CONTRACT: JXL rehydrate must be tagsFromFile source -all:all --Orientation"
    );
    assert!(
        args.iter().any(|a| a == "-all:Orientation="),
        "CONTRACT: JXL rehydrate must clear grouped Orientation from destination"
    );
    assert!(
        !args.iter().any(|a| a == crate::constants::EXIFTOOL_ARG_ALL),
        "CONTRACT: JXL rehydrate must not run another broad metadata strip"
    );
    assert!(
        args.iter().any(|a| a == "-ICC_Profile<ICC_Profile"),
        "CONTRACT: JXL rehydrate can copy ICC when caller reports no embedded ICC"
    );
    assert!(
        !args.iter().any(|arg| arg.eq_ignore_ascii_case("MWG")),
        "CONTRACT: JXL rehydrate must copy physical tags without synthesizing MWG composites"
    );
}

#[test]
fn contract_first_pass_metadata_copy_has_no_strip_all() {
    let mut builder = crate::ExiftoolBuilder::new();
    builder
        .overwrite_original()
        .tags_from_file(Path::new("src.jpg"))
        .arg("-all:all")
        .unsafe_tags()
        .arg("-ICC_Profile<ICC_Profile")
        .input(Path::new("dst.jpg"));
    let args: Vec<String> = builder
        .build()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();

    assert!(
        !args.iter().any(|a| a == crate::constants::EXIFTOOL_ARG_ALL),
        "CONTRACT: first-pass copy must not use -all= (on-demand nuclear only)"
    );
}

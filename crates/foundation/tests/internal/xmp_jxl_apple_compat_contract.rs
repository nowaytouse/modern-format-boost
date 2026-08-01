// CONTRACT: JXL + Apple compat XMP merge nuclear strip (xmp_merger.rs).

use super::{append_jxl_apple_nuclear_xmp_merge, should_jxl_xmp_apple_nuclear_strip};
use crate::builder_base::ToolBuilder;
use std::path::Path;

#[test]
fn contract_jxl_xmp_nuclear_strip_gate_locked() {
    let jxl = Path::new("photo.jxl");
    let jpg = Path::new("photo.jpg");
    assert!(should_jxl_xmp_apple_nuclear_strip(jxl, true));
    assert!(!should_jxl_xmp_apple_nuclear_strip(jxl, false));
    assert!(!should_jxl_xmp_apple_nuclear_strip(jpg, true));
}

#[test]
fn contract_jxl_xmp_nuclear_merge_argv_locked() {
    let mut builder = crate::ExiftoolBuilder::new();
    append_jxl_apple_nuclear_xmp_merge(&mut builder);
    builder.input(Path::new("out.jxl"));
    let args: Vec<String> = builder
        .build()
        .get_args()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let all_pos = args
        .iter()
        .position(|a| a == crate::constants::EXIFTOOL_ARG_ALL)
        .expect("CONTRACT: JXL Apple XMP merge must strip with -all=");
    let restore_pos = args
        .windows(2)
        .position(|w| {
            w[0] == crate::constants::EXIFTOOL_ARG_TAGS_FROM_FILE && w[1].contains('@')
        })
        .expect("CONTRACT: JXL Apple XMP merge must restore from @ via -tagsfromfile");
    assert!(
        all_pos < restore_pos,
        "CONTRACT: -all= must precede @ restore in JXL Apple XMP merge"
    );
}

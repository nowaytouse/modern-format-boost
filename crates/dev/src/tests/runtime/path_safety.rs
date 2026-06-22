use foundation::path_safety::{exiftool_path_arg, magick_safe_path};
use std::path::Path;

#[test]
fn path_safety_suite() {
    test_sers_truncation_defense();
    test_exiftool_argument_injection_defense();
}

fn test_sers_truncation_defense() {
    // Poison Pill: absolute path that triggers V7 pointer offset bug
    let path = Path::new("crates/dev/src/tests/edge/images/poison_pill_grayscale_icc.jpg");
    let safe_path = magick_safe_path(path);

    // With Relativization Shield, it should be a safe relative path
    assert!(
        safe_path.ends_with("poison_pill_grayscale_icc.jpg"),
        "Incorrect path: {safe_path}"
    );
    assert!(
        safe_path.starts_with("./"),
        "Path should be relative: {safe_path}"
    );
}

fn test_exiftool_argument_injection_defense() {
    // Poison Pill: Filename that looks like an exiftool argument
    let path = Path::new("crates/dev/src/tests/edge/images/poison_pill_exiftool_-execute.jpg");
    let safe_path = exiftool_path_arg(path);

    // Should prepend ./ to prevent exiftool from executing the command
    assert!(
        safe_path.starts_with("./"),
        "ExifTool path failed to prepend ./ for injection safety: {safe_path}"
    );
    assert!(
        safe_path.contains("-execute"),
        "Lost injection payload: {safe_path}"
    );
}

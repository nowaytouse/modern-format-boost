use std::io::Write;
use tempfile::NamedTempFile;

use shared_utils::image_detection::{detect_animation, detect_format_from_bytes, DetectedFormat};
use shared_utils::constants::{JXL_HEADER_LONG, JXL_HEADER_SHORT};

#[test]
fn jxl_static_detected_from_short_header_is_static() {
    let mut f = NamedTempFile::new().expect("tempfile");
    let mut header = [0u8; 32];
    header[..JXL_HEADER_SHORT.len()].copy_from_slice(JXL_HEADER_SHORT);
    f.write_all(&header).expect("write header");

    let path = f.path();
    let fmt = detect_format_from_bytes(path).expect("detect format");
    assert!(matches!(fmt, DetectedFormat::JXL));

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt).expect("detect animation");
    assert!(!is_animated, "short-header JXL should be treated as static when ffprobe/djxl unavailable");
    assert_eq!(frame_count, Some(1));
}

#[test]
fn jxl_static_detected_from_long_header_is_static() {
    let mut f = NamedTempFile::new().expect("tempfile");
    let mut header = [0u8; 32];
    header[..JXL_HEADER_LONG.len()].copy_from_slice(JXL_HEADER_LONG);
    f.write_all(&header).expect("write header");

    let path = f.path();
    let fmt = detect_format_from_bytes(path).expect("detect format");
    assert!(matches!(fmt, DetectedFormat::JXL));

    let (is_animated, frame_count, _fps) = detect_animation(path, &fmt).expect("detect animation");
    assert!(!is_animated, "long-header JXL should be treated as static when ffprobe/djxl unavailable");
    assert_eq!(frame_count, Some(1));
}

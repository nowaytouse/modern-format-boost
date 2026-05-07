use shared_utils::numeric_cast::{
    f64_to_u8_strict, f64_to_u32_strict, f64_to_u64_strict, i64_to_u64_strict, option_f32_strict,
    option_f64_strict, option_u64_strict, parse_strict, u64_to_u32_strict,
};

#[test]
fn strict_option_helpers_preserve_missing_metadata() {
    assert_eq!(option_f64_strict(None, "fps"), None);
    assert_eq!(option_f32_strict(None, "ssim"), None);
    assert_eq!(option_u64_strict(None, "frame_count"), None);

    assert_eq!(option_f64_strict(Some(23.976), "fps"), Some(23.976));
    assert_eq!(option_u64_strict(Some(240), "frame_count"), Some(240));
}

#[test]
fn strict_numeric_casts_refuse_forged_values() {
    assert_eq!(f64_to_u64_strict(f64::NAN, "duration_ms"), None);
    assert_eq!(f64_to_u64_strict(f64::INFINITY, "duration_ms"), None);
    assert_eq!(f64_to_u64_strict(-1.0, "duration_ms"), None);
    assert_eq!(f64_to_u32_strict(4_294_967_296.0, "width"), None);
    assert_eq!(i64_to_u64_strict(-1, "file_size"), None);
    assert_eq!(
        u64_to_u32_strict(u64::from(u32::MAX) + 1, "frame_count"),
        None
    );

    assert_eq!(f64_to_u8_strict(42.0, "quality"), Some(42));
}

#[test]
fn strict_parser_refuses_malformed_numeric_text() {
    assert_eq!(parse_strict::<u64>("not-a-number", "bitrate"), None);
    assert_eq!(parse_strict::<u32>("", "width"), None);
    assert_eq!(parse_strict::<u32>("3840", "width"), Some(3840));
}

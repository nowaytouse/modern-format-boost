#[path = "edge/gifs/synth_webp.rs"]
mod synth_webp;

#[test]
fn webp_duration_parser_suite() {
    parse_duration_from_synthetic_animated_webp_anmf_payloads();
}

fn parse_duration_from_synthetic_animated_webp_anmf_payloads() {
    let bytes = synth_webp::build_synthetic_animated_webp_without_vp8x_in_header();
    let dur = shared_utils::image_formats::webp::duration_secs_from_bytes(&bytes)
        .unwrap_or_else(|| panic!("duration should parse from ANMF payloads"));
    // 100ms + 120ms = 220ms
    assert!((dur - 0.22).abs() < 0.001, "duration={dur}");
}

use foundation::quality_matcher::SourceCodec;

#[test]
fn gif_header_stays_in_animated_image_domain() {
    let codec = SourceCodec::identify_by_header(b"GIF89a\x01\x00\x01\x00\x00\x00");

    assert_eq!(codec, Some(SourceCodec::Gif));
    assert!(SourceCodec::Gif.is_animated());
}

#[test]
fn webp_vp8x_animation_flag_routes_to_animated_domain() {
    let codec =
        SourceCodec::identify_by_header(b"RIFF\x00\x00\x00\x00WEBPVP8X\x00\x00\x00\x00\x02");

    assert_eq!(codec, Some(SourceCodec::WebpAnimated));
    assert!(SourceCodec::WebpAnimated.is_animated());
}

#[test]
fn webp_without_animation_flag_stays_static() {
    let codec =
        SourceCodec::identify_by_header(b"RIFF\x00\x00\x00\x00WEBPVP8X\x00\x00\x00\x00\x00");

    assert_eq!(codec, Some(SourceCodec::WebpStatic));
    assert!(!SourceCodec::WebpStatic.is_animated());
}

#[test]
fn png_without_actl_stays_static_image_domain() {
    let codec = SourceCodec::identify_by_header(b"\x89PNG\r\n\x1a\n");

    assert_eq!(codec, Some(SourceCodec::Png));
    assert!(!SourceCodec::Png.is_animated());
}

#[test]
fn apng_actl_header_routes_to_animated_image_domain() {
    let mut header = b"\x89PNG\r\n\x1a\n".to_vec();
    header.extend_from_slice(&[0; 29]);
    header.extend_from_slice(b"acTL");

    let codec = SourceCodec::identify_by_header(&header);

    assert_eq!(codec, Some(SourceCodec::Apng));
    assert!(SourceCodec::Apng.is_animated());
}

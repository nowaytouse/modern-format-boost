#[test]
fn vid_ignores_static_png() -> anyhow::Result<()> {
    // Minimal 1x1 RGBA PNG — bytes generated with correct CRC32 on IHDR/IDAT/IEND chunks.
    // Previous fixture had a corrupted IDAT CRC that caused FFprobe to reject it.
    const ONE_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR length + type
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // width=1, height=1
        0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, // bit depth=8, color=RGBA, CRC
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, // IDAT length + type
        0x78, 0xDA, 0x63, 0x60, 0x60, 0x60, 0xF8, 0x0F, // zlib-compressed scanline
        0x00, 0x01, 0x04, 0x01, 0x00, 0x80, 0xBB, 0xD1, 0x5B, // data + correct CRC
        0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82, // IEND
    ];

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("one.png");
    std::fs::write(&path, ONE_PNG)?;

    let config = foundation::conversion_types::ConversionConfig::default();

    // Call into vid's public API
    let out = vid::auto_convert_with_cache(&path, &config, None)?;

    assert!(out.ignored, "expected output to be ignored: {out:?}");
    assert_eq!(
        out.strategy.target,
        foundation::conversion_types::TargetVideoFormat::Ignored
    );
    assert!(
        out.message.starts_with("IGNORED:"),
        "message = {msg}",
        msg = out.message
    );

    Ok(())
}

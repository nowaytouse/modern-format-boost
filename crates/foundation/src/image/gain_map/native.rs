//! Native jhgm layout, as documented by libjxl's `extras/gain_map.cc` and
//! golden bundle tests. jpegxl-rs's vendored build excludes that extras API;
//! use checked byte serialization and its existing codec for the naked image.
//! No C ABI duplication, second codec, or dependency version change is needed.

use super::{Result, Semantics, Source, ensure};
use anyhow::Context;

pub(super) fn encode(source: &Source) -> Result<Vec<u8>> {
    let mut encoder = jpegxl_rs::encoder_builder()
        .lossless(true)
        .quality(0.0)
        .use_container(false)
        .uses_original_profile(true)
        .color_encoding(jpegxl_rs::encode::ColorEncoding::LinearSrgb)
        .build()?;
    let pixels = encoder
        .encode::<u8, u8>(
            source.pixels.as_raw(),
            source.pixels.width(),
            source.pixels.height(),
        )
        .context("lossless native gain-map encoding failed")?;
    ensure!(
        pixels.data.starts_with(&[0xFF, 0x0A]),
        "gain map is not a naked JXL codestream"
    );
    // No alternate profile means inherit the baseline profile. Only sources
    // explicitly using the base color space reach this path (Semantics::validate).
    let size = source
        .iso
        .len()
        .checked_add(pixels.data.len())
        .and_then(|size| size.checked_add(8))
        .context("gain-map size overflow")?;
    ensure!(
        size <= 256 * 1024 * 1024,
        "gain-map bundle exceeds 256 MiB safety limit"
    );
    let mut output = Vec::with_capacity(size);
    output.push(0); // jhgm version
    output.extend_from_slice(&u16::try_from(source.iso.len())?.to_be_bytes());
    output.extend_from_slice(&source.iso);
    output.extend_from_slice(&[0; 5]); // color encoding size + compressed ICC size
    output.extend_from_slice(&pixels.data);
    Ok(output)
}

pub(super) fn verify(source: &Source, input: &[u8]) -> Result<()> {
    ensure!(
        input.len() >= 8 && input.len() <= 256 * 1024 * 1024,
        "invalid gain-map bundle size"
    );
    ensure!(input[0] == 0, "unsupported jhgm version");
    let end = 3 + usize::from(u16::from_be_bytes([input[1], input[2]]));
    let iso = input
        .get(3..end)
        .context("truncated gain-map ISO metadata")?;
    ensure!(
        input.get(end..end + 5) == Some(&[0; 5]),
        "gain-map inherited color interpretation changed"
    );
    ensure!(iso == source.iso, "gain-map metadata bytes changed");
    let semantics = Semantics::decode(iso)?;
    ensure!(
        semantics == source.semantics,
        "gain-map metadata semantics changed"
    );
    let codestream = input
        .get(end + 5..)
        .context("truncated gain-map codestream")?;
    ensure!(
        codestream.starts_with(&[0xFF, 0x0A]),
        "gain map is not a naked JXL codestream"
    );
    let decoder = jpegxl_rs::decoder_builder()
        .skip_reorientation(true)
        .build()?;
    let (metadata, pixels) = decoder.decode_with::<u8>(codestream)?;
    ensure!(
        metadata.width == source.pixels.width()
            && metadata.height == source.pixels.height()
            && metadata.num_color_channels == 3,
        "gain-map dimensions or channels changed"
    );
    ensure!(pixels == *source.pixels.as_raw(), "gain-map pixels changed");
    // Pixel equality above plus equality of the transfer for every possible
    // sample proves all gain-map locations, without repeating expensive powf
    // evaluations for every pixel of a large image.
    for sample in 0..=255_u8 {
        for channel in 0..3 {
            for weight in [0.0, 0.5, 1.0] {
                for base in [0.0, 0.18, 1.0] {
                    let expected = source.semantics.reconstruct(
                        base,
                        f64::from(sample) / 255.0,
                        channel,
                        weight,
                    );
                    let actual =
                        semantics.reconstruct(base, f64::from(sample) / 255.0, channel, weight);
                    ensure!(
                        expected.is_finite() && actual.to_bits() == expected.to_bits(),
                        "gain-map HDR reconstruction changed"
                    );
                }
            }
        }
    }
    Ok(())
}

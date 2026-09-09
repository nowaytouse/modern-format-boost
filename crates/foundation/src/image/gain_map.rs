//! Native JPEG XL gain maps, separate from reversible JPEG reconstruction.
//!
//! Metadata layout follows ISO 21496-1 (also implemented by libultrahdr's
//! `gainmapmetadata.cpp`); original ISO bytes are retained without rewriting.

use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;
use std::path::Path;

#[cfg(feature = "jpegxl-ffi")]
mod native;

const ISO_NAMESPACE: &[u8] = b"urn:iso:std:iso:ts:21496:-1\0";
const GAIN_NAMESPACE: &str = "http://ns.adobe.com/hdr-gain-map/1.0/";
const FIELDS: [&str; 5] = [
    "GainMapMin",
    "GainMapMax",
    "Gamma",
    "OffsetSDR",
    "OffsetHDR",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fraction {
    numerator: i64,
    denominator: u32,
}

impl Fraction {
    fn new(mut numerator: i64, mut denominator: u64) -> Result<Self> {
        ensure!(denominator != 0, "gain-map denominator is zero");
        let (mut a, mut b) = (numerator.unsigned_abs(), denominator);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        numerator /= i64::try_from(a)?;
        denominator /= a;
        Ok(Self {
            numerator,
            denominator: u32::try_from(denominator)?,
        })
    }

    fn decimal(text: &str) -> Result<Self> {
        let text = text.trim();
        let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
        let exponent: i32 = exponent.parse()?;
        ensure!(
            exponent.unsigned_abs() <= 18,
            "gain-map decimal exponent is out of range"
        );
        ensure!(
            mantissa.matches('.').count() <= 1,
            "invalid gain-map decimal"
        );
        let decimals = match mantissa.split_once('.') {
            Some((_, tail)) => tail.len(),
            None => 0, // Integer syntax has no fractional digits, not missing metadata.
        };
        let mut numerator: i64 = mantissa.replace('.', "").parse()?;
        let power = i32::try_from(decimals)? - exponent;
        let denominator = if power < 0 {
            numerator = numerator
                .checked_mul(
                    10_i64
                        .checked_pow(power.unsigned_abs())
                        .context("gain-map decimal overflow")?,
                )
                .context("gain-map numerator overflow")?;
            1
        } else {
            10_u64
                .checked_pow(u32::try_from(power)?)
                .context("gain-map denominator overflow")?
        };
        Self::new(numerator, denominator)
    }

    #[allow(clippy::cast_precision_loss)] // Numerators are constrained to 32 bits before use.
    fn value(self) -> f64 {
        self.numerator as f64 / f64::from(self.denominator)
    }

    fn write(self, out: &mut Vec<u8>, signed: bool) -> Result<()> {
        if signed {
            out.extend_from_slice(&i32::try_from(self.numerator)?.to_be_bytes());
        } else {
            out.extend_from_slice(&u32::try_from(self.numerator)?.to_be_bytes());
        }
        out.extend_from_slice(&self.denominator.to_be_bytes());
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Semantics {
    headroom: [Fraction; 2],
    channels: [[Fraction; 5]; 3],
    backward: bool,
    use_base_color: bool,
}

impl Semantics {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.use_base_color,
            "gain map uses a distinct alternate color space; its profile must be mapped before conversion"
        );
        let headroom = self.headroom.map(Fraction::value);
        ensure!(
            headroom.iter().all(|x| *x >= 0.0 && x.is_finite())
                && self.headroom[0] != self.headroom[1],
            "invalid gain-map HDR headroom"
        );
        for channel in self.channels {
            let values = channel.map(Fraction::value);
            ensure!(
                values.iter().all(|x| x.is_finite()) && values[0] <= values[1] && values[2] > 0.0,
                "invalid gain-map range or gamma"
            );
            for (index, fraction) in channel.into_iter().enumerate() {
                fraction.write(&mut Vec::new(), index != 2)?;
            }
        }
        Ok(())
    }

    fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut out = vec![0, 0, 0, 0, 0xC0 | if self.backward { 4 } else { 0 }];
        for headroom in self.headroom {
            headroom.write(&mut out, false)?;
        }
        for channel in self.channels {
            for (index, fraction) in channel.into_iter().enumerate() {
                fraction.write(&mut out, index != 2)?;
            }
        }
        Ok(out)
    }

    fn decode(data: &[u8]) -> Result<Self> {
        ensure!(
            data.len() >= 5 && data[..2] == [0, 0],
            "unsupported ISO gain-map metadata version"
        );
        let flags = data[4];
        ensure!(flags & !0xCC == 0, "reserved ISO gain-map flags are set");
        let mut data = &data[5..];
        let denominator = if flags & 8 != 0 {
            Some(read_u32(&mut data)?)
        } else {
            None
        };
        let mut fraction = |signed: bool| -> Result<Fraction> {
            let value = read_u32(&mut data)?;
            let numerator = if signed {
                i64::from(i32::from_be_bytes(value.to_be_bytes()))
            } else {
                i64::from(value)
            };
            Fraction::new(
                numerator,
                u64::from(match denominator {
                    Some(d) => d,
                    None => read_u32(&mut data)?,
                }),
            )
        };
        let headroom = [fraction(false)?, fraction(false)?];
        let mut channels = [[Fraction {
            numerator: 0,
            denominator: 1,
        }; 5]; 3];
        let channel_count = if flags & 0x80 != 0 { 3 } else { 1 };
        for channel in channels.iter_mut().take(channel_count) {
            for (index, value) in channel.iter_mut().enumerate() {
                *value = fraction(index != 2)?;
            }
        }
        if channel_count == 1 {
            channels[1] = channels[0];
            channels[2] = channels[0];
        }
        ensure!(data.is_empty(), "trailing ISO gain-map metadata bytes");
        let value = Self {
            headroom,
            channels,
            backward: flags & 4 != 0,
            use_base_color: flags & 0x40 != 0,
        };
        value.validate()?;
        Ok(value)
    }

    // ISO/Adobe reconstruction in the shared linear base color space. Comparing
    // all gain samples at several headrooms also catches direction/offset errors.
    fn reconstruct(&self, base: f64, gain: f64, channel: usize, weight: f64) -> f64 {
        let [min, max, gamma, base_offset, alt_offset] =
            self.channels[channel].map(Fraction::value);
        let log_gain = (max - min).mul_add(gain.powf(1.0 / gamma), min);
        let weight = if self.backward { -weight } else { weight };
        (base + base_offset).mul_add((log_gain * weight).exp2(), -alt_offset)
    }
}

fn read_u32(data: &mut &[u8]) -> Result<u32> {
    let bytes: [u8; 4] = data
        .get(..4)
        .context("truncated ISO gain-map metadata")?
        .try_into()?;
    *data = &data[4..];
    Ok(u32::from_be_bytes(bytes))
}

fn xmp_semantics(xmp: &str) -> Result<Option<Semantics>> {
    use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
    let mut reader = NsReader::from_str(xmp);
    let mut properties: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut active: Option<(String, usize)> = None;
    let mut depth = 0_usize;
    loop {
        let event = reader.read_event()?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                let (namespace, name) = reader.resolver().resolve_element(element.name());
                if matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == GAIN_NAMESPACE) {
                    ensure!(active.is_none(), "nested gain-map property");
                    ensure!(!empty, "empty gain-map property");
                    ensure!(
                        !properties.contains_key(name.as_ref()),
                        "duplicate gain-map property"
                    );
                    active = Some((name.as_ref().to_owned(), depth));
                }
                for attribute in element.attributes() {
                    let attribute = attribute?;
                    let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                    if matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == GAIN_NAMESPACE)
                    {
                        let name = name.as_ref().to_owned();
                        ensure!(
                            !properties.contains_key(&name),
                            "duplicate gain-map property {name}"
                        );
                        properties.insert(
                            name,
                            vec![
                                attribute
                                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                                    .into_owned(),
                            ],
                        );
                    }
                }
                if !empty {
                    depth += 1;
                    ensure!(depth <= 64, "gain-map XMP nesting limit");
                }
            }
            Event::Text(text) => {
                if let Some((name, _)) = &active {
                    let text = text.xml10_content().trim().to_owned();
                    if !text.is_empty() {
                        properties.entry(name.clone()).or_default().push(text);
                    }
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1).context("unbalanced gain-map XMP")?;
                if active.as_ref().is_some_and(|(_, level)| *level == depth) {
                    active = None;
                }
            }
            Event::DocType(_) => bail!("DTD is forbidden in gain-map XMP"),
            Event::CData(_) | Event::GeneralRef(_) if active.is_some() => {
                bail!("unsupported gain-map value encoding")
            }
            Event::Eof => {
                ensure!(depth == 0, "unclosed gain-map XMP");
                break;
            }
            _ => {}
        }
    }
    if !properties.contains_key("GainMapMax") {
        return Ok(None);
    }
    ensure!(
        properties.get("Version").is_some_and(|v| v == &["1.0"]),
        "unsupported Adobe gain-map version"
    );
    ensure!(
        properties.contains_key("HDRCapacityMax"),
        "required HDRCapacityMax is missing"
    );
    ensure!(
        !properties
            .get("BaseRenditionIsHDR")
            .is_some_and(|v| v != &["False"] && v != &["false"]),
        "Adobe HDR-base gain maps require inverse metadata mapping"
    );
    let mut channels = [[Fraction {
        numerator: 0,
        denominator: 1,
    }; 5]; 3];
    for (index, field) in FIELDS.iter().enumerate() {
        let default = match index {
            2 => "1",
            3 | 4 => "0.015625",
            _ => "0",
        };
        let values = properties
            .get(*field)
            .cloned()
            .unwrap_or_else(|| vec![default.to_owned()]);
        ensure!(
            values.len() == 1 || values.len() == 3,
            "{field} must have one or three channels"
        );
        for (channel_index, channel) in channels.iter_mut().enumerate() {
            channel[index] =
                Fraction::decimal(&values[if values.len() == 1 { 0 } else { channel_index }])?;
        }
    }
    let mut headroom = [Fraction {
        numerator: 0,
        denominator: 1,
    }; 2];
    for (index, field) in ["HDRCapacityMin", "HDRCapacityMax"].into_iter().enumerate() {
        let values = properties
            .get(field)
            .cloned()
            .unwrap_or_else(|| vec![if index == 0 { "0" } else { "1" }.to_owned()]);
        ensure!(values.len() == 1, "{field} must be scalar");
        headroom[index] = Fraction::decimal(&values[0])?;
    }
    let result = Semantics {
        headroom,
        channels,
        backward: false,
        use_base_color: true,
    };
    ensure!(
        result.headroom[0].value() < result.headroom[1].value(),
        "HDRCapacityMax must exceed HDRCapacityMin"
    );
    ensure!(
        result
            .channels
            .iter()
            .all(|channel| channel[3].numerator >= 0 && channel[4].numerator >= 0),
        "Adobe gain-map offsets must be nonnegative"
    );
    result.validate()?;
    Ok(Some(result))
}

fn iso_payload(jpeg: &[u8]) -> Result<Option<&[u8]>> {
    let mut pos = 2;
    let mut found = None;
    while pos + 1 < jpeg.len() {
        ensure!(jpeg[pos] == 0xFF, "invalid JPEG gain-map marker");
        if jpeg[pos + 1] == 0xFF {
            pos += 1;
            continue;
        }
        let marker = jpeg[pos + 1];
        pos += 2;
        if matches!(marker, 0xDA | 0xD9) {
            break;
        }
        if marker == 1 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        let length = jpeg
            .get(pos..pos + 2)
            .context("truncated JPEG gain-map segment")?;
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        ensure!(length >= 2, "invalid JPEG gain-map segment length");
        let payload = jpeg
            .get(pos + 2..pos + length)
            .context("truncated JPEG gain-map payload")?;
        if marker == 0xE2
            && let Some(iso) = payload.strip_prefix(ISO_NAMESPACE)
        {
            ensure!(
                found.replace(iso).is_none(),
                "duplicate ISO gain-map metadata"
            );
        }
        pos += length;
    }
    Ok(found)
}

struct Source {
    pixels: image::RgbImage,
    iso: Vec<u8>,
    semantics: Semantics,
}

fn source(jpeg: &[u8]) -> Result<Option<Source>> {
    if !super::image_jpeg_analysis::is_ultra_hdr_jpeg(jpeg) {
        return Ok(None);
    }
    let payload = super::image_jpeg_analysis::extract_native_gain_map_source(jpeg)
        .map_err(anyhow::Error::msg)?;
    let (iso, semantics) = if let Some(iso) = iso_payload(&payload.gainmap_jpeg)? {
        (iso.to_vec(), Semantics::decode(iso)?)
    } else {
        let mut semantics = None;
        // The secondary image is authoritative. Older writers placed the full
        // parameters in the primary image, so accept that placement as fallback.
        for data in [&payload.gainmap_jpeg[..], jpeg] {
            let Some(packets) = super::image_jpeg_analysis::extract_xmp_from_jpeg_data(data) else {
                // Try the documented primary placement; neither image may
                // silently succeed without the required semantics below.
                continue;
            };
            for xmp in packets {
                if let Some(parsed) = xmp_semantics(&xmp)? {
                    ensure!(
                        semantics
                            .as_ref()
                            .is_none_or(|existing| existing == &parsed),
                        "conflicting gain-map XMP semantics"
                    );
                    semantics = Some(parsed);
                }
            }
            if semantics.is_some() {
                break;
            }
        }
        let semantics = semantics.context("UltraHDR gain-map parameters are missing")?;
        (semantics.encode()?, semantics)
    };
    ensure!(
        matches!(
            payload.gainmap_image,
            image::DynamicImage::ImageLuma8(_) | image::DynamicImage::ImageRgb8(_)
        ),
        "gain-map JPEG is not 8-bit gray/RGB"
    );
    Ok(Some(Source {
        pixels: payload.gainmap_image.to_rgb8(),
        iso,
        semantics,
    }))
}

/// Attach and verify a native `jhgm` without changing any existing JXL bytes.
/// Returns false for ordinary JPEGs. Does not modify the source JPEG.
///
/// # Errors
/// Returns an error if a detected gain map cannot be extracted, mapped, encoded,
/// read back, or proven equivalent. The caller must not publish that candidate.
pub fn attach_jpeg_gain_map(jpeg: &Path, candidate: &Path) -> Result<bool> {
    let Some(source) = source(&std::fs::read(jpeg)?)? else {
        return Ok(false);
    };
    #[cfg(feature = "jpegxl-ffi")]
    {
        let bundle = native::encode(&source)?;
        native::verify(&source, &bundle)?;
        crate::metadata::append_gain_map_to_jxl(candidate, &bundle)?;
        verify_jpeg_gain_map(jpeg, candidate)?;
        Ok(true)
    }
    #[cfg(not(feature = "jpegxl-ffi"))]
    {
        let _ = (source, candidate);
        bail!("native JPEG XL gain-map support is disabled in this build")
    }
}

/// Verify native gain-map pixels, ISO semantics and HDR reconstruction against
/// the original JPEG. Whole-JPEG reconstruction is a separate, mandatory gate.
///
/// # Errors
/// Returns an error for missing, duplicate, malformed or non-equivalent gain maps.
pub fn verify_jpeg_gain_map(jpeg: &Path, candidate: &Path) -> Result<()> {
    let Some(source) = source(&std::fs::read(jpeg)?)? else {
        return Ok(());
    };
    #[cfg(feature = "jpegxl-ffi")]
    {
        native::verify(
            &source,
            &crate::metadata::read_gain_map_from_jxl(candidate)?,
        )
    }
    #[cfg(not(feature = "jpegxl-ffi"))]
    {
        let _ = (source, candidate);
        bail!("native JPEG XL gain-map verification is disabled in this build")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xmp() -> &'static str {
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:g="http://ns.adobe.com/hdr-gain-map/1.0/" g:Version="1.0" g:GainMapMax="2" g:HDRCapacityMax="2"><g:Gamma><rdf:Seq><rdf:li>1</rdf:li><rdf:li>2</rdf:li><rdf:li>0.5</rdf:li></rdf:Seq></g:Gamma></rdf:Description></rdf:RDF></x:xmpmeta>"#
    }

    #[test]
    fn gain_map_semantic_roundtrip_and_malformed_inputs() -> Result<()> {
        let model = xmp_semantics(xmp())?.context("gain-map XMP")?;
        assert_eq!(model.channels[0][3], Fraction::new(1, 64)?);
        assert_eq!(model.channels.map(|c| c[2].value()), [1.0, 2.0, 0.5]);
        assert_eq!(Fraction::decimal("1.5625e-2")?, Fraction::new(1, 64)?);
        let encoded = model.encode()?;
        assert_eq!(Semantics::decode(&encoded)?, model);
        // Independent expected endpoints: log gain 0 -> 2, base 1/4, offset 1/64.
        assert_eq!(model.reconstruct(0.25, 0.0, 0, 1.0), 0.25);
        assert_eq!(model.reconstruct(0.25, 1.0, 0, 1.0), 1.046_875);
        for length in 0..encoded.len() {
            assert!(Semantics::decode(&encoded[..length]).is_err());
        }
        let mut bad = encoded.clone();
        bad[4] |= 1;
        assert!(Semantics::decode(&bad).is_err());
        bad = encoded;
        bad[9..13].fill(0); // first headroom denominator
        assert!(Semantics::decode(&bad).is_err());
        assert!(xmp_semantics(&xmp().replace("<rdf:li>2</rdf:li>", "")).is_err());
        assert!(
            xmp_semantics(&xmp().replace("g:GainMapMax=\"2\"", "g:GainMapMax=\"NaN\"")).is_err()
        );
        assert!(Fraction::decimal("1.2.3").is_err());
        assert!(Fraction::decimal("1e-2147483648").is_err());
        Ok(())
    }

    #[test]
    fn gain_map_compatible_writer_and_reducible_decimal() -> Result<()> {
        let model = xmp_semantics(xmp())?.context("gain-map XMP")?;
        let mut encoded = model.encode()?;
        encoded[3] = 1; // Newer writer, but minimum reader version is still zero.
        assert_eq!(Semantics::decode(&encoded)?, model);
        encoded[1] = 1;
        assert!(Semantics::decode(&encoded).is_err());
        let padded = xmp()
            .replace("g:GainMapMax=\"2\"", "g:GainMapMax=\"2.0000000000\"")
            .replace(
                "g:HDRCapacityMax=\"2\"",
                "g:HDRCapacityMax=\"2.0000000000\"",
            )
            .replace("<rdf:li>1</rdf:li>", "<rdf:li>1.0000000000</rdf:li>");
        assert_eq!(xmp_semantics(&padded)?, Some(model));
        assert_eq!(Fraction::decimal("0.0156250000")?, Fraction::new(1, 64)?);
        assert_eq!(Fraction::decimal("0.0009765625")?, Fraction::new(1, 1024)?);
        Ok(())
    }

    #[cfg(feature = "jpegxl-ffi")]
    #[test]
    fn native_gain_map_preserves_pixels_semantics_and_container_prefix() -> Result<()> {
        let semantics = xmp_semantics(xmp())?.context("gain-map XMP")?;
        let source = Source {
            pixels: image::RgbImage::from_fn(8, 8, |x, y| {
                image::Rgb([
                    u8::try_from(x * 31).unwrap(),
                    u8::try_from(y * 31).unwrap(),
                    127,
                ])
            }),
            iso: semantics.encode()?,
            semantics,
        };
        let bundle = native::encode(&source)?;
        native::verify(&source, &bundle)?;
        let mut corrupted = bundle.clone();
        corrupted[3 + 13] ^= 1;
        assert!(native::verify(&source, &corrupted).is_err());
        for length in [0, 1, 3, bundle.len() - 1] {
            assert!(native::verify(&source, &bundle[..length]).is_err());
        }

        let folder = tempfile::tempdir()?;
        let path = folder.path().join("candidate.jxl");
        let mut encoder = jpegxl_rs::encoder_builder()
            .lossless(true)
            .quality(0.0)
            .use_container(true)
            .uses_original_profile(true)
            .color_encoding(jpegxl_rs::encode::ColorEncoding::Srgb)
            .build()?;
        let original = encoder.encode::<u8, u8>(source.pixels.as_raw(), 8, 8)?.data;
        std::fs::write(&path, &original)?;
        assert!(crate::metadata::read_gain_map_from_jxl(&path).is_err());
        crate::metadata::append_gain_map_to_jxl(&path, &bundle)?;
        native::verify(&source, &crate::metadata::read_gain_map_from_jxl(&path)?)?;
        // Independent container consumer: jxlinfo calls libjxl's native
        // JxlGainMapReadBundle (the vendored Rust codec omits that extras API).
        if crate::tool_builders::JxlinfoBuilder::check_available() {
            use crate::builder_base::ToolBuilder;
            let mut command = crate::tool_builders::JxlinfoBuilder::new()
                .input(&path)
                .build();
            command.arg("-v");
            let output = crate::process_runner::run_command_with_liveness_timeout(
                &mut command,
                std::time::Duration::from_secs(10),
                std::time::Duration::from_secs(30),
                "native gain-map independent readback",
            )?;
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout)
                    .contains("Gain map (jhgm) box: version = 0"),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(!String::from_utf8_lossy(&output.stderr).contains("Invalid gain map"));
            eprintln!(
                "Independent libjxl jhgm readback: {}",
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .find(|line| line.contains("Gain map (jhgm)"))
                    .unwrap_or("missing")
            );
        } else {
            eprintln!("Independent libjxl jhgm readback not run: jxlinfo unavailable");
        }
        let final_bytes = std::fs::read(&path)?;
        assert_eq!(&final_bytes[..original.len()], original);
        crate::metadata::append_gain_map_to_jxl(&path, &bundle)?;
        assert_eq!(std::fs::read(&path)?, final_bytes, "append is idempotent");
        assert!(crate::metadata::append_gain_map_to_jxl(&path, &corrupted).is_err());
        assert_eq!(
            std::fs::read(&path)?,
            final_bytes,
            "conflict never rewrites the original"
        );
        Ok(())
    }
}

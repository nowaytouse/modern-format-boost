use foundation::image_jpeg_analysis::{extract_gainmap_from_jpeg, is_ultra_hdr_jpeg};

fn download_ultrahdr_sample(path: &std::path::Path) -> anyhow::Result<()> {
    const EXPECTED_BLAKE3: &str =
        "4a10e63837ba6e01b8957d2448b6cd4bd31cbebe7546497a3eecb8e7afaa2037";
    const EXPECTED_BYTES: u64 = 2_746_718;
    // Both official endpoints identify the unchanged fixture; a mirror cannot
    // silently replace it with a different JPEG or an HTTP error page.
    let urls = [
        "https://raw.githubusercontent.com/MishaalRahmanGH/Ultra_HDR_Samples/1bc32bb721d821224c2e4e0c012183b932be379b/Originals/Ultra_HDR_Samples_Originals_01.jpg",
        "https://api.github.com/repos/MishaalRahmanGH/Ultra_HDR_Samples/git/blobs/3355b466fee1f1bc59e40463f8aadcf2d216dca3",
    ];
    let mut failures = Vec::new();
    for url in urls {
        let output = std::process::Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--location",
                "--connect-timeout",
                "15",
                "--max-time",
                "30",
                "--retry",
                "1",
                "--retry-max-time",
                "30",
                "--retry-all-errors",
                "--header",
                "Accept: application/vnd.github.raw+json",
            ])
            .arg("--max-filesize")
            .arg(EXPECTED_BYTES.to_string())
            .arg(url)
            .arg("--output")
            .arg(path)
            .output()?;
        let reason = if output.status.success() {
            let length = std::fs::metadata(path)?.len();
            if length != EXPECTED_BYTES {
                format!("fixture length mismatch: expected {EXPECTED_BYTES}, got {length}")
            } else {
                let hash = foundation::common_utils::calculate_blake3_hash(path)?;
                if hash == EXPECTED_BLAKE3 {
                    return Ok(());
                }
                format!("fixture BLAKE3 mismatch: {hash}")
            }
        } else {
            format!(
                "download failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
        };
        eprintln!("Ultra HDR fixture endpoint {url}: {reason}");
        failures.push(format!("{url}: {reason}"));
    }
    anyhow::bail!(
        "All official Ultra HDR fixture endpoints failed: {}",
        failures.join("; ")
    )
}

#[test]
fn ultrahdr_hardening_suite() -> anyhow::Result<()> {
    test_ultrahdr_absolute_offset_fallback()?;
    Ok(())
}

#[test]
fn ultrahdr_real_sample_gainmap_extraction_requires_network() -> anyhow::Result<()> {
    test_real_ultrahdr_samples_from_github()?;
    Ok(())
}

#[test]
fn ultrahdr_to_jxl_conversion_requires_network_and_cjxl() -> anyhow::Result<()> {
    test_ultrahdr_to_jxl_conversion()?;
    Ok(())
}

fn test_ultrahdr_to_jxl_conversion() -> anyhow::Result<()> {
    use foundation::hdr::{IntermediateFormat, convert_ultrahdr_jpeg_to_jxl};
    use std::process::Command;
    use tempfile::TempDir;

    let version = Command::new("cjxl").arg("--version").output()?;
    anyhow::ensure!(
        version.status.success(),
        "Required cjxl tool failed ({}): {}",
        version.status,
        String::from_utf8_lossy(&version.stderr)
    );

    let temp = TempDir::new()?;
    let sample_path = temp.path().join("ultrahdr_sample_convert.jpg");
    download_ultrahdr_sample(&sample_path)?;

    let output_jxl = temp.path().join("output.jxl");

    let result = convert_ultrahdr_jpeg_to_jxl(
        &sample_path,
        &output_jxl,
        false, // apple_compat
        IntermediateFormat::Png16, /* Use PNG16 as it's typically faster/more compatible without
                * OpenEXR lib setup */
        false, // ultimate
        false, // archive
    );

    match result {
        Ok(artifacts) => {
            assert!(output_jxl.exists(), "Output JXL file should exist");
            assert!(
                std::fs::metadata(&output_jxl)?.len() > 1000,
                "Output JXL should be a substantial file"
            );
            assert!(
                artifacts.sidecar_count() >= 1,
                "UltraHDR synthesis should preserve at least one sidecar artifact"
            );
        }
        Err(e) => {
            panic!("❌ Conversion failed: {e}");
        }
    }

    Ok(())
}

fn test_real_ultrahdr_samples_from_github() -> anyhow::Result<()> {
    use tempfile::TempDir;

    let temp = TempDir::new()?;
    let sample_path = temp.path().join("ultrahdr_sample_01.jpg");
    download_ultrahdr_sample(&sample_path)?;

    let data = std::fs::read(&sample_path)?;

    assert!(
        is_ultra_hdr_jpeg(&data),
        "Real sample should be identified as Ultra HDR"
    );

    let result = extract_gainmap_from_jpeg(&data);
    match result {
        Ok((base_img, gainmap_img)) => {
            // Verify it has significant visual data
            assert!(
                base_img.width() > 0 && gainmap_img.width() > 0,
                "Images should have positive dimensions"
            );
        }
        Err(e) => {
            panic!("❌ Failed to extract gainmap from real Ultra HDR sample: {e}");
        }
    }

    Ok(())
}

fn test_ultrahdr_absolute_offset_fallback() -> anyhow::Result<()> {
    // 1) Build a clean, valid JPEG structure (SOI -> APP1 -> APP2 -> SOS -> EOI)
    let mut data = vec![0xFF, 0xD8]; // SOI

    // 2) APP1 XMP
    let xmp_content = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description hdrgm:GainMapMax=\"2.0\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\"/></rdf:RDF></x:xmpmeta>";
    let xmp_hdr = b"http://ns.adobe.com/xap/1.0/\0";
    let xmp_seg_len = u16::try_from(xmp_hdr.len() + xmp_content.len() + 2).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse integer or missing required value: XMP segment length calculation \
             overflow"
        )
    })?;

    data.push(0xFF);
    data.push(0xE1);
    data.extend_from_slice(&xmp_seg_len.to_be_bytes());
    data.extend_from_slice(xmp_hdr);
    data.extend_from_slice(xmp_content);

    // 3) APP2 MPF
    let mpf_id = b"MPF\0";
    let tiff_hdr = b"MM\0*"; // Big Endian
    let ifd_offset = 8u32;

    let mut mpf_payload = Vec::new();
    mpf_payload.extend_from_slice(tiff_hdr);
    mpf_payload.extend_from_slice(&ifd_offset.to_be_bytes());
    // IFD: 2 entries
    mpf_payload.extend_from_slice(&2u16.to_be_bytes());
    // Entry 1: NumberOfImages (Tag 0xB001)
    mpf_payload.extend_from_slice(&0xB001u16.to_be_bytes());
    mpf_payload.extend_from_slice(&4u16.to_be_bytes()); // LONG
    mpf_payload.extend_from_slice(&1u32.to_be_bytes());
    mpf_payload.extend_from_slice(&2u32.to_be_bytes());
    // Entry 2: MPEntry (Tag 0xB002)
    let mp_entry_val_offset = u32::try_from(mpf_payload.len() + 12 + 4).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse integer or missing required value: MP entry offset calculation \
             overflow"
        )
    })?;
    mpf_payload.extend_from_slice(&0xB002u16.to_be_bytes());
    mpf_payload.extend_from_slice(&7u16.to_be_bytes()); // UNDEFINED
    mpf_payload.extend_from_slice(&32u32.to_be_bytes()); // 2 images * 16 bytes
    mpf_payload.extend_from_slice(&mp_entry_val_offset.to_be_bytes());
    // Next IFD offset
    mpf_payload.extend_from_slice(&0u32.to_be_bytes());

    // MP Entries array
    // Primary
    mpf_payload.extend_from_slice(&[0u8; 16]);
    // Gainmap
    let gainmap_size = 10u32;
    // We want to force an ABSOLUTE offset that is valid, but RELATIVE would be
    // invalid. Let's place the gainmap at the VERY end of the file.
    let absolute_offset = 1000u32; // Just pick a large enough fixed offset
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Attributes
    mpf_payload.extend_from_slice(&gainmap_size.to_be_bytes());
    mpf_payload.extend_from_slice(&absolute_offset.to_be_bytes());
    mpf_payload.extend_from_slice(&0u32.to_be_bytes()); // Deps

    let mpf_seg_len = u16::try_from(mpf_id.len() + mpf_payload.len() + 2).map_err(|_| {
        anyhow::anyhow!(
            "Failed to parse integer or missing required value: MPF segment length calculation \
             overflow"
        )
    })?;
    data.push(0xFF);
    data.push(0xE2);
    data.extend_from_slice(&mpf_seg_len.to_be_bytes());
    data.extend_from_slice(mpf_id);
    data.extend_from_slice(&mpf_payload);

    // 4) Main Image content placeholders to reach absolute_offset
    let absolute_offset_usize = usize::try_from(absolute_offset)
        .map_err(|_| anyhow::anyhow!("absolute_offset does not fit usize"))?;
    while data.len() < absolute_offset_usize {
        data.push(0);
    }

    // 5) Gainmap data at absolute_offset
    let gainmap_img = vec![0xFF, 0xD8, 0xFF, 0xDB, 0, 0, 0, 0, 0xFF, 0xD9]; // Minimal JPEG
    // Ensure we don't overwrite if absolute_offset was somehow reached early
    data.truncate(absolute_offset_usize);
    data.extend_from_slice(&gainmap_img);

    // 6) Close main JPEG with EOI
    data.push(0xFF);
    data.push(0xD9);

    assert!(is_ultra_hdr_jpeg(&data), "Should be identified as UltraHDR");

    let result = extract_gainmap_from_jpeg(&data);
    match result {
        Ok(_) => {}
        Err(e) => {
            if e.contains("No MPF") {
                panic!("❌ Failed to find MPF: {e}");
            } else if e.contains("Failed to decode base JPEG")
                || e.contains("Failed to create JPEG reader")
            {
                // Expected: this synthetic file intentionally proves MPF offset
                // handling without carrying decodable base or
                // gainmap JPEG fixtures.
            } else {
                panic!("Unexpected Ultra HDR extraction error: {e}");
            }
        }
    }

    Ok(())
}

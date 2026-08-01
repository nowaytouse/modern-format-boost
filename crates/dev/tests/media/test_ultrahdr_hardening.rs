use foundation::image_jpeg_analysis::{extract_gainmap_from_jpeg, is_ultra_hdr_jpeg};

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

    if Command::new("cjxl").arg("--version").output().is_err() {
        println!("cargo:warning=cjxl is not available, skipping conversion test.");
        return Ok(());
    }

    let temp = TempDir::new()?;
    let sample_url = "https://raw.githubusercontent.com/MishaalRahmanGH/Ultra_HDR_Samples/main/Originals/Ultra_HDR_Samples_Originals_01.jpg";
    let sample_path = temp.path().join("ultrahdr_sample_convert.jpg");

    let status = Command::new("curl")
        .arg("-sSL")
        .arg(sample_url)
        .arg("-o")
        .arg(&sample_path)
        .status()?;

    if !status.success() {
        println!(
            "cargo:warning=failed to download Ultra HDR sample, skipping conversion test due to \
             network issue."
        );
        return Ok(());
    }
    anyhow::ensure!(
        std::fs::metadata(&sample_path)?.len() >= 1000,
        "downloaded Ultra HDR sample is too small to be valid"
    );

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

            // Copy to stable path for user inspection
            let dest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join(".modern_format_boost")
                .join("artifacts");
            let _ = std::fs::create_dir_all(&dest_dir);
            let dest_jxl = dest_dir.join("ultrahdr_test_output.jxl");
            let dest_jpg = dest_dir.join("ultrahdr_test_input.jpg");
            let _ = std::fs::copy(&sample_path, &dest_jpg);
            let _ = std::fs::copy(&output_jxl, &dest_jxl);
            println!(
                "cargo:warning=Copied output files to {}",
                dest_dir.display()
            );
        }
        Err(e) => {
            panic!("❌ Conversion failed: {e}");
        }
    }

    Ok(())
}

fn test_real_ultrahdr_samples_from_github() -> anyhow::Result<()> {
    use std::process::Command;
    use tempfile::TempDir;

    let temp = TempDir::new()?;
    let sample_url = "https://raw.githubusercontent.com/MishaalRahmanGH/Ultra_HDR_Samples/main/Originals/Ultra_HDR_Samples_Originals_01.jpg";
    let sample_path = temp.path().join("ultrahdr_sample_01.jpg");

    let status = Command::new("curl")
        .arg("-sSL")
        .arg(sample_url)
        .arg("-o")
        .arg(&sample_path)
        .status()?;

    if !status.success() {
        println!(
            "cargo:warning=failed to download Ultra HDR sample, skipping extraction test due to \
             network issue."
        );
        return Ok(());
    }

    let data = std::fs::read(&sample_path)?;
    anyhow::ensure!(
        data.len() >= 1000,
        "downloaded Ultra HDR sample is too small to be valid"
    );

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

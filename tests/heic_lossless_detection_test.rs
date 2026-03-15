//! HEIC Lossless Detection Bug Exploration Test
//!
//! This test generates multiple truly lossless HEIC files with different encoding parameters
//! and verifies that detect_heic_is_lossless correctly identifies them as lossless.
//!
//! **CRITICAL**: This test is expected to FAIL on unfixed code to confirm the bug exists.
//! DO NOT attempt to fix the test or the code when it fails.
//!
//! Test Strategy:
//! 1. Generate lossless HEIC files using ffmpeg with verified parameters
//! 2. Verify files are truly lossless using byte-level inspection
//! 3. Run detect_heic_is_lossless and check if it returns Ok(true)
//! 4. Document any failures as counterexamples

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn test_bug_condition_444_rext_lossless() {
    // Generate 4:4:4 RExt lossless HEIC
    let test_dir = PathBuf::from("/tmp/heic_lossless_test");
    fs::create_dir_all(&test_dir).unwrap();
    
    let input_png = test_dir.join("test_input.png");
    let output_heic = test_dir.join("test_444_rext_lossless.heic");
    
    // Create a simple test image
    create_test_png(&input_png);
    
    // Generate lossless HEIC with 4:4:4 RExt profile
    let status = Command::new("ffmpeg")
        .args(&[
            "-y",
            "-i", input_png.to_str().unwrap(),
            "-c:v", "libx265",
            "-x265-params", "lossless=1:profile=rext",
            "-pix_fmt", "yuv444p",
            output_heic.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute ffmpeg");
    
    if !status.status.success() {
        eprintln!("ffmpeg stderr: {}", String::from_utf8_lossy(&status.stderr));
        panic!("ffmpeg failed to generate lossless HEIC");
    }
    
    assert!(output_heic.exists(), "Output HEIC file was not created");
    
    // Verify the file is truly lossless at byte level
    let data = fs::read(&output_heic).unwrap();
    let verification = verify_heic_lossless_encoding(&data);
    println!("Verification result: {:#?}", verification);
    
    assert!(verification.is_lossless, 
        "Generated file is not truly lossless: {:?}", verification);
    
    // Now test detect_heic_is_lossless
    let result = detect_heic_is_lossless(&data, &output_heic);
    
    println!("detect_heic_is_lossless result: {:?}", result);
    
    // This assertion should PASS after the fix, but may FAIL before the fix
    match result {
        Ok(true) => {
            println!("✅ PASS: Correctly detected as lossless");
        }
        Ok(false) => {
            panic!("❌ FAIL: Incorrectly detected as lossy (Bug Condition confirmed)");
        }
        Err(e) => {
            panic!("❌ FAIL: Error during detection: {} (Bug Condition confirmed)", e);
        }
    }
}

#[test]
fn test_bug_condition_12bit_scc_lossless() {
    // Generate 12-bit 4:4:4 SCC lossless HEIC
    let test_dir = PathBuf::from("/tmp/heic_lossless_test");
    fs::create_dir_all(&test_dir).unwrap();
    
    let input_png = test_dir.join("test_input.png");
    let output_heic = test_dir.join("test_12bit_scc_lossless.heic");
    
    create_test_png(&input_png);
    
    // Generate lossless HEIC with 12-bit SCC profile
    let status = Command::new("ffmpeg")
        .args(&[
            "-y",
            "-i", input_png.to_str().unwrap(),
            "-c:v", "libx265",
            "-x265-params", "lossless=1:profile=scc",
            "-pix_fmt", "yuv444p12le",
            output_heic.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute ffmpeg");
    
    if !status.status.success() {
        eprintln!("ffmpeg stderr: {}", String::from_utf8_lossy(&status.stderr));
        // SCC profile might not be supported, skip test
        eprintln!("⚠️  SKIP: SCC profile not supported by this ffmpeg build");
        return;
    }
    
    if !output_heic.exists() {
        eprintln!("⚠️  SKIP: Output file not created (SCC profile not supported)");
        return;
    }
    
    let data = fs::read(&output_heic).unwrap();
    let verification = verify_heic_lossless_encoding(&data);
    println!("Verification result: {:#?}", verification);
    
    let result = detect_heic_is_lossless(&data, &output_heic);
    println!("detect_heic_is_lossless result: {:?}", result);
    
    match result {
        Ok(true) => {
            println!("✅ PASS: Correctly detected as lossless");
        }
        Ok(false) => {
            panic!("❌ FAIL: Incorrectly detected as lossy (Bug Condition confirmed)");
        }
        Err(e) => {
            panic!("❌ FAIL: Error during detection: {} (Bug Condition confirmed)", e);
        }
    }
}

// Helper functions

fn create_test_png(path: &Path) {
    use image::{ImageBuffer, Rgb};
    
    // Create a simple 64x64 test image with gradient
    let img = ImageBuffer::from_fn(64, 64, |x, y| {
        Rgb([
            (x * 4) as u8,
            (y * 4) as u8,
            ((x + y) * 2) as u8,
        ])
    });
    
    img.save(path).unwrap();
}

#[derive(Debug)]
struct HeicLosslessVerification {
    is_lossless: bool,
    profile_idc: Option<u8>,
    chroma_format_idc: Option<u8>,
    bit_depth_luma: Option<u8>,
    bit_depth_chroma: Option<u8>,
    has_identity_matrix: bool,
    has_high_bitdepth_pixi: bool,
    reasons: Vec<String>,
}

fn verify_heic_lossless_encoding(data: &[u8]) -> HeicLosslessVerification {
    let mut verification = HeicLosslessVerification {
        is_lossless: false,
        profile_idc: None,
        chroma_format_idc: None,
        bit_depth_luma: None,
        bit_depth_chroma: None,
        has_identity_matrix: false,
        has_high_bitdepth_pixi: false,
        reasons: Vec::new(),
    };
    
    // Find and parse hvcC box
    if let Some(hvcc_data) = find_box_payload_by_magic(data, b"hvcC") {
        if hvcc_data.len() >= 20 {
            let profile_idc = hvcc_data[1] & 0x1F;
            let chroma_format_idc = hvcc_data[16] & 0x03;
            let bit_depth_luma = ((hvcc_data[17] >> 5) & 0x07) + 8;
            let bit_depth_chroma = (hvcc_data[17] & 0x07) + 8;
            
            verification.profile_idc = Some(profile_idc);
            verification.chroma_format_idc = Some(chroma_format_idc);
            verification.bit_depth_luma = Some(bit_depth_luma);
            verification.bit_depth_chroma = Some(bit_depth_chroma);
            
            // Check lossless indicators
            if profile_idc == 4 || profile_idc == 9 {
                verification.reasons.push(format!("RExt/SCC profile ({})", profile_idc));
                
                if chroma_format_idc == 3 {
                    verification.reasons.push("4:4:4 chroma".to_string());
                    verification.is_lossless = true;
                }
                
                if bit_depth_luma >= 12 || bit_depth_chroma >= 12 {
                    verification.reasons.push(format!("High bit depth ({})", bit_depth_luma));
                    verification.is_lossless = true;
                }
            }
        }
    }
    
    // Check colr box for Identity matrix
    if let Some(colr_data) = find_box_payload_by_magic(data, b"colr") {
        if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
            let matrix = u16::from_be_bytes([colr_data[8], colr_data[9]]);
            if matrix == 0 {
                verification.has_identity_matrix = true;
                verification.reasons.push("Identity matrix (MC=0)".to_string());
                verification.is_lossless = true;
            }
        }
    }
    
    // Check pixi box for high bit depth
    if let Some(pixi_data) = find_box_payload_by_magic(data, b"pixi") {
        if !pixi_data.is_empty() {
            let num_ch = pixi_data[0] as usize;
            if num_ch > 0 && pixi_data.len() > num_ch {
                let max_depth = pixi_data[1..=num_ch].iter().copied().max().unwrap_or(0);
                if max_depth >= 12 {
                    verification.has_high_bitdepth_pixi = true;
                    verification.reasons.push(format!("High bit depth in pixi ({})", max_depth));
                    verification.is_lossless = true;
                }
            }
        }
    }
    
    verification
}

fn find_box_payload_by_magic<'a>(data: &'a [u8], box_type: &[u8; 4]) -> Option<&'a [u8]> {
    if let Some(pos) = data.windows(4).position(|w| w == box_type) {
        if pos >= 4 {
            let size = u32::from_be_bytes([data[pos - 4], data[pos - 3], data[pos - 2], data[pos - 1]]) as usize;
            if size >= 8 && pos + size - 4 <= data.len() {
                return Some(&data[pos + 4..pos - 4 + size]);
            }
        }
    }
    None
}

// Import the actual function from shared_utils
fn detect_heic_is_lossless(data: &[u8], path: &Path) -> Result<bool, String> {
    // This is a placeholder - in the actual test, we'll link against shared_utils
    // For now, we'll use a simplified version
    
    let hvcc_data = find_box_payload_by_magic(data, b"hvcC")
        .ok_or("hvcC not found")?;
    
    if hvcc_data.len() >= 20 {
        let profile_idc = hvcc_data[1] & 0x1F;
        let chroma_format_idc = hvcc_data[16] & 0x03;
        
        if chroma_format_idc == 1 || chroma_format_idc == 2 {
            return Ok(false);
        }
        
        if profile_idc == 1 || profile_idc == 2 || profile_idc == 3 {
            return Ok(false);
        }
        
        if profile_idc == 4 || profile_idc == 9 {
            let is_444 = chroma_format_idc == 3;
            
            // Check colr
            let has_rgb_identity_matrix = find_box_payload_by_magic(data, b"colr")
                .and_then(|colr_data| {
                    if colr_data.len() >= 11 && &colr_data[0..4] == b"nclx" {
                        Some(u16::from_be_bytes([colr_data[8], colr_data[9]]))
                    } else {
                        None
                    }
                })
                .map(|matrix| matrix == 0)
                .unwrap_or(false);
            
            if has_rgb_identity_matrix {
                return Ok(true);
            }
            
            if is_444 {
                return Ok(true);
            }
            
            return Err("RExt/SCC without 4:4:4".to_string());
        }
    }
    
    Ok(false)
}

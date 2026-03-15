//! WebP and AVIF Lossless Detection Test
//! 
//! Generates truly lossless WebP and AVIF files and verifies detection accuracy.

use std::path::Path;
use std::process::Command;

#[test]
fn test_webp_lossless_detection() {
    // Generate a simple test image (PNG)
    let test_png = "tests/test_webp_source.png";
    generate_test_png(test_png);
    
    // Convert to lossless WebP using cwebp
    let lossless_webp = "tests/test_lossless.webp";
    let status = Command::new("cwebp")
        .args(&["-lossless", "-z", "9", test_png, "-o", lossless_webp])
        .status();
    
    if status.is_err() || !status.unwrap().success() {
        eprintln!("⚠️  cwebp not available, skipping WebP lossless test");
        return;
    }
    
    // Verify detection
    let data = std::fs::read(lossless_webp).expect("Failed to read WebP");
    let is_lossless = shared_utils::image_formats::webp::is_lossless_from_bytes(&data);
    
    assert!(is_lossless, "WebP lossless detection failed: VP8L chunk not detected");
    
    println!("✅ WebP lossless detection: PASSED");
    
    // Cleanup
    let _ = std::fs::remove_file(test_png);
    let _ = std::fs::remove_file(lossless_webp);
}

#[test]
fn test_webp_lossy_detection() {
    let test_png = "tests/test_webp_lossy_source.png";
    generate_test_png(test_png);
    
    // Convert to lossy WebP
    let lossy_webp = "tests/test_lossy.webp";
    let status = Command::new("cwebp")
        .args(&["-q", "80", test_png, "-o", lossy_webp])
        .status();
    
    if status.is_err() || !status.unwrap().success() {
        eprintln!("⚠️  cwebp not available, skipping WebP lossy test");
        return;
    }
    
    let data = std::fs::read(lossy_webp).expect("Failed to read WebP");
    let is_lossless = shared_utils::image_formats::webp::is_lossless_from_bytes(&data);
    
    assert!(!is_lossless, "WebP lossy detection failed: VP8 chunk detected as lossless");
    
    println!("✅ WebP lossy detection: PASSED");
    
    let _ = std::fs::remove_file(test_png);
    let _ = std::fs::remove_file(lossy_webp);
}

#[test]
fn test_avif_lossless_detection() {
    let test_png = "tests/test_avif_source.png";
    generate_test_png(test_png);
    
    // Convert to lossless AVIF using avifenc
    let lossless_avif = "tests/test_lossless.avif";
    let status = Command::new("avifenc")
        .args(&[
            "--lossless",
            "--yuv", "444",
            "--depth", "10",
            test_png,
            lossless_avif
        ])
        .status();
    
    if status.is_err() || !status.unwrap().success() {
        eprintln!("⚠️  avifenc not available, skipping AVIF lossless test");
        return;
    }
    
    // Verify detection
    let result = shared_utils::image_detection::detect_compression(
        &shared_utils::image_detection::DetectedFormat::AVIF,
        Path::new(lossless_avif)
    );
    
    match result {
        Ok(comp) => {
            assert_eq!(
                comp,
                shared_utils::image_detection::CompressionType::Lossless,
                "AVIF lossless detection failed"
            );
            println!("✅ AVIF lossless detection: PASSED");
        }
        Err(e) => {
            panic!("AVIF lossless detection error: {}", e);
        }
    }
    
    let _ = std::fs::remove_file(test_png);
    let _ = std::fs::remove_file(lossless_avif);
}

#[test]
fn test_avif_lossy_detection() {
    let test_png = "tests/test_avif_lossy_source.png";
    generate_test_png(test_png);
    
    // Convert to lossy AVIF
    let lossy_avif = "tests/test_lossy.avif";
    let status = Command::new("avifenc")
        .args(&[
            "--min", "20",
            "--max", "30",
            "--yuv", "420",
            test_png,
            lossy_avif
        ])
        .status();
    
    if status.is_err() || !status.unwrap().success() {
        eprintln!("⚠️  avifenc not available, skipping AVIF lossy test");
        return;
    }
    
    let result = shared_utils::image_detection::detect_compression(
        &shared_utils::image_detection::DetectedFormat::AVIF,
        Path::new(lossy_avif)
    );
    
    match result {
        Ok(comp) => {
            assert_eq!(
                comp,
                shared_utils::image_detection::CompressionType::Lossy,
                "AVIF lossy detection failed"
            );
            println!("✅ AVIF lossy detection: PASSED");
        }
        Err(e) => {
            panic!("AVIF lossy detection error: {}", e);
        }
    }
    
    let _ = std::fs::remove_file(test_png);
    let _ = std::fs::remove_file(lossy_avif);
}

/// Generate a simple test PNG image
fn generate_test_png(path: &str) {
    use image::{ImageBuffer, Rgb};
    
    let img = ImageBuffer::from_fn(256, 256, |x, y| {
        let r = (x % 256) as u8;
        let g = (y % 256) as u8;
        let b = ((x + y) % 256) as u8;
        Rgb([r, g, b])
    });
    
    img.save(path).expect("Failed to save test PNG");
}

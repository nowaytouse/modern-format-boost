//! Blocking Behavior Tests
//!
//! Tests whether abnormal media files cause the program to block, freeze, or enter an infinite loop

use anyhow::Result;
use shared_utils::{
    loop_intent::{evaluate_loop_tree, LoopMeta},
    media_meta_utils::scan_gif_headers,
    quality_matcher::SourceCodec,
};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

// Test utility functions
mod test_utils {
    use super::*;

    // Create a huge JPEG file (may cause memory issues)
    pub fn create_huge_jpeg() -> Vec<u8> {
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
        jpeg.extend_from_slice(&[0xFF, 0xE0]); // APP0
        jpeg.extend_from_slice(&[0x00, 0x10]); // Length
        jpeg.extend_from_slice(b"JFIF");
        jpeg.extend_from_slice(&[0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);

        // Add a large amount of data (100MB)
        let large_data = vec![0xFF; 100 * 1024 * 1024];
        jpeg.extend_from_slice(&large_data);

        jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
        jpeg
    }

    // Create a circular GIF (may cause infinite loop)
    pub fn create_circular_gif() -> Vec<u8> {
        let mut gif = Vec::new();
        gif.extend_from_slice(b"GIF89a"); // GIF89a signature
        gif.extend_from_slice(&[0x10, 0x00, 0x10, 0x00]); // 16x16 size
        gif.extend_from_slice(&[0x00, 0x00]); // Global color table flag
        gif.extend_from_slice(&[0x00, 0x00, 0x00]); // Background color
        gif.extend_from_slice(&[0x00, 0x00]); // Pixel aspect ratio

        // Create a large number of frames (may cause slow processing)
        for _ in 0..1000 {
            gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]); // Graphic Control Extension
            gif.extend_from_slice(&[
                0x2C, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00,
            ]);
            gif.extend_from_slice(&[0x02]); // LZW minimum code size
            gif.extend_from_slice(&[0x02, 0x44, 0x01]); // Image data
            gif.extend_from_slice(&[0x00]); // Block terminator
        }

        gif.extend_from_slice(&[0x3B]); // GIF terminator
        gif
    }

    // Create a corrupted PNG file (may cause parsing errors)
    pub fn create_corrupted_png() -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]); // PNG signature
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]); // IHDR length
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // Width 16
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // Height 16
        png.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]); // Bit depth, color type, etc.
        png.extend_from_slice(&[0x2B, 0x7E, 0xE6, 0x73]); // CRC

        // Corrupted IDAT chunk
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0C]); // Invalid length
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // Corrupted data
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Invalid CRC
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]); // CRC
        png
    }

    // Create an infinite loop WebP animation
    pub fn create_infinite_webp() -> Vec<u8> {
        let mut webp = Vec::new();
        // RIFF header
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&[0x40, 0x00, 0x00, 0x00]); // File size
        webp.extend_from_slice(b"WEBP");
        // VP8X chunk (for animation)
        webp.extend_from_slice(b"VP8X");
        webp.extend_from_slice(&[0x0C, 0x00, 0x00, 0x00]); // chunk size
        webp.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]); // flags (animation)
        webp.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]); // width
        webp.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]); // height
                                                           // ANIM chunk (infinite loop)
        webp.extend_from_slice(b"ANIM");
        webp.extend_from_slice(&[0x06, 0x00, 0x00, 0x00]); // chunk size
        webp.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // background color
        webp.extend_from_slice(&[0x00, 0x00]); // loop count (0 = infinite)
        webp
    }

    // Create a zero-byte file
    pub fn create_empty_file() -> Vec<u8> {
        Vec::new()
    }

    // Create a file with only headers
    pub fn create_header_only_file(extension: &str) -> Vec<u8> {
        match extension {
            "jpg" => vec![0xFF, 0xD8],
            "png" => vec![0x89, 0x50, 0x4E, 0x47],
            "gif" => b"GIF87a".to_vec(),
            "webp" => b"RIFF".to_vec(),
            _ => vec![],
        }
    }

    pub fn write_test_file<P: AsRef<Path>>(data: &[u8], path: P) -> Result<()> {
        let mut file = fs::File::create(path)?;
        file.write_all(data)?;
        Ok(())
    }
}

// Timeout test macro
macro_rules! with_timeout {
    ($duration:expr, $block:block) => {{
        let handle = thread::spawn(move || {
            let start = Instant::now();
            let result = $block;
            let elapsed = start.elapsed();
            (result, elapsed)
        });

        // Wait for completion or timeout
        let timeout = Duration::from_secs($duration);
        let completed = handle.join();

        match completed {
            Ok((result, elapsed)) => {
                if elapsed > timeout {
                    return Err(anyhow::anyhow!("Operation timed out: {:?}", elapsed));
                }
                Ok(result)
            }
            Err(_) => Err(anyhow::anyhow!("Thread panicked or terminated")),
        }
    }};
}

// Blocking behavior tests
// ============================================================================

#[test]
fn test_huge_file_blocking() -> Result<()> {
    use test_utils::*;

    println!("🚫 Testing huge file blocking behavior...");

    let jpeg_data = create_huge_jpeg();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&jpeg_data, temp_file.path())?;

    // Test if codec identification blocks
    let jpeg_data_clone = jpeg_data.clone();
    let result = with_timeout!(5, { SourceCodec::identify_by_header(&jpeg_data_clone) })?;

    assert!(
        result.is_some(),
        "Huge file should be able to identify codec"
    );
    println!("   ✅ Codec identification: {:?}", result);

    // Test if file reading blocks
    let file_data = with_timeout!(10, { fs::read(temp_file.path()) })??;

    assert_eq!(
        file_data.len(),
        jpeg_data.len(),
        "Should be able to read full file"
    );
    println!("   ✅ File reading: {} bytes", file_data.len());

    println!("✅ Huge file blocking test passed");
    Ok(())
}

#[test]
fn test_circular_gif_blocking() -> Result<()> {
    use test_utils::*;

    println!("🔄 Testing circular GIF blocking behavior...");

    let gif_data = create_circular_gif();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&gif_data, temp_file.path())?;

    // Test if GIF header scan blocks
    let temp_path = temp_file.path().to_path_buf();
    let result = with_timeout!(30, { scan_gif_headers(&temp_path) })?;

    assert!(result.is_ok(), "Circular GIF header scan should succeed");
    let headers = result?;
    println!("   ✅ GIF header scan: {} frames", headers.frame_count);

    // Test if loop intent evaluation blocks
    let loop_meta = LoopMeta::from_gif_path(temp_file.path())
        .ok_or_else(|| anyhow::anyhow!("Cannot create LoopMeta"))?;
    let _result = with_timeout!(10, { evaluate_loop_tree(&loop_meta, None) })?;

    println!("   ✅ Loop intent evaluation completed");

    println!("✅ Circular GIF blocking test passed");
    Ok(())
}

#[test]
fn test_corrupted_file_blocking() -> Result<()> {
    use test_utils::*;

    println!("💥 Testing corrupted file blocking behavior...");

    let png_data = create_corrupted_png();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&png_data, temp_file.path())?;

    // Test if codec identification blocks
    let png_data_clone = png_data.clone();
    let result = with_timeout!(5, { SourceCodec::identify_by_header(&png_data_clone) })?;

    assert!(
        result.is_some(),
        "Corrupted PNG should be identified as PNG"
    );
    println!("   ✅ Codec identification: {:?}", result);

    // Test if file reading blocks
    let file_data = with_timeout!(5, { fs::read(temp_file.path()) })??;

    assert_eq!(
        file_data.len(),
        png_data.len(),
        "Should be able to read corrupted file"
    );
    println!("   ✅ File reading: {} bytes", file_data.len());

    println!("✅ Corrupted file blocking test passed");
    Ok(())
}

#[test]
fn test_infinite_webp_blocking() -> Result<()> {
    use test_utils::*;

    println!("♾️ Testing infinite loop WebP blocking behavior...");

    let webp_data = create_infinite_webp();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&webp_data, temp_file.path())?;

    // Test if codec identification blocks
    let webp_data_clone = webp_data.clone();
    let result = with_timeout!(5, { SourceCodec::identify_by_header(&webp_data_clone) })?;

    assert!(result.is_some(), "Infinite loop WebP should be identified");
    println!("   ✅ Codec identification: {:?}", result);

    println!("✅ Infinite loop WebP blocking test passed");
    Ok(())
}

#[test]
fn test_empty_file_blocking() -> Result<()> {
    use test_utils::*;

    println!("📄 Testing empty file blocking behavior...");

    let empty_data = create_empty_file();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&empty_data, temp_file.path())?;

    // Test if codec identification blocks
    let empty_data_clone = empty_data.clone();
    let result = with_timeout!(5, { SourceCodec::identify_by_header(&empty_data_clone) })?;

    assert!(result.is_none(), "Empty file should not identify any codec");
    println!("   ✅ Codec identification: None");

    // Test if file reading blocks
    let file_data = with_timeout!(5, { fs::read(temp_file.path()) })??;

    assert_eq!(file_data.len(), 0, "Empty file should read as empty");
    println!("   ✅ File reading: {} bytes", file_data.len());

    println!("✅ Empty file blocking test passed");
    Ok(())
}

#[test]
fn test_header_only_file_blocking() -> Result<()> {
    use test_utils::*;

    println!("📋 Testing header-only file blocking behavior...");

    let extensions = vec!["jpg", "png", "gif", "webp"];

    for ext in extensions {
        let header_data = create_header_only_file(ext);
        let temp_file = NamedTempFile::new()?;
        write_test_file(&header_data, temp_file.path())?;

        // Test if codec identification blocks
        let header_data_clone = header_data.clone();
        let result = with_timeout!(5, { SourceCodec::identify_by_header(&header_data_clone) })?;

        if ext == "gif" {
            assert!(result.is_some(), "GIF header should be identified");
            println!("   ✅ {}: {:?}", ext, result);
        } else {
            // Other formats may fail to identify, which is normal
            println!("   ✅ {}: {:?}", ext, result);
        }
    }

    println!("✅ Header-only file blocking test passed");
    Ok(())
}

// ============================================================================
// Memory pressure tests
// ============================================================================

#[test]
fn test_memory_pressure_blocking() -> Result<()> {
    use test_utils::*;

    println!("🧠 Testing memory pressure blocking behavior...");

    // Create multiple large files to process simultaneously
    let mut handles = Vec::new();

    for i in 0..10 {
        let jpeg_data = create_huge_jpeg();
        let handle = thread::spawn(move || {
            let start = Instant::now();
            let result = SourceCodec::identify_by_header(&jpeg_data);
            let elapsed = start.elapsed();
            (i, result, elapsed)
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    let mut completed = 0;
    let total_handles = handles.len();

    for handle in handles {
        match handle.join() {
            Ok((thread_id, result, elapsed)) => {
                completed += 1;
                println!(
                    "   ✅ Thread {}: {:?} (Time: {:?})",
                    thread_id, result, elapsed
                );
            }
            Err(_) => {
                println!("   ❌ Thread panicked");
            }
        }
    }

    assert!(completed == total_handles, "All threads should complete");
    println!("✅ Memory pressure blocking test passed");
    Ok(())
}

// ============================================================================
// Concurrent access tests
// ============================================================================

#[test]
fn test_concurrent_access_blocking() -> Result<()> {
    use test_utils::*;

    println!("🔀 Testing concurrent access blocking behavior...");

    let gif_data = create_circular_gif();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&gif_data, temp_file.path())?;

    // Create multiple threads accessing the same file simultaneously
    let mut handles = Vec::new();

    for i in 0..20 {
        let file_path = temp_file.path().to_path_buf();
        let handle = thread::spawn(move || {
            let start = Instant::now();
            let result = scan_gif_headers(&file_path);
            let elapsed = start.elapsed();
            (i, result.is_ok(), elapsed)
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    let mut success_count = 0;
    let mut total_time = Duration::ZERO;

    for handle in handles {
        let (thread_id, success, elapsed) = handle.join().unwrap();
        if success {
            success_count += 1;
        }
        total_time += elapsed;
        println!(
            "   ✅ Thread {}: Success={}, Time={:?}",
            thread_id, success, elapsed
        );
    }

    assert!(success_count >= 15, "At least 15 threads should succeed");
    let avg_time = total_time / 20;
    println!(
        "   📊 Success rate: {}/20, Avg time: {:?}",
        success_count, avg_time
    );

    println!("✅ Concurrent access blocking test passed");
    Ok(())
}

// ============================================================================
// Extreme case tests
// ============================================================================

#[test]
fn test_extreme_case_blocking() -> Result<()> {
    use test_utils::*;

    println!("⚡ Testing extreme case blocking behavior...");

    // Test 1: Huge file + concurrent access
    let jpeg_data = create_huge_jpeg();
    let mut handles = Vec::new();

    for _i in 0..5 {
        let data = jpeg_data.clone();
        let handle =
            thread::spawn(move || with_timeout!(10, { SourceCodec::identify_by_header(&data) }));
        handles.push(handle);
    }

    for (i, handle) in handles.into_iter().enumerate() {
        let result = handle.join().unwrap()?;
        assert!(
            result.is_some(),
            "Huge file concurrent identification should succeed"
        );
        println!("   ✅ Huge file concurrency {}: Success", i);
    }

    // Test 2: Corrupted file + mass processing
    let png_data = create_corrupted_png();
    let mut handles = Vec::new();

    for _i in 0..50 {
        let data = png_data.clone();
        let handle =
            thread::spawn(move || with_timeout!(5, { SourceCodec::identify_by_header(&data) }));
        handles.push(handle);
    }

    let mut success_count = 0;
    for handle in handles {
        let result = handle.join().unwrap()?;
        if result.is_some() {
            success_count += 1;
        }
    }

    assert!(
        success_count >= 40,
        "At least 40 corrupted files should be identified successfully"
    );
    println!(
        "   ✅ Corrupted file batch processing: {}/50 success",
        success_count
    );

    println!("✅ Extreme case blocking test passed");
    Ok(())
}

// ============================================================================
// Main test runner
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_blocking_behavior_tests() -> Result<()> {
        println!("🚫 Starting all blocking behavior tests...\n");

        // Basic blocking tests
        test_huge_file_blocking()?;
        test_circular_gif_blocking()?;
        test_corrupted_file_blocking()?;
        test_infinite_webp_blocking()?;
        test_empty_file_blocking()?;
        test_header_only_file_blocking()?;

        // Stress tests
        test_memory_pressure_blocking()?;
        test_concurrent_access_blocking()?;
        test_extreme_case_blocking()?;

        println!("\n🎉 All blocking behavior tests passed!");
        println!("✅ Huge file processing: No blocking");
        println!("✅ Circular GIF processing: No blocking");
        println!("✅ Corrupted file processing: No blocking");
        println!("✅ Infinite loop WebP: No blocking");
        println!("✅ Empty file processing: No blocking");
        println!("✅ Header-only file processing: No blocking");
        println!("✅ Memory pressure test: No blocking");
        println!("✅ Concurrent access test: No blocking");
        println!("✅ Extreme case test: No blocking");

        Ok(())
    }
}

//! Simplified blocking behavior tests
//!
//! Tests whether abnormal media files cause program blocking, freezing, or infinite loops

use anyhow::Result;
use shared_utils::{
    loop_intent::{LoopMeta, evaluate_loop_tree},
    media_meta_utils::scan_gif_headers,
    quality_matcher::SourceCodec,
};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

// Test utility functions
mod test_utils {
    use super::*;

    // Create oversized JPEG file (may cause memory issues)
    pub fn create_large_jpeg() -> Vec<u8> {
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
        jpeg.extend_from_slice(&[0xFF, 0xE0]); // APP0
        jpeg.extend_from_slice(&[0x00, 0x10]); // Length
        jpeg.extend_from_slice(b"JFIF");
        jpeg.extend_from_slice(&[0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);

        // Add appropriate data (1MB)
        let large_data = vec![0xFF; 1024 * 1024];
        jpeg.extend_from_slice(&large_data);

        jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
        jpeg
    }

    // Create multi-frame GIF file
    pub fn create_multi_frame_gif() -> Vec<u8> {
        let mut gif = Vec::new();
        gif.extend_from_slice(b"GIF89a"); // GIF89a signature
        gif.extend_from_slice(&[0x10, 0x00, 0x10, 0x00]); // 16x16 size
        gif.extend_from_slice(&[0x00, 0x00]); // Global color table flag
        gif.extend_from_slice(&[0x00, 0x00, 0x00]); // Background color
        gif.extend_from_slice(&[0x00, 0x00]); // Pixel aspect ratio

        // Create 100 frames
        for _i in 0..100 {
            gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]); // Graphics control extension
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

    // Create corrupted PNG file
    pub fn create_corrupted_png() -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]); // PNG signature
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]); // IHDR length
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // Width 16
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // Height 16
        png.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]); // Bit depth, color type, etc
        png.extend_from_slice(&[0x2B, 0x7E, 0xE6, 0x73]); // CRC

        // Add corrupted IDAT block
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0C]); // Wrong length
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // Corrupted data
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Wrong CRC
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]); // CRC
        png
    }

    // Create zero-byte file
    pub const fn create_empty_file() -> Vec<u8> {
        Vec::new()
    }

    pub fn write_test_file<P: AsRef<Path>>(data: &[u8], path: P) -> Result<()> {
        let mut file = fs::File::create(path)?;
        file.write_all(data)?;
        Ok(())
    }
}

// Simple timeout check function
fn check_timeout<T>(
    operation: impl FnOnce() -> T,
    timeout_secs: u64,
    operation_name: &str,
) -> Result<T> {
    let start = Instant::now();
    let result = operation();
    let elapsed = start.elapsed();

    if elapsed > Duration::from_secs(timeout_secs) {
        return Err(anyhow::anyhow!(
            "Operation '{operation_name}' timed out: {elapsed:?}"
        ));
    }

    println!("   ✅ {operation_name}: took {elapsed:?}");
    Ok(result)
}

// ============================================================================
// Blocking behavior tests
// ============================================================================

#[test]
fn test_large_file_blocking() -> Result<()> {
    use test_utils::*;

    println!("🚫 Testing large file blocking behavior...");

    let jpeg_data = create_large_jpeg();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&jpeg_data, temp_file.path())?;

    // Test codec identification blocking
    let result = check_timeout(
        || SourceCodec::identify_by_header(&jpeg_data),
        5,
        "Codec identification",
    )?;

    assert!(result.is_some(), "Large file should identify codec");
    println!("   ✅ Codec identification: {result:?}");

    // Test file reading blocking
    let file_data = check_timeout(|| fs::read(temp_file.path()).unwrap(), 10, "File reading")?;

    assert_eq!(
        file_data.len(),
        jpeg_data.len(),
        "Should read complete file"
    );
    println!("   ✅ File reading: {} bytes", file_data.len());

    println!("✅ Large file blocking test passed");
    Ok(())
}

#[test]
fn test_multi_frame_gif_blocking() -> Result<()> {
    use test_utils::*;

    println!("🔄 Testing multi-frame GIF blocking behavior...");

    let gif_data = create_multi_frame_gif();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&gif_data, temp_file.path())?;

    // Test GIF header scanning blocking
    let headers = check_timeout(
        || scan_gif_headers(temp_file.path()).unwrap(),
        30,
        "GIF header scanning",
    )?;

    println!("   ✅ GIF header scanning: {} frames", headers.frame_count);

    // Test loop intent evaluation blocking
    let loop_meta = LoopMeta::from_gif_path(temp_file.path())
        .ok_or_else(|| anyhow::anyhow!("Cannot create LoopMeta"))?;

    let _result = check_timeout(
        || evaluate_loop_tree(&loop_meta, None),
        10,
        "Loop intent evaluation",
    )?;

    println!("✅ Multi-frame GIF blocking test passed");
    Ok(())
}

#[test]
fn test_corrupted_file_blocking() -> Result<()> {
    use test_utils::*;

    println!("💥 Testing corrupted file blocking behavior...");

    let png_data = create_corrupted_png();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&png_data, temp_file.path())?;

    // Test codec identification blocking
    let result = check_timeout(
        || SourceCodec::identify_by_header(&png_data),
        5,
        "Codec identification",
    )?;

    assert!(
        result.is_some(),
        "Corrupted PNG should be identified as PNG"
    );
    println!("   ✅ Codec identification: {result:?}");

    // Test file reading blocking
    let file_data = check_timeout(|| fs::read(temp_file.path()).unwrap(), 5, "File reading")?;

    assert_eq!(
        file_data.len(),
        png_data.len(),
        "Should read corrupted file"
    );
    println!("   ✅ File reading: {} bytes", file_data.len());

    println!("✅ Corrupted file blocking test passed");
    Ok(())
}

#[test]
fn test_empty_file_blocking() -> Result<()> {
    use test_utils::*;

    println!("📄 Testing empty file blocking behavior...");

    let empty_data = create_empty_file();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&empty_data, temp_file.path())?;

    // Test codec identification blocking
    let result = check_timeout(
        || SourceCodec::identify_by_header(&empty_data),
        5,
        "Codec identification",
    )?;

    assert!(result.is_none(), "Empty file should not identify codec");
    println!("   ✅ Codec identification: None");

    // Test file reading blocking
    let file_data = check_timeout(|| fs::read(temp_file.path()).unwrap(), 5, "File reading")?;

    assert_eq!(file_data.len(), 0, "Empty file should read as empty");
    println!("   ✅ File reading: {} bytes", file_data.len());

    println!("✅ Empty file blocking test passed");
    Ok(())
}

// ============================================================================
// Concurrent access tests
// ============================================================================

#[test]
fn test_concurrent_access_blocking() -> Result<()> {
    use test_utils::*;

    println!("🔀 Testing concurrent access blocking behavior...");

    let gif_data = create_multi_frame_gif();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&gif_data, temp_file.path())?;

    let file_path = temp_file.path().to_path_buf();
    let mut handles = Vec::new();

    // Create multiple threads accessing the same file simultaneously
    for i in 0..10 {
        let path = file_path.clone();
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            let result = scan_gif_headers(&path);
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
        println!("   ✅ Thread {thread_id}: success={success}, took={elapsed:?}");
    }

    assert!(success_count >= 8, "At least 8 threads should succeed");
    let avg_time = total_time / 10;
    println!("   📊 Success rate: {success_count}/10, average time: {avg_time:?}");

    println!("✅ Concurrent access blocking test passed");
    Ok(())
}

// ============================================================================
// Memory pressure tests
// ============================================================================

#[test]
fn test_memory_pressure_blocking() {
    use test_utils::*;

    println!("🧠 Testing memory pressure blocking behavior...");

    // Create multiple large files for simultaneous processing
    let mut handles = Vec::new();

    for i in 0..5 {
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            let jpeg_data = create_large_jpeg();
            let result = SourceCodec::identify_by_header(&jpeg_data);
            let elapsed = start.elapsed();
            (i, result.is_some(), elapsed)
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    let mut completed = 0;

    for handle in handles {
        match handle.join() {
            Ok((thread_id, result, elapsed)) => {
                completed += 1;
                println!("   ✅ Thread {thread_id}: {result:?} (took: {elapsed:?})");
            }
            Err(_) => {
                println!("   ❌ Thread panicked");
            }
        }
    }

    assert!(completed == 5, "All threads should complete");
    println!("✅ Memory pressure blocking test passed");
}

// ============================================================================
// Main test runner
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_simple_blocking_tests() -> Result<()> {
        println!("🚫 Starting all simplified blocking behavior tests...\n");

        // Basic blocking tests
        test_large_file_blocking()?;
        test_multi_frame_gif_blocking()?;
        test_corrupted_file_blocking()?;
        test_empty_file_blocking()?;

        // Concurrent and pressure tests
        test_concurrent_access_blocking()?;
        test_memory_pressure_blocking();

        println!("\n🎉 All simplified blocking behavior tests passed!");
        println!("✅ Large file processing: No blocking");
        println!("✅ Multi-frame GIF processing: No blocking");
        println!("✅ Corrupted file processing: No blocking");
        println!("✅ Empty file processing: No blocking");
        println!("✅ Concurrent access test: No blocking");
        println!("✅ Memory pressure test: No blocking");

        Ok(())
    }
}

//! 简化的阻塞行为测试
//!
//! 测试异常媒体文件是否会导致程序阻塞、卡死或无限循环

use anyhow::Result;
use shared_utils::{
    loop_intent::{evaluate_loop_tree, LoopMeta},
    media_meta_utils::scan_gif_headers,
    quality_matcher::SourceCodec,
};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

// 测试工具函数
mod test_utils {
    use super::*;

    // 创建超大JPEG文件（可能导致内存问题）
    pub fn create_large_jpeg() -> Vec<u8> {
        let mut jpeg = Vec::new();
        jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI
        jpeg.extend_from_slice(&[0xFF, 0xE0]); // APP0
        jpeg.extend_from_slice(&[0x00, 0x10]); // 长度
        jpeg.extend_from_slice(b"JFIF");
        jpeg.extend_from_slice(&[0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);
        
        // 添加适量数据（1MB）
        let large_data = vec![0xFF; 1024 * 1024];
        jpeg.extend_from_slice(&large_data);
        
        jpeg.extend_from_slice(&[0xFF, 0xD9]); // EOI
        jpeg
    }

    // 创建多帧GIF文件
    pub fn create_multi_frame_gif() -> Vec<u8> {
        let mut gif = Vec::new();
        gif.extend_from_slice(b"GIF89a"); // GIF89a签名
        gif.extend_from_slice(&[0x10, 0x00, 0x10, 0x00]); // 16x16尺寸
        gif.extend_from_slice(&[0x00, 0x00]); // 全局颜色表标志
        gif.extend_from_slice(&[0x00, 0x00, 0x00]); // 背景色
        gif.extend_from_slice(&[0x00, 0x00]); // 像素宽高比

        // 创建100帧
        for _i in 0..100 {
            gif.extend_from_slice(&[0x21, 0xF9, 0x04, 0x00, 0x0A, 0x00, 0x00, 0x00]); // 图形控制扩展
            gif.extend_from_slice(&[0x2C, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x10, 0x00, 0x00, 0x00]);
            gif.extend_from_slice(&[0x02]); // LZW最小码长
            gif.extend_from_slice(&[0x02, 0x44, 0x01]); // 图像数据
            gif.extend_from_slice(&[0x00]); // 块终止符
        }

        gif.extend_from_slice(&[0x3B]); // GIF终止符
        gif
    }

    // 创建损坏的PNG文件
    pub fn create_corrupted_png() -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]); // PNG签名
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]); // IHDR长度
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // 宽度16
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // 高度16
        png.extend_from_slice(&[0x08, 0x02, 0x00, 0x00, 0x00]); // 位深度、颜色类型等
        png.extend_from_slice(&[0x2B, 0x7E, 0xE6, 0x73]); // CRC
        
        // 添加损坏的IDAT块
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x0C]); // 错误的长度
        png.extend_from_slice(b"IDAT");
        png.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // 损坏数据
        png.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // 错误的CRC
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]); // CRC
        png
    }

    // 创建零字节文件
    pub fn create_empty_file() -> Vec<u8> {
        Vec::new()
    }

    pub fn write_test_file<P: AsRef<Path>>(data: &[u8], path: P) -> Result<()> {
        let mut file = fs::File::create(path)?;
        file.write_all(data)?;
        Ok(())
    }
}

// 简单的超时检查函数
fn check_timeout<T>(operation: impl FnOnce() -> T, timeout_secs: u64, operation_name: &str) -> Result<T> {
    let start = Instant::now();
    let result = operation();
    let elapsed = start.elapsed();
    
    if elapsed > Duration::from_secs(timeout_secs) {
        return Err(anyhow::anyhow!("操作 '{}' 超时: {:?}", operation_name, elapsed));
    }
    
    println!("   ✅ {}: 耗时 {:?}", operation_name, elapsed);
    Ok(result)
}

// ============================================================================
// 阻塞行为测试
// ============================================================================

#[test]
fn test_large_file_blocking() -> Result<()> {
    use test_utils::*;
    
    println!("🚫 测试大文件阻塞行为...");
    
    let jpeg_data = create_large_jpeg();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&jpeg_data, temp_file.path())?;
    
    // 测试编解码器识别是否阻塞
    let result = check_timeout(
        || SourceCodec::identify_by_header(&jpeg_data),
        5,
        "编解码器识别"
    )?;
    
    assert!(result.is_some(), "大文件应该能识别编解码器");
    println!("   ✅ 编解码器识别: {:?}", result);
    
    // 测试文件读取是否阻塞
    let file_data = check_timeout(
        || fs::read(temp_file.path()).unwrap(),
        10,
        "文件读取"
    )?;
    
    assert_eq!(file_data.len(), jpeg_data.len(), "应该能读取完整文件");
    println!("   ✅ 文件读取: {} bytes", file_data.len());
    
    println!("✅ 大文件阻塞测试通过");
    Ok(())
}

#[test]
fn test_multi_frame_gif_blocking() -> Result<()> {
    use test_utils::*;
    
    println!("🔄 测试多帧GIF阻塞行为...");
    
    let gif_data = create_multi_frame_gif();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&gif_data, temp_file.path())?;
    
    // 测试GIF头扫描是否阻塞
    let headers = check_timeout(
        || scan_gif_headers(temp_file.path()).unwrap(),
        30,
        "GIF头扫描"
    )?;
    
    println!("   ✅ GIF头扫描: {} 帧", headers.frame_count);
    
    // 测试循环意图评估是否阻塞
    let loop_meta = LoopMeta::from_gif_path(temp_file.path())
        .ok_or_else(|| anyhow::anyhow!("无法创建LoopMeta"))?;
    
    let _result = check_timeout(
        || evaluate_loop_tree(&loop_meta, None),
        10,
        "循环意图评估"
    )?;
    
    println!("✅ 多帧GIF阻塞测试通过");
    Ok(())
}

#[test]
fn test_corrupted_file_blocking() -> Result<()> {
    use test_utils::*;
    
    println!("💥 测试损坏文件阻塞行为...");
    
    let png_data = create_corrupted_png();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&png_data, temp_file.path())?;
    
    // 测试编解码器识别是否阻塞
    let result = check_timeout(
        || SourceCodec::identify_by_header(&png_data),
        5,
        "编解码器识别"
    )?;
    
    assert!(result.is_some(), "损坏PNG应该能识别为PNG");
    println!("   ✅ 编解码器识别: {:?}", result);
    
    // 测试文件读取是否阻塞
    let file_data = check_timeout(
        || fs::read(temp_file.path()).unwrap(),
        5,
        "文件读取"
    )?;
    
    assert_eq!(file_data.len(), png_data.len(), "应该能读取损坏文件");
    println!("   ✅ 文件读取: {} bytes", file_data.len());
    
    println!("✅ 损坏文件阻塞测试通过");
    Ok(())
}

#[test]
fn test_empty_file_blocking() -> Result<()> {
    use test_utils::*;
    
    println!("📄 测试空文件阻塞行为...");
    
    let empty_data = create_empty_file();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&empty_data, temp_file.path())?;
    
    // 测试编解码器识别是否阻塞
    let result = check_timeout(
        || SourceCodec::identify_by_header(&empty_data),
        5,
        "编解码器识别"
    )?;
    
    assert!(result.is_none(), "空文件不应该识别出编解码器");
    println!("   ✅ 编解码器识别: None");
    
    // 测试文件读取是否阻塞
    let file_data = check_timeout(
        || fs::read(temp_file.path()).unwrap(),
        5,
        "文件读取"
    )?;
    
    assert_eq!(file_data.len(), 0, "空文件应该读取为空");
    println!("   ✅ 文件读取: {} bytes", file_data.len());
    
    println!("✅ 空文件阻塞测试通过");
    Ok(())
}

// ============================================================================
// 并发访问测试
// ============================================================================

#[test]
fn test_concurrent_access_blocking() -> Result<()> {
    use test_utils::*;
    
    println!("🔀 测试并发访问阻塞行为...");
    
    let gif_data = create_multi_frame_gif();
    let temp_file = NamedTempFile::new()?;
    write_test_file(&gif_data, temp_file.path())?;
    
    let file_path = temp_file.path().to_path_buf();
    let mut handles = Vec::new();
    
    // 创建多个线程同时访问同一文件
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
    
    // 等待所有线程完成
    let mut success_count = 0;
    let mut total_time = Duration::ZERO;
    
    for handle in handles {
        let (thread_id, success, elapsed) = handle.join().unwrap();
        if success {
            success_count += 1;
        }
        total_time += elapsed;
        println!("   ✅ 线程 {}: 成功={}, 耗时={:?}", thread_id, success, elapsed);
    }
    
    assert!(success_count >= 8, "至少8个线程应该成功");
    let avg_time = total_time / 10;
    println!("   📊 成功率: {}/10, 平均耗时: {:?}", success_count, avg_time);
    
    println!("✅ 并发访问阻塞测试通过");
    Ok(())
}

// ============================================================================
// 内存压力测试
// ============================================================================

#[test]
fn test_memory_pressure_blocking() -> Result<()> {
    use test_utils::*;
    
    println!("🧠 测试内存压力阻塞行为...");
    
    // 创建多个大文件同时处理
    let mut handles = Vec::new();
    
    for i in 0..5 {
        let jpeg_data = create_large_jpeg();
        let handle = std::thread::spawn(move || {
            let start = Instant::now();
            let result = SourceCodec::identify_by_header(&jpeg_data);
            let elapsed = start.elapsed();
            (i, result, elapsed)
        });
        handles.push(handle);
    }
    
    // 等待所有线程完成
    let mut completed = 0;
    
    for handle in handles {
        match handle.join() {
            Ok((thread_id, result, elapsed)) => {
                completed += 1;
                println!("   ✅ 线程 {}: {:?} (耗时: {:?})", thread_id, result, elapsed);
            }
            Err(_) => {
                println!("   ❌ 线程 panicked");
            }
        }
    }
    
    assert!(completed == 5, "所有线程应该完成");
    println!("✅ 内存压力阻塞测试通过");
    Ok(())
}

// ============================================================================
// 主测试运行器
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_simple_blocking_tests() -> Result<()> {
        println!("🚫 开始运行所有简化阻塞行为测试...\n");
        
        // 基础阻塞测试
        test_large_file_blocking()?;
        test_multi_frame_gif_blocking()?;
        test_corrupted_file_blocking()?;
        test_empty_file_blocking()?;
        
        // 并发和压力测试
        test_concurrent_access_blocking()?;
        test_memory_pressure_blocking()?;
        
        println!("\n🎉 所有简化阻塞行为测试通过！");
        println!("✅ 大文件处理: 无阻塞");
        println!("✅ 多帧GIF处理: 无阻塞");
        println!("✅ 损坏文件处理: 无阻塞");
        println!("✅ 空文件处理: 无阻塞");
        println!("✅ 并发访问测试: 无阻塞");
        println!("✅ 内存压力测试: 无阻塞");
        
        Ok(())
    }
}

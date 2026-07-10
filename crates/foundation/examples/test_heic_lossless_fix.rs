// 测试HEIC无损检测修复
// 编译: cargo build --release -p foundation
// 运行: cargo run --release --example test_heic_lossless_fix

use std::path::Path;

fn main() {
    let test_file = Path::new("/Users/nyamiiko/Downloads/Final 4/拍照/IMG_0041.HEIC");
    
    if !test_file.exists() {
        eprintln!("❌ Test file not found: {}", test_file.display());
        std::process::exit(1);
    }
    
    println!("🧪 Testing HEIC lossless detection fix");
    println!("📁 File: {}", test_file.display());
    println!();
    
    let data = match std::fs::read(test_file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("❌ Failed to read file: {}", e);
            std::process::exit(1);
        }
    };
    
    println!("📊 File size: {} bytes", data.len());
    
    // 调用修复后的检测函数
    match foundation::image::image_heic_analysis::detect_heic_is_lossless(&data, test_file) {
        Ok(is_lossless) => {
            println!("✅ Detection completed successfully!");
            println!("🎯 Result: {}", if is_lossless { "LOSSLESS" } else { "LOSSY" });
            println!();
            println!("💡 This means the fix is working - no more errors for transquant_bypass_enabled_flag=1");
        }
        Err(e) => {
            eprintln!("❌ Detection failed: {}", e);
            eprintln!();
            eprintln!("⚠️  This means the fix needs adjustment");
            std::process::exit(1);
        }
    }
}

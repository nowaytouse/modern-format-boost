use std::process::Command;
use std::fs;
use std::path::Path;

#[test]
fn test_cjxl_grayscale_icc_fallback() {
    // 1. Create a large enough grayscale PNG (to avoid the 500KB "small PNG" skip)
    let input = Path::new("test_grayscale_icc.png");
    let output = Path::new("test_grayscale_icc.jxl");
    
    // Cleanup
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
    
    // Create 1024x1024 grayscale PNG with noise to ensure it's > 500KB
    let status = Command::new("magick")
        .args(&["-size", "1024x1024", "xc:gray", "+noise", "random", "test_grayscale_icc.png"])
        .status()
        .expect("magick failed to run");
    assert!(status.success());
    
    // 2. Assign an sRGB profile (RGB color space) to this grayscale image
    // This often triggers the libpng warning when cjxl reads it.
    let _ = Command::new("magick")
        .args(&["test_grayscale_icc.png", "-colorspace", "sRGB", "test_grayscale_icc.png"])
        .status();
    
    // 3. Try to run the tool's conversion logic
    // Since we're in an integration test, we can call the binary or the library function.
    // Let's call the library function convert_to_jxl.
    
    use img::lossless_converter::{convert_to_jxl, ConvertOptions};
    let options = ConvertOptions {
        verbose: true,
        ..Default::default()
    };
    
    let result = convert_to_jxl(input, &options, 0.1, None);
    
    match result {
        Ok(_) => {
            println!("✅ Conversion succeeded (presumably via fallback)!");
            assert!(output.exists());
            
            // Verify it's a valid JXL
            let status = Command::new("jxlinfo")
                .arg(output)
                .status()
                .expect("jxlinfo failed to run");
            assert!(status.success());
        }
        Err(e) => {
            // If it failed, check if it's because cjxl is NOT installed
            // But we checked it earlier.
            panic!("❌ Conversion failed despite fallback logic: {:?}", e);
        }
    }
    
    // Cleanup
    let _ = fs::remove_file(input);
    let _ = fs::remove_file(output);
}

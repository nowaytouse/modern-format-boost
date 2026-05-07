use shared_utils::image_jpeg_analysis::extract_gainmap_from_jpeg;
use std::fs;
use std::path::Path;

fn main() {
    println!("Running test...");
    test_real_hdr_file_extraction_final();
    println!("✅ Test completed!");
}

fn test_real_hdr_file_extraction_final() {
    let path = Path::new("/Users/nyamiiko/Downloads/GitHub/modern_format_boost/debug/IMG_0413.JPG");
    if !path.exists() {
        return;
    }
    let data = fs::read(path).unwrap_or_else(|e| panic!("Failed to read image: {e:?}"));
    let result = extract_gainmap_from_jpeg(&data);
    match result {
        Ok((base, gain)) => {
            println!("✅ REAL HDR FILE EXTRACTION SUCCESSFUL!");
            println!(
                "   Base: {}x{}, Gain: {}x{}",
                base.width(),
                base.height(),
                gain.width(),
                gain.height()
            );
        }
        Err(e) => {
            panic!("❌ FAILED ON REAL FILE: {e}");
        }
    }
}

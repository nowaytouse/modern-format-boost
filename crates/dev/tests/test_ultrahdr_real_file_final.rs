use std::fs;
use std::path::Path;
use shared_utils::image_jpeg_analysis::extract_gainmap_from_jpeg;

#[test]
fn test_real_hdr_file_extraction_final() {
    let path = Path::new("/Users/nyamiiko/Downloads/GitHub/modern_format_boost/debug/IMG_0413.JPG");
    if !path.exists() { return; }
    let data = fs::read(path).expect("Failed to read image");
    let result = extract_gainmap_from_jpeg(&data);
    match result {
        Ok((base, gain)) => {
            println!("✅ REAL HDR FILE EXTRACTION SUCCESSFUL!");
            println!("   Base: {}x{}, Gain: {}x{}", base.width(), base.height(), gain.width(), gain.height());
        }
        Err(e) => {
            panic!("❌ FAILED ON REAL FILE: {}", e);
        }
    }
}

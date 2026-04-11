use shared_utils::image_jpeg_analysis::extract_xmp_from_jpeg_data;
use std::fs;

fn main() {
    let data = fs::read("debug/Ultra_HDR_Samples-main/Ultra_HDR_Samples-main/Originals/Ultra_HDR_Samples_Originals_01.jpg").unwrap();
    if let Some(xmp) = extract_xmp_from_jpeg_data(&data) {
        println!("XMP found (len: {}): \n{:?}", xmp.len(), xmp);
    } else {
        println!("No XMP extracted!");
    }
}

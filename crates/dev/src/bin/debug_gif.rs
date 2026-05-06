use shared_utils::quality_matcher::SourceCodec;

fn main() {
    let truncated_gif = b"GIF87";
    println!("Testing: {truncated_gif:?}");
    println!("Length: {}", truncated_gif.len());

    for (i, &byte) in truncated_gif.iter().enumerate() {
        println!("Byte {}: 0x{:02X} ('{}')", i, byte, byte as char);
    }

    let codec = SourceCodec::identify_by_header(truncated_gif);
    println!("Identified as: {codec:?}");

    // Test against MPEG patterns
    println!("Starts with 0x47: {}", truncated_gif.starts_with(&[0x47]));
    println!(
        "Starts with [0x00, 0x00, 0x01, 0xBA]: {}",
        truncated_gif.starts_with(&[0x00, 0x00, 0x01, 0xBA])
    );
}

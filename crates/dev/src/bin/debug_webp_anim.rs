use std::fs;
fn main() {
    let data = fs::read("test.webp").unwrap();
    let has_anim = data.windows(4).any(|w| w == b"ANIM");
    println!("Has ANIM: {has_anim}");
}

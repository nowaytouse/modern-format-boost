fn main() {
    let path = std::env::var("PATH").unwrap_or_default();
    println!("PATH: {}", path);
}

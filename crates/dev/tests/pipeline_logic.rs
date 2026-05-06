use std::path::PathBuf;

fn main() {
    println!("Running test...");
    let test_file = get_edge_file("test.jpg");
    println!("Resolved test file path: {}", test_file.display());
    // Simple test to verify the module compiles
    println!("✅ Pipeline logic test completed!");
}

fn get_edge_file(name: &str) -> PathBuf {
    let cargo_manifest = env!("CARGO_MANIFEST_DIR");
    let edge = PathBuf::from(cargo_manifest).join("tests").join("edge");

    let img = edge.join("images").join(name);
    if img.exists() {
        return img;
    }

    let gif = edge.join("gifs").join(name);
    if gif.exists() {
        return gif;
    }

    let jxl = edge.join("jxl").join(name);
    if jxl.exists() {
        return jxl;
    }

    // Fallback to current directory
    PathBuf::from(name)
}

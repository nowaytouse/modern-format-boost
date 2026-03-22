use std::fs;
use blake3;

fn normalize(content: &str) -> String {
    content.replace("\r\n", "\n").trim().to_string()
}

fn main() {
    let readme = fs::read_to_string("README.md").unwrap();
    let changelog = fs::read_to_string("CHANGELOG.md").unwrap();
    
    let norm_readme = normalize(&readme);
    let norm_changelog = normalize(&changelog);
    
    let readme_hash = blake3::hash(norm_readme.as_bytes());
    let changelog_hash = blake3::hash(norm_changelog.as_bytes());
    
    let readme_lines = norm_readme.lines().count();
    let changelog_lines = norm_changelog.lines().count();
    
    println!("--- NORMALIZED README ---");
    println!("BLAKE3: {}", readme_hash.to_hex());
    println!("Lines: {}", readme_lines);
    println!("--- NORMALIZED CHANGELOG ---");
    println!("BLAKE3: {}", changelog_hash.to_hex());
    println!("Lines: {}", changelog_lines);
}

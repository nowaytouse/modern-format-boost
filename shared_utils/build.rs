use std::fs;
use blake3;

fn main() {
    // Expected values for v0.10.91-nightly
    const EXPECTED_README_HASH: &str = "04ac36b1be447f68e2c09cd0a2eb9f8514f319595bde34b397708c5c99ec974f";
    const EXPECTED_README_LINES: usize = 488;
    
    const EXPECTED_CHANGELOG_HASH: &str = "ea3f1aaa55b0d37fe0424df5fed7a00d1c43b414caeda22fb0429ddb3517f3b5";
    const EXPECTED_CHANGELOG_LINES: usize = 3730;

    // Check README.md
    let readme = fs::read("../README.md").expect("FATAL: README.md not found in workspace root");
    let readme_hash = blake3::hash(&readme);
    let readme_content = String::from_utf8_lossy(&readme);
    let readme_lines = if readme_content.is_empty() { 0 } else { readme_content.lines().count() };
    
    if readme_hash.to_hex().as_str() != EXPECTED_README_HASH {
        panic!(
            "\n💥 INTEGRITY FAILURE (README.md Hash Mismatch)\nGot:      {}\nExpected: {}\n",
            readme_hash.to_hex(), EXPECTED_README_HASH
        );
    }
    
    if readme_lines != EXPECTED_README_LINES {
        panic!(
            "\n💥 INTEGRITY FAILURE (README.md Line count Mismatch)\nGot:      {}\nExpected: {}\n",
            readme_lines, EXPECTED_README_LINES
        );
    }

    // Check CHANGELOG.md
    let changelog = fs::read("../CHANGELOG.md").expect("FATAL: CHANGELOG.md not found in workspace root");
    let changelog_hash = blake3::hash(&changelog);
    let changelog_content = String::from_utf8_lossy(&changelog);
    let changelog_lines = if changelog_content.is_empty() { 0 } else { changelog_content.lines().count() };
    
    if changelog_hash.to_hex().as_str() != EXPECTED_CHANGELOG_HASH {
        panic!(
            "\n💥 INTEGRITY FAILURE (CHANGELOG.md Hash Mismatch)\nGot:      {}\nExpected: {}\n",
            changelog_hash.to_hex(), EXPECTED_CHANGELOG_HASH
        );
    }
    
    if changelog_lines != EXPECTED_CHANGELOG_LINES {
        panic!(
            "\n💥 INTEGRITY FAILURE (CHANGELOG.md Line count Mismatch)\nGot:      {}\nExpected: {}\n",
            changelog_lines, EXPECTED_CHANGELOG_LINES
        );
    }

    // Tell Cargo to re-run this script if these files change
    println!("cargo:rerun-if-changed=../README.md");
    println!("cargo:rerun-if-changed=../CHANGELOG.md");
}

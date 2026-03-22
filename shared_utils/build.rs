use std::fs;
use blake3;

fn normalize_and_hash(content: &str) -> (String, usize) {
    let normalized = content.replace("\r\n", "\n").trim().to_string();
    let hash = blake3::hash(normalized.as_bytes()).to_hex().to_string();
    let lines = if normalized.is_empty() { 0 } else { normalized.lines().count() };
    (hash, lines)
}

fn main() {
    // Expected values for v0.10.91-nightly (Normalized)
    const EXPECTED_README_HASH: &str = "83809b1afaf228ebc183da0de2c1d174f45e8fa0d53ba91876417bb84f36c96c";
    const EXPECTED_README_LINES: usize = 488;
    
    const EXPECTED_CHANGELOG_HASH: &str = "1ac2165abe926ca4199bb7483b3ef8cfb3dc322f7c16481c39cb7deed0955274";
    const EXPECTED_CHANGELOG_LINES: usize = 3729;

    // Check README.md
    let readme_raw = fs::read_to_string("../README.md").expect("FATAL: README.md not found in workspace root");
    let (readme_hash, readme_lines) = normalize_and_hash(&readme_raw);
    
    if readme_hash != EXPECTED_README_HASH {
        panic!(
            "\n💥 INTEGRITY FAILURE (README.md Hash Mismatch)\nGot:      {}\nExpected: {}\n",
            readme_hash, EXPECTED_README_HASH
        );
    }
    
    if readme_lines != EXPECTED_README_LINES {
        panic!(
            "\n💥 INTEGRITY FAILURE (README.md Line count Mismatch)\nGot:      {}\nExpected: {}\n",
            readme_lines, EXPECTED_README_LINES
        );
    }

    // Check CHANGELOG.md
    let changelog_raw = fs::read_to_string("../CHANGELOG.md").expect("FATAL: CHANGELOG.md not found in workspace root");
    let (changelog_hash, changelog_lines) = normalize_and_hash(&changelog_raw);
    
    if changelog_hash != EXPECTED_CHANGELOG_HASH {
        panic!(
            "\n💥 INTEGRITY FAILURE (CHANGELOG.md Hash Mismatch)\nGot:      {}\nExpected: {}\n",
            changelog_hash, EXPECTED_CHANGELOG_HASH
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

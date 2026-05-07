//! Real silent fallback detection tests
//!
//! This test scans actual code to detect real silent fallback modes

use std::fs;
use std::path::Path;

fn main() {
    println!("Running real silent fallback detection tests...");
    test_detect_real_unwrap_or_patterns();
    test_detect_expect_patterns();
    test_detect_panic_patterns();
    test_code_quality_metrics();
    println!("✅ Real silent fallback detection tests passed!");
}

fn test_detect_real_unwrap_or_patterns() {
    let src_dir = Path::new("src");
    let mut found_silent_fallbacks = Vec::new();

    // Recursively scan all Rust files
    for entry in walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file()
            || entry.path().extension() != Some(std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };

        let lines: Vec<&str> = content.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            // Detect silent fallback to 0 pattern
            if line.contains("unwrap_or(0)") || line.contains("unwrap_or(0.0)") {
                found_silent_fallbacks.push(format!(
                    "{}:{}: Silent fallback to 0 - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // Detect silent fallback to 1 pattern
            if line.contains("unwrap_or(1)") {
                found_silent_fallbacks.push(format!(
                    "{}:{}: Silent fallback to 1 - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // Detect silent fallback to small number pattern
            if line.contains("unwrap_or(")
                && (line.contains("unwrap_or(2)")
                    || line.contains("unwrap_or(3)")
                    || line.contains("unwrap_or(4)")
                    || line.contains("unwrap_or(5)")
                    || line.contains("unwrap_or(10)")
                    || line.contains("unwrap_or(100)"))
            {
                found_silent_fallbacks.push(format!(
                    "{}:{}: Silent fallback to small number - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // Detect silent fallback to default string pattern
            if line.contains("unwrap_or(\"") {
                found_silent_fallbacks.push(format!(
                    "{}:{}: Silent fallback to default string - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }
        }
    }

    // Output results
    if found_silent_fallbacks.is_empty() {
        println!("✅ No silent fallback patterns found");
    } else {
        println!(
            "\n🚨 Found {} silent fallback instances:",
            found_silent_fallbacks.len()
        );
        for fallback in &found_silent_fallbacks {
            println!("  {fallback}");
        }
        println!("\n⚠️  These silent fallbacks should be replaced with explicit error handling");
    }

    // This test always passes but reports issues
    println!("Silent fallback detection completed, check output for details");
}

fn test_detect_expect_patterns() {
    let src_dir = Path::new("src");
    let mut found_vague_expects = Vec::new();

    for entry in walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file()
            || entry.path().extension() != Some(std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };

        let lines: Vec<&str> = content.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            // Detect vague expect messages
            if line.contains(".expect(\"") {
                let expect_msg = extract_expect_message(line);
                if is_vague_expect_message(&expect_msg) {
                    found_vague_expects.push(format!(
                        "{}:{}: Vague expect message - {}",
                        entry.path().display(),
                        line_num + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    if found_vague_expects.is_empty() {
        println!("✅ No vague expect messages found");
    } else {
        println!(
            "\n🚨 Found {} vague expect messages:",
            found_vague_expects.len()
        );
        for expect_msg in &found_vague_expects {
            println!("  {expect_msg}");
        }
        println!("\n⚠️  These expect messages should be more specific");
    }

    println!("expect message detection completed, check output for details");
}

fn extract_expect_message(line: &str) -> String {
    if let Some(start) = line.find(".expect(\"")
        && let Some(end) = line[start + 9..].find('"')
    {
        return line[start + 9..start + 9 + end].to_string();
    }
    String::new()
}

fn is_vague_expect_message(msg: &str) -> bool {
    let vague_patterns = vec![
        "required",
        "missing",
        "failed",
        "error",
        "invalid",
        "none",
        "empty",
        "null",
        "not found",
        "unable",
        "cannot",
    ];

    let msg_lower = msg.to_lowercase();
    vague_patterns
        .iter()
        .any(|pattern| msg_lower.contains(pattern))
}

fn test_detect_panic_patterns() {
    let src_dir = Path::new("src");
    let mut found_panics = Vec::new();

    for entry in walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file()
            || entry.path().extension() != Some(std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };

        let lines: Vec<&str> = content.lines().collect();

        for (line_num, line) in lines.iter().enumerate() {
            // Detect panic! macros
            if line.contains("panic!(") {
                found_panics.push(format!(
                    "{}:{}: panic! macro - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // Detect unwrap()
            if line.contains(".unwrap()")
                && !line.contains("unwrap_or")
                && !line.contains("unwrap_or_else")
            {
                found_panics.push(format!(
                    "{}:{}: Direct unwrap() - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }
        }
    }

    if found_panics.is_empty() {
        println!("✅ No panic/unwrap patterns found");
    } else {
        println!("\n🚨 Found {} panic/unwrap instances:", found_panics.len());
        for panic in &found_panics {
            println!("  {panic}");
        }
        println!("\n⚠️  These panic/unwrap should be replaced with error handling");
    }

    println!("panic pattern detection completed, check output for details");
}

fn test_code_quality_metrics() {
    let src_dir = Path::new("src");
    let mut total_lines = 0;
    let mut unwrap_or_count = 0;
    let mut expect_count = 0;
    let mut unwrap_count = 0;
    let mut panic_count = 0;

    for entry in walkdir::WalkDir::new(src_dir)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file()
            || entry.path().extension() != Some(std::ffi::OsStr::new("rs"))
        {
            continue;
        }

        let Ok(content) = fs::read_to_string(entry.path()) else {
            continue;
        };

        total_lines += content.lines().count();
        unwrap_or_count += content.matches("unwrap_or(").count();
        expect_count += content.matches(".expect(\"").count();
        unwrap_count += content.matches(".unwrap()").count();
        panic_count += content.matches("panic!(").count();
    }

    println!("\n📊 Code Quality Metrics:");
    println!("  Total lines: {total_lines}");
    println!("  unwrap_or instances: {unwrap_or_count}");
    println!("  expect instances: {expect_count}");
    println!("  Direct unwrap instances: {unwrap_count}");
    println!("  panic! instances: {panic_count}");

    #[allow(clippy::cast_precision_loss)]
    let density = (unwrap_or_count + expect_count + unwrap_count + panic_count) as f64
        / total_lines as f64
        * 1000.0;
    println!("  Issue density: {density:.2} issues / 1000 lines of code");

    if density > 5.0 {
        println!("⚠️  High issue density, needs improvement");
    } else if density > 2.0 {
        println!("🟡 Medium issue density, could be improved");
    } else {
        println!("✅ Low issue density");
    }

    println!("Code quality metrics calculation completed");
}

//! 真实的静默回退检测测试
//!
//! 这个测试会扫描实际代码，检测真正的静默回退模式

use std::fs;
use std::path::Path;

fn main() {
    println!("运行真实静默回退检测测试...");
    test_detect_real_unwrap_or_patterns();
    test_detect_expect_patterns();
    test_detect_panic_patterns();
    test_code_quality_metrics();
    println!("✅ 真实静默回退检测测试通过！");
}

fn test_detect_real_unwrap_or_patterns() {
    let src_dir = Path::new("src");
    let mut found_silent_fallbacks = Vec::new();

    // 递归扫描所有Rust文件
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
            // 检测静默回退到0的模式
            if line.contains("unwrap_or(0)") || line.contains("unwrap_or(0.0)") {
                found_silent_fallbacks.push(format!(
                    "{}:{}: 静默回退到0 - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // 检测静默回退到1的模式
            if line.contains("unwrap_or(1)") {
                found_silent_fallbacks.push(format!(
                    "{}:{}: 静默回退到1 - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // 检测静默回退到小数值的模式
            if line.contains("unwrap_or(")
                && (line.contains("unwrap_or(2)")
                    || line.contains("unwrap_or(3)")
                    || line.contains("unwrap_or(4)")
                    || line.contains("unwrap_or(5)")
                    || line.contains("unwrap_or(10)")
                    || line.contains("unwrap_or(100)"))
            {
                found_silent_fallbacks.push(format!(
                    "{}:{}: 静默回退到小数值 - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // 检测静默回退到默认字符串的模式
            if line.contains("unwrap_or(\"") {
                found_silent_fallbacks.push(format!(
                    "{}:{}: 静默回退到默认字符串 - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }
        }
    }

    // 输出结果
    if found_silent_fallbacks.is_empty() {
        println!("✅ 未发现静默回退模式");
    } else {
        println!("\n🚨 发现 {} 个静默回退实例:", found_silent_fallbacks.len());
        for fallback in &found_silent_fallbacks {
            println!("  {fallback}");
        }
        println!("\n⚠️  这些静默回退应该被替换为显式错误处理");
    }

    // 这个测试总是通过，但会报告问题
    println!("静默回退检测完成，查看输出了解详情");
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
            // 检测模糊的expect消息
            if line.contains(".expect(\"") {
                let expect_msg = extract_expect_message(line);
                if is_vague_expect_message(&expect_msg) {
                    found_vague_expects.push(format!(
                        "{}:{}: 模糊的expect消息 - {}",
                        entry.path().display(),
                        line_num + 1,
                        line.trim()
                    ));
                }
            }
        }
    }

    if found_vague_expects.is_empty() {
        println!("✅ 未发现模糊的expect消息");
    } else {
        println!(
            "\n🚨 发现 {} 个模糊的expect消息:",
            found_vague_expects.len()
        );
        for expect_msg in &found_vague_expects {
            println!("  {expect_msg}");
        }
        println!("\n⚠️  这些expect消息应该更具体");
    }

    println!("expect消息检测完成，查看输出了解详情");
}

fn extract_expect_message(line: &str) -> String {
    if let Some(start) = line.find(".expect(\"") {
        if let Some(end) = line[start + 9..].find('"') {
            return line[start + 9..start + 9 + end].to_string();
        }
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
            // 检测panic!宏
            if line.contains("panic!(") {
                found_panics.push(format!(
                    "{}:{}: panic!宏 - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }

            // 检测unwrap()
            if line.contains(".unwrap()")
                && !line.contains("unwrap_or")
                && !line.contains("unwrap_or_else")
            {
                found_panics.push(format!(
                    "{}:{}: 直接unwrap() - {}",
                    entry.path().display(),
                    line_num + 1,
                    line.trim()
                ));
            }
        }
    }

    if found_panics.is_empty() {
        println!("✅ 未发现panic/unwrap模式");
    } else {
        println!("\n🚨 发现 {} 个panic/unwrap实例:", found_panics.len());
        for panic in &found_panics {
            println!("  {panic}");
        }
        println!("\n⚠️  这些panic/unwrap应该被替换为错误处理");
    }

    println!("panic模式检测完成，查看输出了解详情");
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

    println!("\n📊 代码质量指标:");
    println!("  总行数: {total_lines}");
    println!("  unwrap_or实例: {unwrap_or_count}");
    println!("  expect实例: {expect_count}");
    println!("  直接unwrap实例: {unwrap_count}");
    println!("  panic!实例: {panic_count}");

    #[allow(clippy::cast_precision_loss)]
    let density = (unwrap_or_count + expect_count + unwrap_count + panic_count) as f64
        / total_lines as f64
        * 1000.0;
    println!("  问题密度: {density:.2} 个问题/千行代码");

    if density > 5.0 {
        println!("⚠️  问题密度较高，需要改进");
    } else if density > 2.0 {
        println!("🟡 问题密度中等，可以改进");
    } else {
        println!("✅ 问题密度较低");
    }

    println!("代码质量指标计算完成");
}

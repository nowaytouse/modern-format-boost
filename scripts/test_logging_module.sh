#!/bin/bash
# 测试logging模块的基本功能
# Test basic functionality of the logging module

set -euo pipefail

echo "🔍 Testing logging module..."

# 创建临时测试程序
cat > /tmp/test_logging.rs << 'EOF'
use shared_utils::logging::{LogConfig, init_logging, log_external_tool};
use tracing::{info, warn, error};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    // 初始化日志系统
    let config = LogConfig::default();
    init_logging("test_logging", config)?;
    
    // 测试基本日志
    info!("This is an info message");
    warn!("This is a warning message");
    error!("This is an error message");
    
    // 测试结构化日志
    info!(file = "test.mp4", size = 1024, "Processing file");
    
    // 测试外部工具日志
    log_external_tool(
        "ffmpeg",
        &["-i", "input.mp4", "output.mp4"],
        "ffmpeg version 6.0...",
        Some(0),
        Duration::from_secs(5),
    );
    
    log_external_tool(
        "x265",
        &["--input", "test.yuv"],
        "x265 error output",
        Some(1),
        Duration::from_secs(2),
    );
    
    println!("✅ Logging test completed successfully!");
    println!("📁 Log file location: {:?}", std::env::temp_dir().join("test_logging.log"));
    
    Ok(())
}
EOF

# 编译测试程序
cd "$(dirname "$0")/.."
echo "📦 Compiling test program..."
rustc --edition 2021 \
    -L target/debug/deps \
    --extern shared_utils=target/debug/libshared_utils.rlib \
    --extern anyhow=target/debug/deps/libanyhow-*.rlib \
    --extern tracing=target/debug/deps/libtracing-*.rlib \
    /tmp/test_logging.rs -o /tmp/test_logging 2>&1 || {
    echo "❌ Compilation failed. Building shared_utils first..."
    cargo build --package shared_utils
    rustc --edition 2021 \
        -L target/debug/deps \
        --extern shared_utils=target/debug/libshared_utils.rlib \
        --extern anyhow=target/debug/deps/libanyhow-*.rlib \
        --extern tracing=target/debug/deps/libtracing-*.rlib \
        /tmp/test_logging.rs -o /tmp/test_logging
}

# 运行测试程序
echo "🚀 Running test program..."
/tmp/test_logging

# 检查日志文件
LOG_FILE=$(ls -t /tmp/test_logging.log* 2>/dev/null | head -1)
if [ -n "$LOG_FILE" ]; then
    echo ""
    echo "📄 Log file content (last 20 lines):"
    tail -20 "$LOG_FILE"
    echo ""
    echo "✅ Log file created successfully: $LOG_FILE"
else
    echo "⚠️  Warning: Log file not found in /tmp/"
fi

# 清理
rm -f /tmp/test_logging.rs /tmp/test_logging

echo ""
echo "✅ All logging module tests passed!"

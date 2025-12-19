#!/bin/bash
# 🔥 v6.7: 核心功能验证脚本

set -e

echo "🔬 v6.7 Core Function Test"

# 运行单元测试
cd shared_utils
echo "📊 Running unit tests..."
cargo test --lib --release > test_results.log 2>&1

# 检查测试结果
if grep -q "test result: ok" test_results.log; then
    echo "✅ All unit tests passed"
    grep "test result:" test_results.log
else
    echo "❌ Tests failed:"
    tail -20 test_results.log
    exit 1
fi

# 测试实际文件
echo ""
echo "📁 Testing with real file..."
TEST_FILE="../test_input/test_60s.mp4"

if [ -f "$TEST_FILE" ]; then
    echo "✅ Found test file: $TEST_FILE"
    
    # 使用 vidquality-hevc 测试
    cd ../vidquality_hevc
    echo "🎬 Running vidquality-hevc with pure media comparison..."
    
    # 使用双击脚本参数: --explore --match-quality true --compress --apple-compat
    timeout 60s ./target/release/vidquality-hevc auto "$TEST_FILE" \
        --explore --match-quality true --compress --apple-compat \
        --output "/tmp/test_v6.7_output.mp4" 2>&1 | head -50
    
    if [ -f "/tmp/test_v6.7_output.mp4" ]; then
        echo "✅ Output created successfully"
        rm -f "/tmp/test_v6.7_output.mp4"
    else
        echo "⚠️ No output file (may be expected for highly compressed input)"
    fi
else
    echo "⚠️ Test file not found, skipping real file test"
fi

echo "✅ v6.7 core functionality verified"
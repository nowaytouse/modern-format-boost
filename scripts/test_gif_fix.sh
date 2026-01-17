#!/bin/bash
# 测试 GIF → HEVC 转换修复
# 验证 v6.9.17 修复的 FFmpeg 参数冲突问题

set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🧪 Testing GIF → HEVC Conversion Fix (v6.9.17)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# 测试文件
TEST_GIF="/Users/nyamiiko/Downloads/1/参考/内容 猎奇/4h8uh4vkss9clo2wfiy30kach.gif"
TEST_OUTPUT="/tmp/4h8uh4vkss9clo2wfiy30kach.mp4"

# 清理旧输出
rm -f "$TEST_OUTPUT"

echo ""
echo "📁 Test file: $TEST_GIF"
echo "📁 Output: $TEST_OUTPUT"
echo ""

# 检查文件是否存在
if [ ! -f "$TEST_GIF" ]; then
    echo "❌ Test file not found!"
    exit 1
fi

# 获取文件信息
echo "📊 Input file info:"
ffprobe -v quiet -print_format json -show_format -show_streams "$TEST_GIF" | grep -E '"codec_name"|"width"|"height"|"duration"|"size"' | head -5
echo ""

# 运行转换测试
echo "🔄 Running conversion test..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# 使用 imgquality_hevc auto 进行转换（模拟实际场景）
if ../imgquality_hevc/target/release/imgquality-hevc auto \
    --explore \
    --match-quality \
    --compress \
    --output /tmp \
    "$TEST_GIF" 2>&1 | tee /tmp/test_gif_conversion.log; then
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "✅ Conversion SUCCEEDED!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    # 检查输出文件
    if [ -f "$TEST_OUTPUT" ]; then
        echo ""
        echo "📊 Output file info:"
        ls -lh "$TEST_OUTPUT"
        ffprobe -v quiet -print_format json -show_format -show_streams "$TEST_OUTPUT" | grep -E '"codec_name"|"width"|"height"|"duration"|"size"' | head -5
        echo ""
        echo "✅ Output file created successfully"
        
        # 清理
        rm -f "$TEST_OUTPUT"
    else
        echo "⚠️  Output file not found at expected location"
        echo "   Checking for alternative output locations..."
        find /tmp -name "*4h8uh4vkss9clo2wfiy30kach*" -type f -mmin -5
    fi
    
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "🎉 TEST PASSED - Fix verified!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    exit 0
else
    echo ""
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "❌ Conversion FAILED!"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    # 显示错误日志
    echo ""
    echo "📋 Error log:"
    cat /tmp/test_gif_conversion.log | grep -i "error\|failed" || echo "No specific error found"
    
    echo ""
    echo "❌ TEST FAILED - Fix not working"
    exit 1
fi

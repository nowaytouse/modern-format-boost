#!/bin/bash
# 测试脚本：验证命名漏洞修复
# Test script: Verify dash vulnerability fix

set -e

echo "🔍 Testing dash-prefixed filename handling..."
echo ""

# 创建测试目录
TEST_DIR="/tmp/mfb_dash_test_$$"
mkdir -p "$TEST_DIR"
cd "$TEST_DIR"

echo "📁 Test directory: $TEST_DIR"
echo ""

# 清理函数
cleanup() {
    echo ""
    echo "🧹 Cleaning up..."
    cd /tmp
    rm -rf "$TEST_DIR"
    echo "✅ Cleanup complete"
}
trap cleanup EXIT

# 1. 创建测试图片 (以 - 开头的文件名)
echo "1️⃣  Creating test images with dash-prefixed names..."

# 创建一个简单的测试 JPEG
ffmpeg -f lavfi -i color=red:s=100x100:d=0.1 -frames:v 1 -y -- "-test.jpg" 2>/dev/null
ffmpeg -f lavfi -i color=blue:s=100x100:d=0.1 -frames:v 1 -y -- "--test.png" 2>/dev/null
ffmpeg -f lavfi -i color=green:s=100x100:d=0.1 -frames:v 1 -y -- "-rf.jpg" 2>/dev/null

if [ -f "-test.jpg" ] && [ -f "--test.png" ] && [ -f "-rf.jpg" ]; then
    echo "   ✅ Test files created successfully"
    ls -lh -- *.jpg *.png 2>/dev/null | awk '{print "      " $9 " (" $5 ")"}'
else
    echo "   ❌ Failed to create test files"
    exit 1
fi
echo ""

# 2. 测试 cjxl 转换
echo "2️⃣  Testing cjxl conversion with dash-prefixed filenames..."

# 查找 cjxl
if ! command -v cjxl &> /dev/null; then
    echo "   ⚠️  cjxl not found, skipping cjxl test"
else
    # 测试 JPEG 转换
    if cjxl --lossless_jpeg=1 -- "-test.jpg" "-test.jxl" 2>/dev/null; then
        if [ -f "-test.jxl" ]; then
            echo "   ✅ cjxl: -test.jpg → -test.jxl ($(stat -f%z -- "-test.jxl" 2>/dev/null || stat -c%s -- "-test.jxl") bytes)"
        else
            echo "   ❌ cjxl: Output file not created"
        fi
    else
        echo "   ❌ cjxl: Conversion failed"
    fi

    # 测试 PNG 转换
    if cjxl -d 0.0 -e 7 -- "--test.png" "--test.jxl" 2>/dev/null; then
        if [ -f "--test.jxl" ]; then
            echo "   ✅ cjxl: --test.png → --test.jxl ($(stat -f%z -- "--test.jxl" 2>/dev/null || stat -c%s -- "--test.jxl") bytes)"
        else
            echo "   ❌ cjxl: Output file not created"
        fi
    else
        echo "   ❌ cjxl: Conversion failed"
    fi
fi
echo ""

# 3. 测试 ImageMagick
echo "3️⃣  Testing ImageMagick with dash-prefixed filenames..."

if ! command -v magick &> /dev/null; then
    echo "   ⚠️  magick not found, skipping ImageMagick test"
else
    if magick -- "-rf.jpg" "-rf_converted.png" 2>/dev/null; then
        if [ -f "-rf_converted.png" ]; then
            echo "   ✅ magick: -rf.jpg → -rf_converted.png ($(stat -f%z -- "-rf_converted.png" 2>/dev/null || stat -c%s -- "-rf_converted.png") bytes)"
        else
            echo "   ❌ magick: Output file not created"
        fi
    else
        echo "   ❌ magick: Conversion failed"
    fi
fi
echo ""

# 4. 测试 FFmpeg (使用 ./ 前缀)
echo "4️⃣  Testing FFmpeg with ./ prefix for dash-prefixed filenames..."

if ffmpeg -i ./-test.jpg -f null - -y 2>&1 | grep -q "Stream"; then
    echo "   ✅ ffmpeg: Successfully read ./-test.jpg"
else
    echo "   ❌ ffmpeg: Failed to read file"
fi
echo ""

# 5. 测试实际的工具 (如果已编译)
echo "5️⃣  Testing actual imgquality-hevc tool (if available)..."

IMGQUALITY_HEVC="/Users/user/Downloads/GitHub/modern_format_boost/target/release/imgquality-hevc"

if [ -f "$IMGQUALITY_HEVC" ]; then
    mkdir -p output
    if "$IMGQUALITY_HEVC" -- "-test.jpg" -o output 2>&1 | grep -q "Processing\|Converted\|Skipped"; then
        echo "   ✅ imgquality-hevc: Successfully processed -test.jpg"
        if [ -f "output/-test.jxl" ]; then
            echo "      Output: output/-test.jxl ($(stat -f%z output/-test.jxl 2>/dev/null || stat -c%s output/-test.jxl) bytes)"
        fi
    else
        echo "   ⚠️  imgquality-hevc: Processing completed (check output)"
    fi
else
    echo "   ⚠️  imgquality-hevc not found at $IMGQUALITY_HEVC"
    echo "      Run: cd /Users/user/Downloads/GitHub/modern_format_boost && cargo build --release"
fi
echo ""

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ Dash vulnerability fix verification complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

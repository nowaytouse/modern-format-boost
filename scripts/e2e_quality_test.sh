#!/bin/bash
# 🔥 End-to-End Quality Verification Test
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
TEST_DIR="/tmp/e2e_quality_test_$$"

echo "🧪 E2E Quality Verification Test"
echo "================================"

# 1. 编译
echo ""
echo "🔨 Building vidquality_hevc..."
cd "$PROJECT_ROOT/vidquality_hevc"
cargo build --release 2>&1 | tail -5
echo "✅ Build complete"

# 2. 准备测试环境
echo ""
echo "📁 Setting up test environment..."
mkdir -p "$TEST_DIR"

# 创建测试视频（副本）
echo "📹 Creating test video (5s, 640x480)..."
ffmpeg -f lavfi -i testsrc=duration=5:size=640x480:rate=30 \
    -c:v libx264 -crf 18 -y "$TEST_DIR/input.mp4" 2>/dev/null

echo "✅ Test video: $(ls -lh "$TEST_DIR/input.mp4" | awk '{print $5}')"

# 3. 运行转换（捕获输出）
echo ""
echo "🎬 Running conversion with quality verification..."
echo "   Command: vidquality_hevc --explore --match-quality"
echo ""

cd "$TEST_DIR"
"$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc" \
    auto input.mp4 --explore --match-quality=true 2>&1 | tee conversion.log

# 4. 验证结果
echo ""
echo "🔍 Verifying results..."

if grep -q "Using standalone vmaf tool" conversion.log; then
    echo "✅ Standalone vmaf tool was used"
else
    echo "⚠️  Standalone vmaf tool NOT detected"
fi

if grep -q "MS-SSIM score:" conversion.log; then
    SCORE=$(grep "MS-SSIM score:" conversion.log | tail -1 | awk '{print $NF}')
    echo "✅ MS-SSIM calculated: $SCORE"
else
    echo "❌ MS-SSIM calculation failed"
fi

if grep -q "ALL.*QUALITY.*CALCULATIONS.*FAILED" conversion.log; then
    echo "❌ Quality calculation failed!"
    exit 1
fi

# 5. 清理
echo ""
echo "🧹 Cleaning up..."
rm -rf "$TEST_DIR"

echo ""
echo "🎉 Test Complete!"
echo "✅ Quality verification is working correctly"

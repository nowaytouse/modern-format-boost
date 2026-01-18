#!/bin/bash
# 🔥 v6.9: MS-SSIM 目标阈值测试脚本
# 测试 MS-SSIM 作为目标阈值（不仅仅是验证）

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TEST_DIR="$PROJECT_DIR/test_media"
TOOL="$PROJECT_DIR/vidquality_hevc/target/release/vidquality-hevc"

echo "═══════════════════════════════════════════════════════════════"
echo "🔥 MS-SSIM Target Threshold Test (v6.9)"
echo "═══════════════════════════════════════════════════════════════"

# 检查工具
if [ ! -f "$TOOL" ]; then
    echo "❌ Tool not found: $TOOL"
    echo "   Run: cargo build --release"
    exit 1
fi

# 创建测试目录
mkdir -p "$TEST_DIR"

# 生成短测试视频 (3秒)
SHORT_VIDEO="$TEST_DIR/test_short_3s.mp4"
if [ ! -f "$SHORT_VIDEO" ]; then
    echo "📹 Generating short test video (3s)..."
    ffmpeg -y -f lavfi -i testsrc=duration=3:size=640x480:rate=30 \
           -c:v libx264 -preset fast -crf 18 \
           "$SHORT_VIDEO" 2>/dev/null
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Test 1: Short video with MS-SSIM target (auto-enabled)"
echo "═══════════════════════════════════════════════════════════════"
echo ""

# 测试短视频 - MS-SSIM 应该自动启用
"$TOOL" auto "$SHORT_VIDEO" --explore --match-quality true --force 2>&1 | tee /tmp/ms_ssim_test.log

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "📊 Test Result Analysis"
echo "═══════════════════════════════════════════════════════════════"

# 检查日志中是否有 MS-SSIM 相关输出
if grep -q "MS-SSIM" /tmp/ms_ssim_test.log; then
    echo "✅ MS-SSIM calculation was performed"
    grep "MS-SSIM" /tmp/ms_ssim_test.log
else
    echo "⚠️  No MS-SSIM output found in log"
fi

# 检查是否有目标阈值检查
if grep -q "MS-SSIM TARGET" /tmp/ms_ssim_test.log; then
    echo "✅ MS-SSIM target threshold check was performed"
else
    echo "ℹ️  MS-SSIM target threshold check not triggered (quality may be good)"
fi

echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "✅ MS-SSIM Target Test Complete"
echo "═══════════════════════════════════════════════════════════════"

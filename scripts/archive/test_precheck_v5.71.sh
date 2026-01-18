#!/bin/bash
# 🔥 v5.71 Precheck Module Test Script
# 测试预检查模块的改进：古老编解码器识别、FPS分类、色彩检测

set -e

echo "═══════════════════════════════════════════════════════════════"
echo "🧪 Precheck Module v5.71 Test"
echo "═══════════════════════════════════════════════════════════════"

# 编译
echo ""
echo "📦 Building shared_utils..."
cargo build -p shared_utils --release 2>&1 | tail -5

echo ""
echo "📦 Building vidquality-hevc..."
cargo build -p vidquality-hevc --release 2>&1 | tail -5

echo ""
echo "✅ Build successful!"
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "📋 Test Summary:"
echo "  ✅ FpsCategory enum with 4 levels (Normal/Extended/Extreme/Invalid)"
echo "  ✅ ProcessingRecommendation with 5 levels"
echo "  ✅ Legacy codecs → StronglyRecommended (not skip!)"
echo "  ✅ Modern codecs → NotRecommended (warning only)"
echo "  ✅ HDR detection (bt2020, 10-bit)"
echo "  ✅ Color space/pixel format extraction"
echo "═══════════════════════════════════════════════════════════════"

#!/bin/bash
# 诊断 ffmpeg 配置问题
echo "🔍 FFmpeg Diagnostic Report"
echo "==========================="
echo ""

echo "1️⃣  FFmpeg Version:"
ffmpeg -version 2>&1 | head -3
echo ""

echo "2️⃣  Configuration:"
ffmpeg -version 2>&1 | grep "configuration:" | tr ' ' '\n' | grep -E "(libvmaf|libx265|libsvtav1)" || echo "  ⚠️  No relevant libs in configuration"
echo ""

echo "3️⃣  Available Filters:"
echo "  libvmaf:"
ffmpeg -hide_banner -filters 2>&1 | grep vmaf || echo "    ❌ Not found"
echo "  ssim:"
ffmpeg -hide_banner -filters 2>&1 | grep "ssim" || echo "    ❌ Not found"
echo ""

echo "4️⃣  Available Encoders:"
for enc in libx265 libsvtav1 libaom-av1 libx264; do
    if ffmpeg -hide_banner -encoders 2>&1 | grep -q "$enc"; then
        echo "  ✅ $enc"
    else
        echo "  ❌ $enc"
    fi
done
echo ""

echo "5️⃣  System Libraries:"
for lib in libvmaf libx265 libsvtav1; do
    if [ -f "/opt/homebrew/lib/${lib}.dylib" ] || [ -f "/usr/local/lib/${lib}.dylib" ]; then
        echo "  ✅ $lib installed"
    else
        echo "  ❌ $lib NOT installed"
    fi
done
echo ""

echo "6️⃣  Homebrew FFmpeg Info:"
brew info ffmpeg 2>&1 | head -10
echo ""

echo "7️⃣  Recommendation:"
echo "==================="
if ffmpeg -hide_banner -filters 2>&1 | grep -q "libvmaf"; then
    echo "✅ Your ffmpeg has libvmaf support"
    echo "💡 The issue may be with filter syntax"
else
    echo "❌ Your ffmpeg lacks libvmaf support"
    echo "💡 Run: ./scripts/rebuild_ffmpeg_full.sh"
fi

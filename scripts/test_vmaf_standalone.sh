#!/bin/bash
# 🔥 Test Standalone VMAF Integration
set -e

echo "🧪 Testing standalone vmaf integration..."

# 检查 vmaf 工具
if ! command -v vmaf &>/dev/null; then
    echo "❌ vmaf tool not found"
    echo "💡 Install: brew install libvmaf"
    exit 1
fi

echo "✅ vmaf tool found: $(which vmaf)"

# 编译
cd "$(dirname "$0")/.."
echo "🔨 Building..."
cargo build --release 2>&1 | tail -5

echo "✅ Build complete"
echo "💡 Test with real video files to verify MS-SSIM calculation"

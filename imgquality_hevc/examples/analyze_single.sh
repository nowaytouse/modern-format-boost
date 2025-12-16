#!/opt/homebrew/bin/bash

# ============================================================================
# 📊 单个图像质量分析示例
# ============================================================================
#
# 使用 imgquality 工具分析单个图像文件，显示详细的质量参数
#
# Usage: ./analyze_single.sh <image_file>
# ============================================================================

set -e

# 检查参数
if [ $# -eq 0 ]; then
    echo "❌ Error: No image file specified"
    echo "Usage: $0 <image_file>"
    exit 1
fi

IMAGE_FILE="$1"

# 检查文件是否存在
if [ ! -f "$IMAGE_FILE" ]; then
    echo "❌ Error: File not found: $IMAGE_FILE"
    exit 1
fi

# 检查 imgquality 是否可用
if ! command -v imgquality &> /dev/null; then
    echo "❌ Error: imgquality not found"
    echo "Please build the project first:"
    echo "  cd /path/to/imgquality"
    echo "  cargo build --release"
    echo "  export PATH=\"\$PATH:\$(pwd)/target/release\""
    exit 1
fi

echo "╔══════════════════════════════════════════════╗"
echo "║   📊 Image Quality Analysis                  ║"
echo "╚══════════════════════════════════════════════╝"
echo ""

# 执行分析（人类可读格式）
imgquality analyze "$IMAGE_FILE" --recommend

echo ""
echo "═══════════════════════════════════════════════"
echo "💡 Tips:"
echo "  • Use '--output json' for machine-readable output"
echo "  • Use 'imgquality convert' to upgrade the format"
echo "═══════════════════════════════════════════════"

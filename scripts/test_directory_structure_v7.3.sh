#!/bin/bash
# 🔥 v7.3.1: 测试目录结构保留功能（包括失败文件的fallback复制）

set -e

echo "🧪 Testing Directory Structure Preservation v7.3.1"
echo "=================================================="

# 清理旧测试
rm -rf /tmp/test_dir_structure_v7.3
mkdir -p /tmp/test_dir_structure_v7.3/{input,output}

# 创建测试目录结构
mkdir -p /tmp/test_dir_structure_v7.3/input/photos/2024/summer
mkdir -p /tmp/test_dir_structure_v7.3/input/docs/work

# 创建测试文件（使用真实图片）
echo "📝 Creating test files..."

# 1. 正常PNG（会成功转换）
convert -size 100x100 xc:blue /tmp/test_dir_structure_v7.3/input/photos/2024/summer/beach.png

# 2. 创建一个会失败的文件（损坏的图片）
echo "fake image data" > /tmp/test_dir_structure_v7.3/input/docs/work/broken.png

# 3. 创建一个GIF（可能会因为太短而跳过）
convert -size 50x50 xc:red /tmp/test_dir_structure_v7.3/input/photos/cat.gif

echo ""
echo "📂 Input structure:"
tree /tmp/test_dir_structure_v7.3/input || find /tmp/test_dir_structure_v7.3/input -type f

echo ""
echo "🚀 Running conversion..."
./target/release/imgquality-hevc auto \
    /tmp/test_dir_structure_v7.3/input \
    --output /tmp/test_dir_structure_v7.3/output \
    --recursive \
    --verbose

echo ""
echo "📂 Output structure:"
tree /tmp/test_dir_structure_v7.3/output || find /tmp/test_dir_structure_v7.3/output -type f

echo ""
echo "🔍 Verification:"

# 检查目录结构是否保留
if [ -f "/tmp/test_dir_structure_v7.3/output/photos/2024/summer/beach.jxl" ] || \
   [ -f "/tmp/test_dir_structure_v7.3/output/photos/2024/summer/beach.png" ]; then
    echo "✅ beach.png: Directory structure preserved"
else
    echo "❌ beach.png: Directory structure LOST"
    exit 1
fi

if [ -f "/tmp/test_dir_structure_v7.3/output/docs/work/broken.png" ]; then
    echo "✅ broken.png: Failed file copied with directory structure"
else
    echo "❌ broken.png: Failed file NOT copied or structure LOST"
    exit 1
fi

if [ -f "/tmp/test_dir_structure_v7.3/output/photos/cat.gif" ] || \
   [ -f "/tmp/test_dir_structure_v7.3/output/photos/cat.jxl" ]; then
    echo "✅ cat.gif: File converted/copied with directory structure"
else
    echo "❌ cat.gif: File NOT found or structure LOST"
    exit 1
fi

echo ""
echo "✅ All tests passed!"

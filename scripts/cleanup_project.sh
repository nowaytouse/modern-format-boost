#!/bin/bash
# 🔥 v7.3.2: 项目清理脚本 - 删除缓存和旧二进制文件

set -e

echo "🧹 Cleaning Modern Format Boost Project..."
echo "=========================================="

# 1. 清理 Cargo 构建缓存
echo ""
echo "📦 Cleaning Cargo build cache..."
cargo clean
echo "✅ Cargo cache cleaned"

# 2. 删除所有旧的二进制文件（保留最新的 target/release）
echo ""
echo "🗑️  Removing old/misplaced binary files..."

# 删除子目录下可能存在的残留 target 文件夹（归并为一个）
find . -mindepth 2 -name "target" -type d -exec rm -rf {} + 2>/dev/null || true

# 删除旧的二进制文件（已改名或位置不对的）
find . -type f \( -name "imgquality*" -o -name "vidquality*" \) -not -path "*/target/release/*" -delete 2>/dev/null || true

echo "✅ Old binaries and redundant targets removed"

# 3. 清理临时文件
echo ""
echo "🗑️  Removing temporary files..."
find . -name "*.tmp" -delete 2>/dev/null || true
find . -name ".DS_Store" -delete 2>/dev/null || true

echo "✅ Temporary files removed"

# 4. 显示当前二进制文件
echo ""
echo "📋 Current binaries in target/release:"
ls -lh target/release/img-* target/release/vid-* 2>/dev/null || echo "   (No binaries found - run 'cargo build --release')"

# 5. 显示项目大小
echo ""
echo "📊 Project size:"
du -sh . 2>/dev/null || echo "   (Unable to calculate)"

echo ""
echo "✅ Cleanup complete!"

#!/bin/bash
# 批量修复所有工具的 base_dir 支持

cd "$(dirname "$0")"

echo "🔧 修复 imgquality_av1..."
# 在 ConvertOptions 初始化中添加 base_dir: None
sed -i '' 's/let options = ConvertOptions {/let options = ConvertOptions {\n        base_dir: None,/' imgquality_av1/src/main.rs

echo "🔧 修复 vidquality_hevc..."
# 查找并修复 vidquality_hevc/src/main.rs 中的 ConvertOptions
grep -n "ConvertOptions {" vidquality_hevc/src/main.rs || echo "  未找到 ConvertOptions"

echo "🔧 修复 vidquality_av1..."
grep -n "ConvertOptions {" vidquality_av1/src/main.rs || echo "  未找到 ConvertOptions"

echo ""
echo "✅ 完成！现在编译测试..."
cargo build --release -p imgquality-av1 2>&1 | tail -3

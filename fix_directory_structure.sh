#!/bin/bash
# 修复目录结构保留功能
# 在 lossless_converter.rs 中使用 determine_output_path_with_base

cd "$(dirname "$0")"

echo "🔧 修复 lossless_converter.rs 使用新的路径函数..."

# 备份
cp imgquality_hevc/src/lossless_converter.rs imgquality_hevc/src/lossless_converter.rs.bak

# 在文件顶部添加新函数导入
sed -i '' 's/use shared_utils::conversion::{determine_output_path,/use shared_utils::conversion::{determine_output_path, determine_output_path_with_base,/' imgquality_hevc/src/lossless_converter.rs

echo "✅ 已添加新函数导入"
echo "⚠️  需要手动修改各个转换函数使用 determine_output_path_with_base"
echo "   当 options.base_dir.is_some() 时使用新函数"

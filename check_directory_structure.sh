#!/bin/bash
# 检查四个工具是否都支持目录结构保留

cd "$(dirname "$0")"

echo "🔍 检查目录结构保留功能实现状态"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

check_tool() {
    local tool=$1
    local main_file="${tool}/src/main.rs"
    
    echo "📦 检查 $tool..."
    
    # 1. 检查 AutoConvertConfig 是否有 base_dir 字段
    if grep -q "base_dir: Option<PathBuf>" "$main_file" 2>/dev/null; then
        echo "  ✅ AutoConvertConfig 有 base_dir 字段"
    else
        echo "  ❌ AutoConvertConfig 缺少 base_dir 字段"
        return 1
    fi
    
    # 2. 检查是否在 auto_convert_directory 中设置 base_dir
    if grep -q "base_dir.*Some(input.to_path_buf())" "$main_file" 2>/dev/null; then
        echo "  ✅ auto_convert_directory 设置 base_dir"
    else
        echo "  ❌ auto_convert_directory 未设置 base_dir"
        return 1
    fi
    
    # 3. 检查 ConvertOptions 是否传递 base_dir
    if grep -q "base_dir:.*config.base_dir" "$main_file" 2>/dev/null; then
        echo "  ✅ ConvertOptions 传递 base_dir"
    else
        echo "  ⚠️  ConvertOptions 可能未传递 base_dir"
    fi
    
    echo ""
}

# 检查四个工具
check_tool "imgquality_hevc"
check_tool "imgquality_av1"
check_tool "vidquality_hevc"
check_tool "vidquality_av1"

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "🔍 检查 shared_utils 中的路径函数..."

if grep -q "pub fn determine_output_path_with_base" shared_utils/src/conversion.rs; then
    echo "  ✅ determine_output_path_with_base 函数存在"
else
    echo "  ❌ determine_output_path_with_base 函数不存在"
fi

if grep -q "base_dir: Option<PathBuf>" shared_utils/src/conversion.rs; then
    echo "  ✅ ConvertOptions 有 base_dir 字段"
else
    echo "  ❌ ConvertOptions 缺少 base_dir 字段"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

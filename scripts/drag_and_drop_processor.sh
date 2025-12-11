#!/bin/bash
# Modern Format Boost - Drag & Drop Processor
# 拖拽式一键处理脚本
# 
# 使用方法：将文件夹拖拽到此脚本上，或双击后选择文件夹
# Usage: Drag folder to this script, or double-click and select folder
#
# 🔥 v3.9: 新增 XMP 元数据合并功能
#   - 自动检测 .xmp sidecar 文件
#   - 在格式转换前将元数据合并到媒体文件
#   - 合并后自动删除 .xmp 文件

set -e

# 获取脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 工具路径
IMGQUALITY_HEVC="$PROJECT_ROOT/imgquality_hevc/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"

# XMP 合并计数器
XMP_SUCCESS=0
XMP_FAILED=0
XMP_SKIPPED=0

# 检查工具是否存在
check_tools() {
    if [[ ! -f "$IMGQUALITY_HEVC" ]]; then
        echo "❌ imgquality-hevc not found. Building..."
        cd "$PROJECT_ROOT"
        cargo build --release -p imgquality-hevc
    fi
    
    if [[ ! -f "$VIDQUALITY_HEVC" ]]; then
        echo "❌ vidquality-hevc not found. Building..."
        cd "$PROJECT_ROOT"
        cargo build --release -p vidquality-hevc
    fi
}

# 显示欢迎信息
show_welcome() {
    echo "🚀 Modern Format Boost - 一键处理器 v3.9"
    echo "=================================================="
    echo "📁 处理模式：原地转换（删除原文件）"
    echo "📋 XMP合并：自动检测并合并 sidecar 元数据"
    echo "🔧 图像参数：--in-place --recursive --match-quality --explore"
    echo "🎬 视频参数：--in-place --recursive --match-quality true --explore"
    echo "=================================================="
    echo ""
}

# 获取目标目录
get_target_directory() {
    if [[ $# -gt 0 ]]; then
        # 从命令行参数获取（拖拽模式）
        TARGET_DIR="$1"
    else
        # 交互模式：让用户选择目录
        echo "请将要处理的文件夹拖拽到此窗口，然后按回车："
        echo "或者直接输入文件夹路径："
        read -r TARGET_DIR
        
        # 去除可能的引号和空格
        TARGET_DIR=$(echo "$TARGET_DIR" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//;s/^"//;s/"$//')
    fi
    
    # 验证目录
    if [[ ! -d "$TARGET_DIR" ]]; then
        echo "❌ 错误：目录不存在: $TARGET_DIR"
        exit 1
    fi
    
    echo "📂 目标目录: $TARGET_DIR"
}

# 安全检查
safety_check() {
    # 危险目录检查
    case "$TARGET_DIR" in
        "/" | "/System"* | "/usr"* | "/bin"* | "/sbin"* | "$HOME" | "$HOME/Desktop" | "$HOME/Documents")
            echo "❌ 危险目录，拒绝处理: $TARGET_DIR"
            echo "为了安全，请选择具体的子目录进行处理。"
            exit 1
            ;;
    esac
    
    # 确认处理
    echo ""
    echo "⚠️  即将开始原地处理（会删除原文件）："
    echo "   目录: $TARGET_DIR"
    echo "   模式: 递归处理所有子目录"
    echo "   参数: --match-quality --explore"
    echo ""
    echo "确认继续？(y/N): "
    read -r CONFIRM
    
    if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
        echo "❌ 用户取消操作"
        exit 0
    fi
}

# 统计文件数量
count_files() {
    echo "📊 正在统计文件..."
    
    # XMP 文件
    XMP_COUNT=$(find "$TARGET_DIR" -type f -iname "*.xmp" | wc -l | tr -d ' ')
    
    # 图像文件
    IMG_COUNT=$(find "$TARGET_DIR" -type f \( \
        -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.png" -o -iname "*.gif" \
        -o -iname "*.bmp" -o -iname "*.tiff" -o -iname "*.webp" -o -iname "*.heic" \
    \) | wc -l | tr -d ' ')
    
    # 视频文件
    VID_COUNT=$(find "$TARGET_DIR" -type f \( \
        -iname "*.mp4" -o -iname "*.mov" -o -iname "*.avi" -o -iname "*.mkv" \
        -o -iname "*.webm" -o -iname "*.m4v" -o -iname "*.flv" \
    \) | wc -l | tr -d ' ')
    
    echo "   📋 XMP文件:  $XMP_COUNT"
    echo "   🖼️  图像文件: $IMG_COUNT"
    echo "   🎬 视频文件: $VID_COUNT"
    echo "   📁 总计: $((IMG_COUNT + VID_COUNT))"
    
    if [[ $((IMG_COUNT + VID_COUNT)) -eq 0 ]]; then
        echo "❌ 未找到支持的媒体文件"
        exit 1
    fi
}

# 🔥 XMP 元数据合并功能
merge_xmp_files() {
    if [[ $XMP_COUNT -eq 0 ]]; then
        echo "📋 未检测到 XMP 文件，跳过合并步骤"
        return
    fi
    
    # 检查 exiftool 是否可用
    if ! command -v exiftool &> /dev/null; then
        echo "⚠️  ExifTool 未安装，跳过 XMP 合并"
        echo "   安装方法: brew install exiftool"
        return
    fi
    
    echo ""
    echo "📋 开始合并 XMP 元数据..."
    echo "=================================================="
    echo "   检测到 $XMP_COUNT 个 XMP sidecar 文件"
    echo ""
    
    XMP_SUCCESS=0
    XMP_FAILED=0
    XMP_SKIPPED=0
    
    # 遍历所有 XMP 文件
    while IFS= read -r -d '' xmp_file; do
        # 获取基础文件名（去掉 .xmp 后缀）
        base_name="${xmp_file%.*}"
        
        # 检查对应的媒体文件是否存在
        if [[ -f "$base_name" ]]; then
            media_file="$base_name"
        else
            # 尝试查找同名但不同扩展名的文件
            base_name_no_ext="${xmp_file%.xmp}"
            media_file=$(find "$(dirname "$xmp_file")" -maxdepth 1 -type f -name "$(basename "$base_name_no_ext").*" ! -name "*.xmp" | head -n 1)
            
            if [[ -z "$media_file" ]]; then
                echo "   ⏭️  跳过: $(basename "$xmp_file") (无对应媒体文件)"
                ((XMP_SKIPPED++)) || true
                continue
            fi
        fi
        
        # 执行合并
        echo "   🔄 合并: $(basename "$xmp_file") → $(basename "$media_file")"
        
        if exiftool -P -overwrite_original -tagsfromfile "$xmp_file" -all:all "$media_file" > /dev/null 2>&1; then
            # 保留时间戳
            touch -r "$xmp_file" "$media_file" 2>/dev/null || true
            
            # 删除 XMP 文件
            rm "$xmp_file"
            echo "      ✅ 成功，已删除 XMP 文件"
            ((XMP_SUCCESS++)) || true
        else
            echo "      ❌ 合并失败"
            ((XMP_FAILED++)) || true
        fi
        
    done < <(find "$TARGET_DIR" -type f -iname "*.xmp" -print0 2>/dev/null)
    
    echo ""
    echo "📋 XMP 合并完成: ✅ $XMP_SUCCESS 成功, ❌ $XMP_FAILED 失败, ⏭️ $XMP_SKIPPED 跳过"
}

# 处理图像文件
process_images() {
    if [[ $IMG_COUNT -gt 0 ]]; then
        echo ""
        echo "🖼️  开始处理图像文件..."
        echo "=================================================="
        
        "$IMGQUALITY_HEVC" auto "$TARGET_DIR" \
            --in-place \
            --recursive \
            --match-quality \
            --explore
        
        echo "✅ 图像处理完成"
    fi
}

# 处理视频文件
process_videos() {
    if [[ $VID_COUNT -gt 0 ]]; then
        echo ""
        echo "🎬 开始处理视频文件..."
        echo "=================================================="
        
        "$VIDQUALITY_HEVC" auto "$TARGET_DIR" \
            --in-place \
            --recursive \
            --match-quality true \
            --explore
        
        echo "✅ 视频处理完成"
    fi
}

# 显示完成信息
show_completion() {
    echo ""
    echo "🎉 处理完成！"
    echo "=================================================="
    echo "📁 处理目录: $TARGET_DIR"
    if [[ $XMP_COUNT -gt 0 ]]; then
        echo "📋 XMP合并:  ✅ $XMP_SUCCESS 成功"
    fi
    echo "🖼️  图像文件: $IMG_COUNT"
    echo "🎬 视频文件: $VID_COUNT"
    echo "=================================================="
    echo ""
    echo "按任意键退出..."
    read -n 1
}

# 主函数
main() {
    show_welcome
    check_tools
    get_target_directory "$@"
    safety_check
    count_files
    merge_xmp_files  # 🔥 先合并 XMP 元数据
    process_images
    process_videos
    show_completion
}

# 错误处理
trap 'echo "❌ 处理过程中发生错误，请检查日志"; read -n 1' ERR

# 运行主函数
main "$@"
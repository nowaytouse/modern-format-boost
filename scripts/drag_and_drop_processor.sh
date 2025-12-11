#!/bin/bash
# Modern Format Boost - Drag & Drop Processor
# 拖拽式一键处理脚本
# 
# 使用方法：将文件夹拖拽到此脚本上，或双击后选择文件夹
# Usage: Drag folder to this script, or double-click and select folder

set -e

# 获取脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 工具路径
IMGQUALITY_HEVC="$PROJECT_ROOT/imgquality_hevc/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"

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
    echo "🚀 Modern Format Boost - 一键处理器"
    echo "=================================================="
    echo "📁 处理模式：原地转换（删除原文件）"
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
    
    echo "   🖼️  图像文件: $IMG_COUNT"
    echo "   🎬 视频文件: $VID_COUNT"
    echo "   📁 总计: $((IMG_COUNT + VID_COUNT))"
    
    if [[ $((IMG_COUNT + VID_COUNT)) -eq 0 ]]; then
        echo "❌ 未找到支持的媒体文件"
        exit 1
    fi
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
    process_images
    process_videos
    show_completion
}

# 错误处理
trap 'echo "❌ 处理过程中发生错误，请检查日志"; read -n 1' ERR

# 运行主函数
main "$@"
#!/opt/homebrew/bin/bash
# Modern Format Boost - Drag & Drop Processor
# 拖拽式一键处理脚本
# 
# 使用方法：将文件夹拖拽到此脚本上，或双击后选择文件夹
# Usage: Drag folder to this script, or double-click and select folder
#
# 🔥 v5.0: 简化模式
#   - 模式1: 原地转换（删除原文件）
#   - 模式2: 输出到相邻目录（保留原文件）
#   - 断点续传 + 原子操作保护
#   - 预处理验证机制

set -e

# 获取脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 工具路径
IMGQUALITY_HEVC="$PROJECT_ROOT/imgquality_hevc/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"
XMP_MERGER="$PROJECT_ROOT/xmp_merger/target/release/xmp-merge"

# 模式设置
OUTPUT_MODE="inplace"  # inplace 或 adjacent
OUTPUT_DIR=""

# 检查工具是否存在
check_tools() {
    local need_build=false
    
    if [[ ! -f "$IMGQUALITY_HEVC" ]]; then
        echo "❌ imgquality-hevc not found"
        need_build=true
    fi
    
    if [[ ! -f "$VIDQUALITY_HEVC" ]]; then
        echo "❌ vidquality-hevc not found"
        need_build=true
    fi
    
    if [[ ! -f "$XMP_MERGER" ]]; then
        echo "❌ xmp-merge not found"
        need_build=true
    fi
    
    if [[ "$need_build" == "true" ]]; then
        echo "🔧 Building tools..."
        cd "$PROJECT_ROOT"
        cargo build --release -p imgquality-hevc -p vidquality-hevc -p xmp_merger 2>&1 | tail -5
        echo "✅ Build complete"
    fi
}

# 显示欢迎信息
show_welcome() {
    echo ""
    echo "🚀 Modern Format Boost v5.0"
    echo "=================================================="
    echo "📋 XMP合并：自动检测并合并 sidecar 元数据"
    echo "🍎 Apple兼容：默认启用（AV1/VP9 → HEVC）"
    echo "🔄 断点续传：支持中断后继续处理"
    echo "=================================================="
}

# 🔥 选择运行模式
select_mode() {
    echo ""
    echo "请选择输出模式："
    echo "  [1] 🚀 原地转换 - 删除原文件，节省空间"
    echo "  [2] 📂 输出到相邻目录 - 保留原文件，安全预览"
    echo "  [Q] 退出"
    echo ""
    read -r MODE_CHOICE
    
    case "$MODE_CHOICE" in
        1)
            OUTPUT_MODE="inplace"
            echo "✅ 原地转换模式"
            ;;
        2)
            OUTPUT_MODE="adjacent"
            # 创建相邻输出目录
            local base_name=$(basename "$TARGET_DIR")
            OUTPUT_DIR="$(dirname "$TARGET_DIR")/${base_name}_converted"
            mkdir -p "$OUTPUT_DIR"
            echo "✅ 输出到相邻目录: $OUTPUT_DIR"
            ;;
        *)
            echo "❌ 用户取消"
            exit 0
            ;;
    esac
}

# 获取目标目录
get_target_directory() {
    if [[ $# -gt 0 ]]; then
        TARGET_DIR="$1"
    else
        echo "请将要处理的文件夹拖拽到此窗口，然后按回车："
        read -r TARGET_DIR
        TARGET_DIR=$(echo "$TARGET_DIR" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//;s/^"//;s/"$//')
    fi
    
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
            exit 1
            ;;
    esac
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        echo ""
        echo "⚠️  即将开始原地处理（会删除原文件）"
        echo "确认继续？(y/N): "
        read -r CONFIRM
        if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
            echo "❌ 用户取消"
            exit 0
        fi
    fi
}

# 统计文件数量
count_files() {
    echo ""
    echo "📊 统计文件..."
    
    XMP_COUNT=$(find "$TARGET_DIR" -type f -iname "*.xmp" 2>/dev/null | wc -l | tr -d ' ')
    IMG_COUNT=$(find "$TARGET_DIR" -type f \( \
        -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.png" -o -iname "*.gif" \
        -o -iname "*.bmp" -o -iname "*.tiff" -o -iname "*.webp" -o -iname "*.heic" \
    \) 2>/dev/null | wc -l | tr -d ' ')
    VID_COUNT=$(find "$TARGET_DIR" -type f \( \
        -iname "*.mp4" -o -iname "*.mov" -o -iname "*.avi" -o -iname "*.mkv" \
        -o -iname "*.webm" -o -iname "*.m4v" \
    \) 2>/dev/null | wc -l | tr -d ' ')
    
    echo "   📋 XMP:  $XMP_COUNT"
    echo "   🖼️  图像: $IMG_COUNT"
    echo "   🎬 视频: $VID_COUNT"
    
    if [[ $((IMG_COUNT + VID_COUNT)) -eq 0 ]]; then
        echo "❌ 未找到支持的媒体文件"
        exit 1
    fi
}

# XMP 合并
merge_xmp_files() {
    [[ $XMP_COUNT -eq 0 ]] && return
    
    if ! command -v exiftool &> /dev/null; then
        echo "⚠️  ExifTool 未安装，跳过 XMP 合并"
        return
    fi
    
    echo ""
    echo "📋 合并 XMP 元数据..."
    "$XMP_MERGER" --delete-xmp "$TARGET_DIR"
}

# 处理图像
process_images() {
    [[ $IMG_COUNT -eq 0 ]] && return
    
    echo ""
    echo "🖼️  处理图像..."
    
    # 🔥 v4.8: 默认启用 --explore --match-quality --compress --cpu --apple-compat
    local args=(
        auto "$TARGET_DIR"
        --recursive
        --explore
        --match-quality
        --compress
        --cpu
        --apple-compat
    )
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place)
    else
        args+=(--output "$OUTPUT_DIR")
    fi
    
    "$IMGQUALITY_HEVC" "${args[@]}"
}

# 处理视频
process_videos() {
    [[ $VID_COUNT -eq 0 ]] && return
    
    echo ""
    echo "🎬 处理视频..."
    
    # 🔥 v4.8: 默认启用 --explore --match-quality --compress --cpu --apple-compat
    local args=(
        auto "$TARGET_DIR"
        --recursive
        --explore
        --match-quality true
        --compress
        --cpu
        --apple-compat
    )
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        args+=(--in-place)
    else
        args+=(--output "$OUTPUT_DIR")
    fi
    
    "$VIDQUALITY_HEVC" "${args[@]}"
}

# 完成信息
show_completion() {
    echo ""
    echo "🎉 处理完成！"
    echo "=================================================="
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        echo "📂 输出目录: $OUTPUT_DIR"
        echo ""
        echo "是否打开输出目录？(y/N): "
        read -r OPEN_DIR
        if [[ "$OPEN_DIR" =~ ^[Yy]$ ]]; then
            open "$OUTPUT_DIR" 2>/dev/null || true
        fi
    else
        echo "📂 处理目录: $TARGET_DIR"
    fi
    
    echo ""
    echo "按任意键退出..."
    read -n 1
}

# 主函数
main() {
    check_tools
    get_target_directory "$@"
    show_welcome
    select_mode
    safety_check
    count_files
    merge_xmp_files
    process_images
    process_videos
    show_completion
}

trap 'echo ""; echo "⚠️ 处理被中断"; read -n 1' INT TERM
main "$@"

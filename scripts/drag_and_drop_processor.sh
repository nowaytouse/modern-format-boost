#!/bin/bash
# Modern Format Boost - Drag & Drop Processor
# 拖拽式一键处理脚本
# 
# 使用方法：将文件夹拖拽到此脚本上，或双击后选择文件夹
# Usage: Drag folder to this script, or double-click and select folder
#
# 🔥 v4.1: 断点续传 + 原子操作保护
#   - 进度文件记录已处理文件，中断后可续传
#   - 锁文件防止重复运行
#   - XMP 合并支持断点续传

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

# 🔥 断点续传相关
PROGRESS_DIR=""
PROGRESS_FILE=""
LOCK_FILE=""
RESUME_MODE=false

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
    echo "🚀 Modern Format Boost - 一键处理器 v4.1"
    echo "=================================================="
    echo "📁 处理模式：原地转换（删除原文件）"
    echo "📋 XMP合并：自动检测并合并 sidecar 元数据"
    echo "🍎 Apple兼容：默认启用（AV1/VP9 → HEVC）"
    echo "🔄 断点续传：支持中断后继续处理"
    echo "=================================================="
    echo ""
}

# 🔥 初始化断点续传
init_progress_tracking() {
    # 使用目录路径的 hash 作为唯一标识
    local dir_hash=$(echo "$TARGET_DIR" | md5 | cut -c1-8)
    PROGRESS_DIR="$TARGET_DIR/.mfb_progress"
    PROGRESS_FILE="$PROGRESS_DIR/completed_$dir_hash.txt"
    LOCK_FILE="$PROGRESS_DIR/processing.lock"
    
    # 创建进度目录
    mkdir -p "$PROGRESS_DIR"
    
    # 检查是否有未完成的任务
    if [[ -f "$LOCK_FILE" ]]; then
        local lock_pid=$(cat "$LOCK_FILE" 2>/dev/null)
        if kill -0 "$lock_pid" 2>/dev/null; then
            echo "❌ 另一个处理进程正在运行 (PID: $lock_pid)"
            echo "   如果确认没有其他进程，请删除: $LOCK_FILE"
            exit 1
        else
            echo "⚠️  检测到上次处理被中断"
            rm -f "$LOCK_FILE"
        fi
    fi
    
    # 检查是否有进度文件（断点续传）
    if [[ -f "$PROGRESS_FILE" ]]; then
        local completed_count=$(wc -l < "$PROGRESS_FILE" | tr -d ' ')
        if [[ $completed_count -gt 0 ]]; then
            echo ""
            echo "🔄 检测到上次未完成的任务"
            echo "   已完成: $completed_count 个文件"
            echo ""
            echo "选择操作："
            echo "  [R] 继续上次任务（跳过已处理文件）"
            echo "  [N] 重新开始（清除进度）"
            echo "  [Q] 退出"
            read -r RESUME_CHOICE
            
            case "$RESUME_CHOICE" in
                [Rr])
                    RESUME_MODE=true
                    echo "✅ 将继续上次任务"
                    ;;
                [Nn])
                    rm -f "$PROGRESS_FILE"
                    echo "✅ 已清除进度，重新开始"
                    ;;
                *)
                    echo "❌ 用户取消"
                    exit 0
                    ;;
            esac
        fi
    fi
    
    # 创建锁文件
    echo $$ > "$LOCK_FILE"
}

# 🔥 检查文件是否已处理
is_file_completed() {
    local file_path="$1"
    if [[ "$RESUME_MODE" == "true" ]] && [[ -f "$PROGRESS_FILE" ]]; then
        grep -qxF "$file_path" "$PROGRESS_FILE" 2>/dev/null
        return $?
    fi
    return 1
}

# 🔥 标记文件已完成
mark_file_completed() {
    local file_path="$1"
    echo "$file_path" >> "$PROGRESS_FILE"
}

# 🔥 清理进度文件（任务完成时）
cleanup_progress() {
    if [[ -d "$PROGRESS_DIR" ]]; then
        rm -f "$LOCK_FILE"
        # 任务完成后删除进度文件
        rm -f "$PROGRESS_FILE"
        # 如果目录为空则删除
        rmdir "$PROGRESS_DIR" 2>/dev/null || true
    fi
}

# 🔥 中断处理
handle_interrupt() {
    echo ""
    echo "⚠️  处理被中断！"
    echo "   进度已保存，下次运行可继续处理"
    rm -f "$LOCK_FILE"
    exit 130
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
        # 🔥 断点续传：检查是否已处理
        if is_file_completed "xmp:$xmp_file"; then
            ((XMP_SKIPPED++)) || true
            continue
        fi
        
        # 获取基础文件名（去掉 .xmp 后缀）
        base_name="${xmp_file%.*}"
        
        # 检查对应的媒体文件是否存在
        if [[ -f "$base_name" ]]; then
            media_file="$base_name"
        else
            # 🔥 优化：直接检查常见扩展名，避免 find 的性能问题
            base_name_no_ext="${xmp_file%.xmp}"
            dir_path="$(dirname "$xmp_file")"
            file_stem="$(basename "$base_name_no_ext")"
            media_file=""
            
            # 遍历常见媒体扩展名，直接检查文件是否存在（最快）
            for ext in mp4 mov mkv avi webm gif png jpg jpeg webp avif heic tiff bmp; do
                candidate="$dir_path/$file_stem.$ext"
                if [[ -f "$candidate" ]]; then
                    media_file="$candidate"
                    break
                fi
            done
            
            if [[ -z "$media_file" ]]; then
                echo "   ⏭️  跳过: $(basename "$xmp_file") (无对应媒体文件)"
                mark_file_completed "xmp:$xmp_file"
                ((XMP_SKIPPED++)) || true
                continue
            fi
        fi
        
        # 执行合并
        echo "   🔄 合并: $(basename "$xmp_file") → $(basename "$media_file")"
        
        # 🔥 创建临时文件保存媒体文件的原始时间戳（在 exiftool 修改前）
        timestamp_ref=$(mktemp)
        touch -r "$media_file" "$timestamp_ref" 2>/dev/null || true
        
        if exiftool -P -overwrite_original -tagsfromfile "$xmp_file" -all:all "$media_file" > /dev/null 2>&1; then
            # 🔥 恢复媒体文件的原始时间戳（exiftool 会修改时间戳）
            touch -r "$timestamp_ref" "$media_file" 2>/dev/null || true
            rm -f "$timestamp_ref"
            
            # 删除 XMP 文件
            rm "$xmp_file"
            echo "      ✅ 成功，已删除 XMP 文件"
            mark_file_completed "xmp:$xmp_file"
            ((XMP_SUCCESS++)) || true
        else
            rm -f "$timestamp_ref"
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
            --explore \
            --apple-compat
        
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
            --explore \
            --apple-compat
        
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
    
    # 🔥 初始化断点续传（在 safety_check 之前，以便检测未完成任务）
    init_progress_tracking
    
    safety_check
    count_files
    merge_xmp_files  # 🔥 先合并 XMP 元数据
    process_images
    process_videos
    
    # 🔥 任务完成，清理进度文件
    cleanup_progress
    
    show_completion
}

# 🔥 错误和中断处理
trap 'handle_interrupt' INT TERM
trap 'echo "❌ 处理过程中发生错误，进度已保存"; rm -f "$LOCK_FILE"; read -n 1' ERR

# 运行主函数
main "$@"
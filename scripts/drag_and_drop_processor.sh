#!/opt/homebrew/bin/bash
# Modern Format Boost - Drag & Drop Processor
# 拖拽式一键处理脚本
# 
# 使用方法：将文件夹拖拽到此脚本上，或双击后选择文件夹
# Usage: Drag folder to this script, or double-click and select folder
#
# 🔥 v5.1: 改进交互体验
#   - 方向键选择模式
#   - 统一进度条样式
#   - 更好的视觉反馈

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

# ═══════════════════════════════════════════════════════════════
# 终端颜色和样式
# ═══════════════════════════════════════════════════════════════
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m' # No Color

# 清屏并移动光标
clear_screen() {
    printf "\033[2J\033[H"
}

# 隐藏/显示光标
hide_cursor() { printf "\033[?25l"; }
show_cursor() { printf "\033[?25h"; }

# 移动光标到指定行
move_to_line() { printf "\033[%d;0H" "$1"; }

# 清除当前行
clear_line() { printf "\033[2K"; }

# ═══════════════════════════════════════════════════════════════
# 方向键选择菜单
# ═══════════════════════════════════════════════════════════════
select_with_arrows() {
    local options=("$@")
    local selected=0
    local count=${#options[@]}
    
    hide_cursor
    
    # 保存起始行
    local start_line
    start_line=$(tput lines)
    
    while true; do
        # 显示选项
        for i in "${!options[@]}"; do
            if [[ $i -eq $selected ]]; then
                echo -e "  ${GREEN}▶ ${BOLD}${options[$i]}${NC}"
            else
                echo -e "    ${DIM}${options[$i]}${NC}"
            fi
        done
        
        # 读取按键
        read -rsn1 key
        
        # 处理方向键（方向键是 ESC + [ + A/B/C/D）
        if [[ $key == $'\x1b' ]]; then
            read -rsn2 key
            case $key in
                '[A') # 上
                    ((selected--))
                    [[ $selected -lt 0 ]] && selected=$((count - 1))
                    ;;
                '[B') # 下
                    ((selected++))
                    [[ $selected -ge $count ]] && selected=0
                    ;;
            esac
        elif [[ $key == '' ]]; then  # Enter
            break
        elif [[ $key == 'q' || $key == 'Q' ]]; then
            show_cursor
            echo ""
            echo -e "${RED}❌ 用户取消${NC}"
            exit 0
        fi
        
        # 清除已显示的选项，重新绘制
        for ((i=0; i<count; i++)); do
            printf "\033[A\033[2K"
        done
    done
    
    show_cursor
    return $selected
}

# ═══════════════════════════════════════════════════════════════
# 固定位置进度条
# ═══════════════════════════════════════════════════════════════
draw_progress_bar() {
    local current=$1
    local total=$2
    local width=50
    local percent=$((current * 100 / total))
    local filled=$((current * width / total))
    local empty=$((width - filled))
    
    # 构建进度条
    local bar=""
    for ((i=0; i<filled; i++)); do bar+="█"; done
    for ((i=0; i<empty; i++)); do bar+="░"; done
    
    # 颜色根据进度变化
    local color=$GREEN
    [[ $percent -lt 30 ]] && color=$RED
    [[ $percent -ge 30 && $percent -lt 70 ]] && color=$YELLOW
    
    printf "\r  ${color}[${bar}]${NC} ${BOLD}%3d%%${NC} (%d/%d)" "$percent" "$current" "$total"
}

# ═══════════════════════════════════════════════════════════════
# 检查工具
# ═══════════════════════════════════════════════════════════════
check_tools() {
    local need_build=false
    
    if [[ ! -f "$IMGQUALITY_HEVC" ]]; then
        echo -e "${RED}❌ imgquality-hevc not found${NC}"
        need_build=true
    fi
    
    if [[ ! -f "$VIDQUALITY_HEVC" ]]; then
        echo -e "${RED}❌ vidquality-hevc not found${NC}"
        need_build=true
    fi
    
    if [[ ! -f "$XMP_MERGER" ]]; then
        echo -e "${RED}❌ xmp-merge not found${NC}"
        need_build=true
    fi
    
    if [[ "$need_build" == "true" ]]; then
        echo -e "${YELLOW}🔧 Building tools...${NC}"
        cd "$PROJECT_ROOT"
        cargo build --release -p imgquality-hevc -p vidquality-hevc -p xmp_merger 2>&1 | tail -5
        echo -e "${GREEN}✅ Build complete${NC}"
    fi
}

# ═══════════════════════════════════════════════════════════════
# 显示欢迎信息
# ═══════════════════════════════════════════════════════════════
show_welcome() {
    clear_screen
    echo ""
    echo -e "${CYAN}${BOLD}"
    echo "  ╔══════════════════════════════════════════════════════╗"
    echo "  ║                                                      ║"
    echo "  ║     🚀 Modern Format Boost v5.1                      ║"
    echo "  ║                                                      ║"
    echo "  ╚══════════════════════════════════════════════════════╝"
    echo -e "${NC}"
    echo ""
    echo -e "  ${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "  ${BLUE}📋${NC} XMP合并：自动检测并合并 sidecar 元数据"
    echo -e "  ${BLUE}🍎${NC} Apple兼容：默认启用（AV1/VP9 → HEVC）"
    echo -e "  ${BLUE}🔄${NC} 断点续传：支持中断后继续处理"
    echo -e "  ${BLUE}🎯${NC} 智能压缩：v4.13 三阶段精确搜索"
    echo -e "  ${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
}

# ═══════════════════════════════════════════════════════════════
# 选择运行模式（方向键）
# ═══════════════════════════════════════════════════════════════
select_mode() {
    echo -e "${BOLD}请选择输出模式：${NC} ${DIM}(↑↓ 选择, Enter 确认, Q 退出)${NC}"
    echo ""
    
    local options=(
        "🚀 原地转换 - 删除原文件，节省空间"
        "📂 输出到相邻目录 - 保留原文件，安全预览"
    )
    
    select_with_arrows "${options[@]}"
    local choice=$?
    
    echo ""
    
    case $choice in
        0)
            OUTPUT_MODE="inplace"
            echo -e "${GREEN}✅ 已选择：原地转换模式${NC}"
            ;;
        1)
            OUTPUT_MODE="adjacent"
            local base_name=$(basename "$TARGET_DIR")
            OUTPUT_DIR="$(dirname "$TARGET_DIR")/${base_name}_converted"
            mkdir -p "$OUTPUT_DIR"
            echo -e "${GREEN}✅ 已选择：输出到相邻目录${NC}"
            echo -e "   ${DIM}→ $OUTPUT_DIR${NC}"
            ;;
    esac
    echo ""
}

# ═══════════════════════════════════════════════════════════════
# 获取目标目录
# ═══════════════════════════════════════════════════════════════
get_target_directory() {
    if [[ $# -gt 0 ]]; then
        TARGET_DIR="$1"
    else
        echo -e "${BOLD}请将要处理的文件夹拖拽到此窗口，然后按回车：${NC}"
        echo ""
        read -r TARGET_DIR
        TARGET_DIR=$(echo "$TARGET_DIR" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//;s/^"//;s/"$//')
    fi
    
    if [[ ! -d "$TARGET_DIR" ]]; then
        echo -e "${RED}❌ 错误：目录不存在: $TARGET_DIR${NC}"
        exit 1
    fi
    
    echo -e "${BLUE}📂${NC} 目标目录: ${BOLD}$TARGET_DIR${NC}"
}

# ═══════════════════════════════════════════════════════════════
# 安全检查
# ═══════════════════════════════════════════════════════════════
safety_check() {
    # 危险目录检查
    case "$TARGET_DIR" in
        "/" | "/System"* | "/usr"* | "/bin"* | "/sbin"* | "$HOME" | "$HOME/Desktop" | "$HOME/Documents")
            echo -e "${RED}❌ 危险目录，拒绝处理: $TARGET_DIR${NC}"
            exit 1
            ;;
    esac
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        echo ""
        echo -e "${YELLOW}⚠️  即将开始原地处理（会删除原文件）${NC}"
        echo -e "${BOLD}确认继续？${NC} ${DIM}(y/N)${NC}: "
        read -r CONFIRM
        if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
            echo -e "${RED}❌ 用户取消${NC}"
            exit 0
        fi
    fi
}

# ═══════════════════════════════════════════════════════════════
# 统计文件数量
# ═══════════════════════════════════════════════════════════════
count_files() {
    echo ""
    echo -e "${CYAN}📊 统计文件...${NC}"
    
    XMP_COUNT=$(find "$TARGET_DIR" -type f -iname "*.xmp" 2>/dev/null | wc -l | tr -d ' ')
    IMG_COUNT=$(find "$TARGET_DIR" -type f \( \
        -iname "*.jpg" -o -iname "*.jpeg" -o -iname "*.png" -o -iname "*.gif" \
        -o -iname "*.bmp" -o -iname "*.tiff" -o -iname "*.webp" -o -iname "*.heic" \
    \) 2>/dev/null | wc -l | tr -d ' ')
    VID_COUNT=$(find "$TARGET_DIR" -type f \( \
        -iname "*.mp4" -o -iname "*.mov" -o -iname "*.avi" -o -iname "*.mkv" \
        -o -iname "*.webm" -o -iname "*.m4v" \
    \) 2>/dev/null | wc -l | tr -d ' ')
    
    echo ""
    echo -e "  ${DIM}┌─────────────────────────────┐${NC}"
    echo -e "  ${DIM}│${NC}  📋 XMP:  ${BOLD}$XMP_COUNT${NC}"
    echo -e "  ${DIM}│${NC}  🖼️  图像: ${BOLD}$IMG_COUNT${NC}"
    echo -e "  ${DIM}│${NC}  🎬 视频: ${BOLD}$VID_COUNT${NC}"
    echo -e "  ${DIM}└─────────────────────────────┘${NC}"
    
    if [[ $((IMG_COUNT + VID_COUNT)) -eq 0 ]]; then
        echo -e "${RED}❌ 未找到支持的媒体文件${NC}"
        exit 1
    fi
}

# ═══════════════════════════════════════════════════════════════
# XMP 合并
# ═══════════════════════════════════════════════════════════════
merge_xmp_files() {
    [[ $XMP_COUNT -eq 0 ]] && return
    
    if ! command -v exiftool &> /dev/null; then
        echo -e "${YELLOW}⚠️  ExifTool 未安装，跳过 XMP 合并${NC}"
        return
    fi
    
    echo ""
    echo -e "${CYAN}📋 合并 XMP 元数据...${NC}"
    "$XMP_MERGER" --delete-xmp "$TARGET_DIR"
}

# ═══════════════════════════════════════════════════════════════
# 处理图像
# ═══════════════════════════════════════════════════════════════
process_images() {
    [[ $IMG_COUNT -eq 0 ]] && return
    
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BOLD}🖼️  处理图像 ($IMG_COUNT 个文件)${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    
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

# ═══════════════════════════════════════════════════════════════
# 处理视频
# ═══════════════════════════════════════════════════════════════
process_videos() {
    [[ $VID_COUNT -eq 0 ]] && return
    
    echo ""
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${BOLD}🎬 处理视频 ($VID_COUNT 个文件)${NC}"
    echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
    
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

# ═══════════════════════════════════════════════════════════════
# 完成信息
# ═══════════════════════════════════════════════════════════════
show_completion() {
    echo ""
    echo -e "${GREEN}${BOLD}"
    echo "  ╔══════════════════════════════════════════════════════╗"
    echo "  ║                                                      ║"
    echo "  ║     🎉 处理完成！                                    ║"
    echo "  ║                                                      ║"
    echo "  ╚══════════════════════════════════════════════════════╝"
    echo -e "${NC}"
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        echo -e "  ${BLUE}📂${NC} 输出目录: ${BOLD}$OUTPUT_DIR${NC}"
        echo ""
        echo -e "  ${BOLD}是否打开输出目录？${NC} ${DIM}(y/N)${NC}: "
        read -r OPEN_DIR
        if [[ "$OPEN_DIR" =~ ^[Yy]$ ]]; then
            open "$OUTPUT_DIR" 2>/dev/null || true
        fi
    else
        echo -e "  ${BLUE}📂${NC} 处理目录: ${BOLD}$TARGET_DIR${NC}"
    fi
    
    echo ""
    echo -e "  ${DIM}按任意键退出...${NC}"
    read -n 1
}

# ═══════════════════════════════════════════════════════════════
# 主函数
# ═══════════════════════════════════════════════════════════════
main() {
    # 确保退出时显示光标
    trap 'show_cursor; echo ""; echo -e "${YELLOW}⚠️ 处理被中断${NC}"; read -n 1' INT TERM EXIT
    
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
    
    # 正常退出时移除 trap
    trap - EXIT
}

main "$@"

#!/opt/homebrew/bin/bash
# Modern Format Boost - Drag & Drop Processor v5.5
# 
# 🔥 v5.5: 全面改进进度显示
# - 固定底部进度条
# - 详细进度参数
# - 实时 CRF/SSIM 显示
# 
# 使用方法：将文件夹拖拽到此脚本上，或双击后选择文件夹

# 获取脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# 工具路径
IMGQUALITY_HEVC="$PROJECT_ROOT/imgquality_hevc/target/release/imgquality-hevc"
VIDQUALITY_HEVC="$PROJECT_ROOT/vidquality_hevc/target/release/vidquality-hevc"
XMP_MERGER="$PROJECT_ROOT/xmp_merger/target/release/xmp-merge"

# 模式设置
OUTPUT_MODE="inplace"
OUTPUT_DIR=""
SELECTED=0

# 终端颜色
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# 🔥 v5.5: 进度条辅助函数
TERM_WIDTH=$(tput cols 2>/dev/null || echo 80)
PROGRESS_LINE=""

# 清除当前行并打印
print_progress() {
    printf '\r\033[K%s' "$1"
}

# 打印固定底部进度框
print_progress_box() {
    local stage="$1"
    local current="$2"
    local total="$3"
    local file="$4"
    local extra="$5"
    
    local pct=$((current * 100 / total))
    local bar_width=30
    local filled=$((pct * bar_width / 100))
    local empty=$((bar_width - filled))
    
    local bar=""
    for ((i=0; i<filled; i++)); do bar+="━"; done
    for ((i=0; i<empty; i++)); do bar+="─"; done
    
    printf '\r\033[K'
    printf "${CYAN}│${NC} %s ${CYAN}│${NC} [${GREEN}%s${NC}] %d/%d (%d%%) ${CYAN}│${NC} %s ${CYAN}│${NC}" \
        "$stage" "$bar" "$current" "$total" "$pct" "${file:0:30}"
    [[ -n "$extra" ]] && printf " %s" "$extra"
}

# ═══════════════════════════════════════════════════════════════
# 方向键选择菜单 (v5.2)
# 使用全局变量 SELECTED 返回结果，避免 set -e 问题
# ═══════════════════════════════════════════════════════════════
select_menu() {
    local opt1="$1"
    local opt2="$2"
    SELECTED=0
    
    # 隐藏光标
    printf '\033[?25l'
    
    # 绘制函数
    draw() {
        if [[ $SELECTED -eq 0 ]]; then
            printf "  \033[32m▶ \033[1m%s\033[0m\n" "$opt1"
            printf "    \033[2m%s\033[0m\n" "$opt2"
        else
            printf "    \033[2m%s\033[0m\n" "$opt1"
            printf "  \033[32m▶ \033[1m%s\033[0m\n" "$opt2"
        fi
    }
    
    # 清除两行
    clear2() {
        printf '\033[A\033[2K\033[A\033[2K'
    }
    
    draw
    
    while true; do
        # 读取一个字符
        local c
        IFS= read -rsn1 c 2>/dev/null || c=""
        
        # 检查 ESC 序列
        if [[ "$c" == $'\033' ]]; then
            local c2 c3
            IFS= read -rsn1 -t 0.1 c2 2>/dev/null || c2=""
            IFS= read -rsn1 -t 0.1 c3 2>/dev/null || c3=""
            # 上箭头: ESC [ A 或 ESC O A
            if [[ "$c2" == "[" && "$c3" == "A" ]] || [[ "$c2" == "O" && "$c3" == "A" ]]; then
                SELECTED=$((1 - SELECTED))
                clear2; draw
            # 下箭头: ESC [ B 或 ESC O B
            elif [[ "$c2" == "[" && "$c3" == "B" ]] || [[ "$c2" == "O" && "$c3" == "B" ]]; then
                SELECTED=$((1 - SELECTED))
                clear2; draw
            fi
        # Enter
        elif [[ "$c" == "" ]]; then
            break
        # j/k vim 风格
        elif [[ "$c" == "j" || "$c" == "k" ]]; then
            SELECTED=$((1 - SELECTED))
            clear2; draw
        # 数字 1/2
        elif [[ "$c" == "1" ]]; then
            SELECTED=0; clear2; draw
        elif [[ "$c" == "2" ]]; then
            SELECTED=1; clear2; draw
        # q 退出
        elif [[ "$c" == "q" || "$c" == "Q" ]]; then
            printf '\033[?25h'
            echo -e "\n${RED}❌ 用户取消${NC}"
            exit 0
        fi
    done
    
    printf '\033[?25h'
}

# ═══════════════════════════════════════════════════════════════
# 检查工具
# ═══════════════════════════════════════════════════════════════
check_tools() {
    # 🔥 v5.35: 强制重新编译以确保使用最新代码
    echo -e "${YELLOW}🔧 Rebuilding tools with latest fixes...${NC}"
    (cd "$PROJECT_ROOT/imgquality_hevc" && cargo build --release 2>/dev/null) &
    (cd "$PROJECT_ROOT/vidquality_hevc" && cargo build --release 2>/dev/null) &
    (cd "$PROJECT_ROOT/xmp_merger" && cargo build --release 2>/dev/null) &
    wait
    echo -e "${GREEN}✅ Build complete${NC}"
}

# ═══════════════════════════════════════════════════════════════
# 显示欢迎信息
# ═══════════════════════════════════════════════════════════════
show_welcome() {
    printf '\033[2J\033[H'
    echo ""
    echo -e "${CYAN}${BOLD}"
    echo "  ╔══════════════════════════════════════════════════════════════════════════╗"
    echo "  ║     🚀 Modern Format Boost v5.5                                          ║"
    echo "  ║     全面改进进度显示 - 固定底部进度条 + 详细参数                         ║"
    echo "  ╚══════════════════════════════════════════════════════════════════════════╝"
    echo -e "${NC}"
    echo -e "  ${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "  ${BLUE}📋${NC} XMP合并  ${BLUE}🍎${NC} Apple兼容  ${BLUE}🔄${NC} 断点续传  ${BLUE}🎯${NC} v4.13算法  ${MAGENTA}📊${NC} 实时进度"
    echo -e "  ${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
}

# ═══════════════════════════════════════════════════════════════
# 选择运行模式
# ═══════════════════════════════════════════════════════════════
select_mode() {
    echo -e "${BOLD}请选择输出模式：${NC} ${DIM}(↑↓/jk 选择, Enter 确认, Q 退出)${NC}"
    echo ""
    
    select_menu "🚀 原地转换 - 删除原文件，节省空间" "📂 输出到相邻目录 - 保留原文件，安全预览"
    
    echo ""
    if [[ $SELECTED -eq 0 ]]; then
        OUTPUT_MODE="inplace"
        echo -e "${GREEN}✅ 已选择：原地转换模式${NC}"
    else
        OUTPUT_MODE="adjacent"
        local base_name
        base_name=$(basename "$TARGET_DIR")
        OUTPUT_DIR="$(dirname "$TARGET_DIR")/${base_name}_converted"
        mkdir -p "$OUTPUT_DIR"
        echo -e "${GREEN}✅ 已选择：输出到相邻目录${NC}"
        echo -e "   ${DIM}→ $OUTPUT_DIR${NC}"
    fi
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
        read -r TARGET_DIR
        TARGET_DIR="${TARGET_DIR%\"}"
        TARGET_DIR="${TARGET_DIR#\"}"
        TARGET_DIR="${TARGET_DIR%\'}"
        TARGET_DIR="${TARGET_DIR#\'}"
        TARGET_DIR="${TARGET_DIR## }"
        TARGET_DIR="${TARGET_DIR%% }"
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
    case "$TARGET_DIR" in
        "/"|"/System"*|"/usr"*|"/bin"*|"/sbin"*|"$HOME"|"$HOME/Desktop"|"$HOME/Documents")
            echo -e "${RED}❌ 危险目录，拒绝处理: $TARGET_DIR${NC}"
            exit 1
            ;;
    esac
    
    if [[ "$OUTPUT_MODE" == "inplace" ]]; then
        echo -e "${YELLOW}⚠️  即将开始原地处理（会删除原文件）${NC}"
        echo -ne "${BOLD}确认继续？${NC} ${DIM}(y/N)${NC}: "
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
    
    echo -e "  📋 XMP: ${BOLD}$XMP_COUNT${NC}  🖼️ 图像: ${BOLD}$IMG_COUNT${NC}  🎬 视频: ${BOLD}$VID_COUNT${NC}"
    
    if [[ $((IMG_COUNT + VID_COUNT)) -eq 0 ]]; then
        echo -e "${RED}❌ 未找到支持的媒体文件${NC}"
        exit 1
    fi
}

# ═══════════════════════════════════════════════════════════════
# XMP 合并
# ═══════════════════════════════════════════════════════════════
merge_xmp_files() {
    [[ $XMP_COUNT -eq 0 ]] && return 0
    command -v exiftool &>/dev/null || { echo -e "${YELLOW}⚠️ ExifTool 未安装${NC}"; return 0; }
    echo -e "${CYAN}📋 合并 XMP 元数据...${NC}"
    "$XMP_MERGER" --delete-xmp "$TARGET_DIR" || true
}

# ═══════════════════════════════════════════════════════════════
# 处理图像
# ═══════════════════════════════════════════════════════════════
process_images() {
    [[ $IMG_COUNT -eq 0 ]] && return 0

    echo ""
    echo -e "${CYAN}╭─────────────────────────────────────────────────────────────────────────╮${NC}"
    echo -e "${CYAN}│${NC} ${BOLD}🖼️  处理图像${NC} │ $IMG_COUNT 个文件 │ --explore --match-quality --compress │"
    echo -e "${CYAN}╰─────────────────────────────────────────────────────────────────────────╯${NC}"
    echo -e "${DIM}   进度条将显示: CRF 值 | SSIM | 大小变化 | 迭代次数 | 耗时${NC}"
    echo ""

    # 🔥 v5.35: 使用 --explore --match-quality --compress 组合
    local args=(auto "$TARGET_DIR" --recursive --explore --match-quality --compress --apple-compat)
    [[ "$OUTPUT_MODE" == "inplace" ]] && args+=(--in-place) || args+=(--output "$OUTPUT_DIR")

    # 🔥 v5.35: 完全禁止键盘交互 - 多层防护
    # 1. 关闭stdin文件描述符
    # 2. 禁用终端canonical模式和echo
    # 3. 设置TERM=dumb告诉程序终端不支持交互
    exec 0<&-
    stty -echo -icanon 2>/dev/null || true
    TERM=dumb "$IMGQUALITY_HEVC" "${args[@]}" || true
    stty echo icanon 2>/dev/null || true
}

# ═══════════════════════════════════════════════════════════════
# 处理视频
# ═══════════════════════════════════════════════════════════════
process_videos() {
    [[ $VID_COUNT -eq 0 ]] && return 0

    echo ""
    echo -e "${CYAN}╭─────────────────────────────────────────────────────────────────────────╮${NC}"
    echo -e "${CYAN}│${NC} ${BOLD}🎬 处理视频${NC} │ $VID_COUNT 个文件 │ --explore --match-quality --compress │"
    echo -e "${CYAN}╰─────────────────────────────────────────────────────────────────────────╯${NC}"
    echo -e "${DIM}   进度条将显示: CRF 值 | SSIM | 大小变化 | 迭代次数 | 耗时${NC}"
    echo ""

    # 🔥 v5.5: 使用 --explore --match-quality --compress 组合
    local args=(auto "$TARGET_DIR" --recursive --explore --match-quality true --compress --apple-compat)
    [[ "$OUTPUT_MODE" == "inplace" ]] && args+=(--in-place) || args+=(--output "$OUTPUT_DIR")

    # 🔥 v5.35: 完全禁止键盘交互 - 多层防护
    # 1. 关闭stdin文件描述符
    # 2. 禁用终端canonical模式和echo
    # 3. 设置TERM=dumb告诉程序终端不支持交互
    exec 0<&-
    stty -echo -icanon 2>/dev/null || true
    TERM=dumb "$VIDQUALITY_HEVC" "${args[@]}" || true
    stty echo icanon 2>/dev/null || true
}

# ═══════════════════════════════════════════════════════════════
# 完成信息
# ═══════════════════════════════════════════════════════════════
show_completion() {
    echo ""
    echo -e "${GREEN}${BOLD}╭─────────────────────────────────────────────────────────────────────────╮"
    echo -e "│     🎉 处理完成！                                                       │"
    echo -e "╰─────────────────────────────────────────────────────────────────────────╯${NC}"
    
    # 显示处理摘要
    echo -e "  ${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "  📊 处理摘要:"
    echo -e "     🖼️  图像: $IMG_COUNT 个"
    echo -e "     🎬 视频: $VID_COUNT 个"
    echo -e "     📋 XMP:  $XMP_COUNT 个"
    echo -e "  ${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    
    if [[ "$OUTPUT_MODE" == "adjacent" ]]; then
        echo -e "  ${BLUE}📂${NC} 输出目录: ${BOLD}$OUTPUT_DIR${NC}"
        echo -ne "  是否打开？ ${DIM}(y/N)${NC}: "
        read -r ans
        [[ "$ans" =~ ^[Yy]$ ]] && open "$OUTPUT_DIR" 2>/dev/null
    fi
    
    echo ""
    echo -e "  ${DIM}按任意键退出...${NC}"
    read -rsn1
}

# ═══════════════════════════════════════════════════════════════
# 主函数
# ═══════════════════════════════════════════════════════════════
main() {
    trap 'printf "\033[?25h"; echo -e "\n${YELLOW}⚠️ 中断${NC}"' INT TERM
    
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

main "$@"

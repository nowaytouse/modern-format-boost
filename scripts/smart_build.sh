#!/usr/bin/env bash
# Smart Build System v7.4.1 - 智能选择性构建
# 
# 🔥 v7.4.1 修复：
# - ✅ 兼容 macOS bash 3.x（移除关联数组）
# 🔥 v7.4 特性：
# - ✅ 选择性构建（仅构建需要的项目）
# - ✅ 智能清理过时二进制
# - ✅ 智能时间戳比对
# - ✅ 强制重新构建选项
# - ✅ 准确的路径处理

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

# ═══════════════════════════════════════════════════════════════
# 颜色定义
# ═══════════════════════════════════════════════════════════════
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# ═══════════════════════════════════════════════════════════════
# 项目配置 - 兼容 bash 3.x
# ═══════════════════════════════════════════════════════════════
# 格式: "项目目录:二进制名称"
ALL_PROJECTS=(
    "imgquality_hevc:imgquality-hevc"
    "vidquality_hevc:vidquality-hevc"
    "imgquality_av1:imgquality-av1"
    "vidquality_av1:vidquality-av1"
    "xmp_merger:xmp-merge"
)

# 默认构建项目（HEVC工具）
DEFAULT_PROJECTS=("imgquality_hevc" "vidquality_hevc")

# 辅助函数：根据项目目录获取二进制名称
get_binary_name() {
    local project_dir="$1"
    for entry in "${ALL_PROJECTS[@]}"; do
        local dir="${entry%%:*}"
        local bin="${entry##*:}"
        if [[ "$dir" == "$project_dir" ]]; then
            echo "$bin"
            return 0
        fi
    done
    echo ""
    return 1
}

# CLI 参数
FORCE_REBUILD=false
CLEAN_BUILD=false
VERBOSE=false
CLEAN_OLD_BINARIES=true
BUILD_ALL=false
SELECTED_PROJECTS=()

# ═══════════════════════════════════════════════════════════════
# 输出函数
# ═══════════════════════════════════════════════════════════════
print_header() {
    echo ""
    echo -e "${CYAN}${BOLD}🔧 Smart Build System v7.4${NC}"
    echo -e "${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

print_status() {
    local project="$1"
    local action="$2"
    local reason="$3"
    
    if [[ "$action" == "skip" ]]; then
        echo -e "${GREEN}✓${NC} ${BOLD}$project${NC} ${DIM}(up-to-date)${NC}"
    elif [[ "$action" == "rebuild" ]]; then
        echo -e "${YELLOW}⏳${NC} ${BOLD}$project${NC} ${DIM}($reason)${NC}"
    fi
}

print_success() {
    echo -e "${GREEN}✅${NC} ${BOLD}$1${NC} - compiled"
}

print_error() {
    echo ""
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${RED}❌ COMPILATION FAILED: $1${NC}"
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
}

# ═══════════════════════════════════════════════════════════════
# 🔥 v7.4: 智能清理过时二进制
# ═══════════════════════════════════════════════════════════════
clean_old_binaries() {
    echo -e "${YELLOW}🧹 Cleaning old binaries...${NC}"
    
    local cleaned=0
    
    # 查找并删除所有旧的二进制文件（不在 target/release 中的）
    for entry in "${ALL_PROJECTS[@]}"; do
        local binary_name="${entry##*:}"
        while IFS= read -r -d '' old_binary; do
            echo -e "   ${RED}🗑️  Removing: ${DIM}$old_binary${NC}"
            rm -f "$old_binary"
            cleaned=$((cleaned + 1))
        done < <(find . -name "$binary_name" -type f -not -path "*/target/*" -print0 2>/dev/null)
    done
    
    if [ $cleaned -eq 0 ]; then
        echo -e "   ${GREEN}✓${NC} ${DIM}No old binaries found${NC}"
    else
        echo -e "   ${GREEN}✅ Cleaned $cleaned old binary file(s)${NC}"
    fi
    echo ""
}

# ═══════════════════════════════════════════════════════════════
# 时间戳函数
# ═══════════════════════════════════════════════════════════════
get_newest_source_mtime() {
    local project_dir="$1"
    local newest=0
    
    # 项目源代码
    if [[ -d "$project_dir/src" ]]; then
        while IFS= read -r -d '' file; do
            local mtime
            mtime=$(stat -f %m "$file" 2>/dev/null || stat -c %Y "$file" 2>/dev/null || echo 0)
            [[ $mtime -gt $newest ]] && newest=$mtime
        done < <(find "$project_dir/src" -type f -name "*.rs" -print0 2>/dev/null)
    fi
    
    if [[ -f "$project_dir/Cargo.toml" ]]; then
        local mtime
        mtime=$(stat -f %m "$project_dir/Cargo.toml" 2>/dev/null || stat -c %Y "$project_dir/Cargo.toml" 2>/dev/null || echo 0)
        [[ $mtime -gt $newest ]] && newest=$mtime
    fi

    # shared_utils 依赖
    if [[ -d "shared_utils/src" ]]; then
        while IFS= read -r -d '' file; do
            local mtime
            mtime=$(stat -f %m "$file" 2>/dev/null || stat -c %Y "$file" 2>/dev/null || echo 0)
            [[ $mtime -gt $newest ]] && newest=$mtime
        done < <(find "shared_utils/src" -type f -name "*.rs" -print0 2>/dev/null)
    fi
    
    echo "$newest"
}

get_binary_mtime() {
    local binary_path="$1"
    [[ ! -f "$binary_path" ]] && echo "0" && return
    stat -f %m "$binary_path" 2>/dev/null || stat -c %Y "$binary_path" 2>/dev/null || echo 0
}

# ═══════════════════════════════════════════════════════════════
# 编译决策
# ═══════════════════════════════════════════════════════════════
decide_build_action() {
    local project_dir="$1"
    local binary_name="$2"
    local binary_path="$project_dir/target/release/$binary_name"
    
    [[ "$FORCE_REBUILD" == "true" ]] && echo "rebuild:force" && return
    [[ ! -f "$binary_path" ]] && echo "rebuild:binary-missing" && return
    
    local source_mtime binary_mtime
    source_mtime=$(get_newest_source_mtime "$project_dir")
    binary_mtime=$(get_binary_mtime "$binary_path")
    
    [[ $source_mtime -gt $binary_mtime ]] && echo "rebuild:source-newer" && return
    
    echo "skip"
}

# ═══════════════════════════════════════════════════════════════
# 编译函数
# ═══════════════════════════════════════════════════════════════
build_project() {
    local project_dir="$1"
    
    # 🔥 修复：正确处理 cargo 输出和返回码
    if cargo build --release --manifest-path "$project_dir/Cargo.toml"; then
        return 0
    else
        print_error "$project_dir"
        return 1
    fi
}

# ═══════════════════════════════════════════════════════════════
# CLI 参数解析
# ═══════════════════════════════════════════════════════════════
parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --force|-f)
                FORCE_REBUILD=true
                shift
                ;;
            --clean|-c)
                CLEAN_BUILD=true
                shift
                ;;
            --verbose|-v)
                VERBOSE=true
                shift
                ;;
            --no-clean-old)
                CLEAN_OLD_BINARIES=false
                shift
                ;;
            --all|-a)
                BUILD_ALL=true
                shift
                ;;
            --hevc)
                SELECTED_PROJECTS+=("imgquality_hevc" "vidquality_hevc")
                shift
                ;;
            --av1)
                SELECTED_PROJECTS+=("imgquality_av1" "vidquality_av1")
                shift
                ;;
            --img)
                SELECTED_PROJECTS+=("imgquality_hevc" "imgquality_av1")
                shift
                ;;
            --vid)
                SELECTED_PROJECTS+=("vidquality_hevc" "vidquality_av1")
                shift
                ;;
            --xmp)
                SELECTED_PROJECTS+=("xmp_merger")
                shift
                ;;
            --help|-h)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --force, -f       Force rebuild all selected projects"
                echo "  --clean, -c       Clean build artifacts before compiling"
                echo "  --verbose, -v     Show detailed output"
                echo "  --no-clean-old    Don't clean old binary files"
                echo "  --all, -a         Build all projects"
                echo "  --hevc            Build HEVC tools (default)"
                echo "  --av1             Build AV1 tools"
                echo "  --img             Build image tools"
                echo "  --vid             Build video tools"
                echo "  --xmp             Build XMP merger"
                echo "  --help, -h        Show this help"
                echo ""
                echo "Examples:"
                echo "  $0                # Build HEVC tools (default)"
                echo "  $0 --all          # Build all projects"
                echo "  $0 --hevc --force # Force rebuild HEVC tools"
                echo "  $0 --img --av1    # Build AV1 image tools"
                exit 0
                ;;
            *)
                echo -e "${RED}Unknown option: $1${NC}"
                exit 1
                ;;
        esac
    done
}

# ═══════════════════════════════════════════════════════════════
# 主函数
# ═══════════════════════════════════════════════════════════════
main() {
    parse_args "$@"
    print_header
    
    # 确定要构建的项目
    local projects_to_build=()
    if [[ "$BUILD_ALL" == "true" ]]; then
        # 构建所有项目 - 提取项目目录名
        for entry in "${ALL_PROJECTS[@]}"; do
            projects_to_build+=("${entry%%:*}")
        done
    elif [[ ${#SELECTED_PROJECTS[@]} -gt 0 ]]; then
        projects_to_build=("${SELECTED_PROJECTS[@]}")
    else
        projects_to_build=("${DEFAULT_PROJECTS[@]}")
    fi
    
    echo -e "${CYAN}📦 Building:${NC} ${BOLD}${projects_to_build[*]}${NC}"
    echo ""
    
    # 清理旧二进制
    if [[ "$CLEAN_OLD_BINARIES" == "true" ]]; then
        clean_old_binaries
    fi
    
    # 清理构建产物
    if [[ "$CLEAN_BUILD" == "true" ]]; then
        echo -e "${YELLOW}🧹 Cleaning build artifacts...${NC}"
        for proj_dir in "${projects_to_build[@]}"; do
            rm -rf "$proj_dir/target/release/deps" 2>/dev/null || true
        done
        rm -rf "shared_utils/target/release/deps" 2>/dev/null || true
        echo ""
    fi
    
    local rebuilt=0
    local skipped=0
    local failed=0
    
    for proj_dir in "${projects_to_build[@]}"; do
        local binary_name
        binary_name=$(get_binary_name "$proj_dir")
        
        if [[ -z "$binary_name" ]]; then
            echo -e "${RED}❌ Unknown project: $proj_dir${NC}"
            failed=$((failed + 1))
            continue
        fi
        
        local decision
        decision=$(decide_build_action "$proj_dir" "$binary_name")
        local action="${decision%%:*}"
        local reason="${decision##*:}"

        if [[ "$action" == "skip" ]]; then
            print_status "$proj_dir" "skip" ""
            skipped=$((skipped + 1))
        else
            print_status "$proj_dir" "rebuild" "$reason"
            if build_project "$proj_dir"; then
                print_success "$proj_dir"
                rebuilt=$((rebuilt + 1))
            else
                failed=$((failed + 1))
            fi
        fi
    done
    
    echo ""
    echo -e "${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    
    if [[ $failed -gt 0 ]]; then
        echo -e "${RED}❌ Build failed: $failed project(s)${NC}"
        exit 1
    fi
    
    if [[ $rebuilt -eq 0 ]]; then
        echo -e "${GREEN}✅ All binaries up-to-date (skipped $skipped)${NC}"
    else
        echo -e "${GREEN}✅ Built $rebuilt, skipped $skipped${NC}"
    fi
    
    # 显示二进制信息
    if [[ "$VERBOSE" == "true" ]] || [[ $rebuilt -gt 0 ]]; then
        echo ""
        echo -e "${DIM}Binary info:${NC}"
        for proj_dir in "${projects_to_build[@]}"; do
            local binary_name
            binary_name=$(get_binary_name "$proj_dir")
            if [[ -z "$binary_name" ]]; then
                continue
            fi
            local binary_path="$proj_dir/target/release/$binary_name"
            if [[ -f "$binary_path" ]]; then
                local size mtime
                size=$(ls -lh "$binary_path" | awk '{print $5}')
                mtime=$(stat -f "%Sm" -t "%Y-%m-%d %H:%M" "$binary_path" 2>/dev/null || stat -c "%y" "$binary_path" 2>/dev/null | cut -d. -f1)
                echo -e "  ${BOLD}$binary_name${NC}: $size, $mtime"
            fi
        done
    fi
}

main "$@"

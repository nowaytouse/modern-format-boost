#!/bin/bash
# Smart Build System for Modern Format Boost
# v1.0: 智能编译 - 时间戳比对 + 版本号识别
#
# 只在源代码更新时重新编译，大幅减少启动时间

set -e

# 获取脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# ═══════════════════════════════════════════════════════════════
# 颜色定义
# ═══════════════════════════════════════════════════════════════
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# ═══════════════════════════════════════════════════════════════
# 项目配置: "项目目录:二进制名称"
# ═══════════════════════════════════════════════════════════════
PROJECTS=(
    "vidquality_hevc:vidquality-hevc"
    "imgquality_hevc:imgquality-hevc"
    "vidquality_av1:vidquality-av1"
    "imgquality_av1:imgquality-av1"
    "xmp_merger:xmp-merge"
)

# 共享库目录
SHARED_UTILS_DIR="shared_utils"

# CLI 参数
FORCE_REBUILD=false
CLEAN_BUILD=false
VERBOSE=false

# ═══════════════════════════════════════════════════════════════
# 输出函数
# ═══════════════════════════════════════════════════════════════

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
    local project="$1"
    echo -e "${GREEN}✅${NC} ${BOLD}$project${NC} - compiled"
}

print_error() {
    local message="$1"
    echo ""
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${RED}❌ COMPILATION FAILED: $message${NC}"
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo ""
}

print_header() {
    echo ""
    echo -e "${CYAN}${BOLD}🔧 Smart Build System v1.0${NC}"
    echo -e "${DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
}

# ═══════════════════════════════════════════════════════════════
# 时间戳函数
# ═══════════════════════════════════════════════════════════════

# 获取目录下所有源文件的最新修改时间 (Unix timestamp)
get_newest_source_mtime() {
    local project_dir="$1"
    local newest=0
    
    # 扫描 src/ 目录
    if [[ -d "$project_dir/src" ]]; then
        while IFS= read -r -d '' file; do
            local mtime
            mtime=$(stat -f %m "$file" 2>/dev/null || stat -c %Y "$file" 2>/dev/null || echo 0)
            [[ $mtime -gt $newest ]] && newest=$mtime
        done < <(find "$project_dir/src" -type f -name "*.rs" -print0 2>/dev/null)
    fi
    
    # 检查 Cargo.toml
    if [[ -f "$project_dir/Cargo.toml" ]]; then
        local mtime
        mtime=$(stat -f %m "$project_dir/Cargo.toml" 2>/dev/null || stat -c %Y "$project_dir/Cargo.toml" 2>/dev/null || echo 0)
        [[ $mtime -gt $newest ]] && newest=$mtime
    fi

    # 包含 shared_utils (依赖传播)
    if [[ -d "$SHARED_UTILS_DIR/src" ]]; then
        while IFS= read -r -d '' file; do
            local mtime
            mtime=$(stat -f %m "$file" 2>/dev/null || stat -c %Y "$file" 2>/dev/null || echo 0)
            [[ $mtime -gt $newest ]] && newest=$mtime
        done < <(find "$SHARED_UTILS_DIR/src" -type f -name "*.rs" -print0 2>/dev/null)
    fi
    
    if [[ -f "$SHARED_UTILS_DIR/Cargo.toml" ]]; then
        local mtime
        mtime=$(stat -f %m "$SHARED_UTILS_DIR/Cargo.toml" 2>/dev/null || stat -c %Y "$SHARED_UTILS_DIR/Cargo.toml" 2>/dev/null || echo 0)
        [[ $mtime -gt $newest ]] && newest=$mtime
    fi
    
    echo "$newest"
}

# 获取二进制文件修改时间
get_binary_mtime() {
    local binary_path="$1"
    
    if [[ ! -f "$binary_path" ]]; then
        echo "0"
        return
    fi
    
    stat -f %m "$binary_path" 2>/dev/null || stat -c %Y "$binary_path" 2>/dev/null || echo 0
}

# ═══════════════════════════════════════════════════════════════
# 版本函数
# ═══════════════════════════════════════════════════════════════

# 从 Cargo.toml 获取版本号
get_cargo_version() {
    local cargo_toml="$1"
    grep -m1 '^version' "$cargo_toml" 2>/dev/null | sed 's/.*"\(.*\)".*/\1/' || echo "unknown"
}

# 从二进制获取版本号
get_binary_version() {
    local binary_path="$1"
    
    if [[ ! -x "$binary_path" ]]; then
        echo "missing"
        return
    fi
    
    # 尝试执行 --version，超时 2 秒
    local version
    version=$(timeout 2 "$binary_path" --version 2>/dev/null | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' || echo "unknown")
    echo "$version"
}

# ═══════════════════════════════════════════════════════════════
# 编译决策
# ═══════════════════════════════════════════════════════════════

# 判断是否需要重新编译
# 返回: "skip" 或 "rebuild:原因"
decide_build_action() {
    local project_dir="$1"
    local binary_name="$2"
    local binary_path="$project_dir/target/release/$binary_name"
    
    # 强制重编译
    if [[ "$FORCE_REBUILD" == "true" ]]; then
        echo "rebuild:force"
        return
    fi
    
    # 二进制不存在
    if [[ ! -f "$binary_path" ]]; then
        echo "rebuild:binary-missing"
        return
    fi
    
    # 时间戳比对
    local source_mtime binary_mtime
    source_mtime=$(get_newest_source_mtime "$project_dir")
    binary_mtime=$(get_binary_mtime "$binary_path")
    
    if [[ $source_mtime -gt $binary_mtime ]]; then
        echo "rebuild:source-newer"
        return
    fi
    
    # 版本号比对 (可选，失败时跳过)
    local cargo_version binary_version
    cargo_version=$(get_cargo_version "$project_dir/Cargo.toml")
    binary_version=$(get_binary_version "$binary_path")
    
    if [[ "$cargo_version" != "unknown" && "$binary_version" != "unknown" && "$binary_version" != "missing" ]]; then
        if [[ "$cargo_version" != "$binary_version" ]]; then
            echo "rebuild:version-mismatch"
            return
        fi
    fi
    
    echo "skip"
}

# ═══════════════════════════════════════════════════════════════
# 编译函数
# ═══════════════════════════════════════════════════════════════

build_project() {
    local project_dir="$1"
    
    if ! cargo build --release --manifest-path "$project_dir/Cargo.toml" 2>&1; then
        print_error "$project_dir"
        return 1
    fi
    
    return 0
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
            --help|-h)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --force, -f    Force rebuild all projects"
                echo "  --clean, -c    Clean build artifacts before compiling"
                echo "  --verbose, -v  Show detailed output"
                echo "  --help, -h     Show this help"
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
    
    # 清理构建产物
    if [[ "$CLEAN_BUILD" == "true" ]]; then
        echo -e "${YELLOW}🧹 Cleaning build artifacts...${NC}"
        for proj_config in "${PROJECTS[@]}"; do
            local proj_dir="${proj_config%%:*}"
            rm -rf "$proj_dir/target/release/deps" 2>/dev/null || true
        done
        rm -rf "$SHARED_UTILS_DIR/target/release/deps" 2>/dev/null || true
        echo ""
    fi
    
    local rebuilt=0
    local skipped=0
    local failed=0
    
    for proj_config in "${PROJECTS[@]}"; do
        local proj_dir="${proj_config%%:*}"
        local binary_name="${proj_config##*:}"
        
        local decision
        decision=$(decide_build_action "$proj_dir" "$binary_name")
        local action="${decision%%:*}"
        local reason="${decision##*:}"

        if [[ "$action" == "skip" ]]; then
            print_status "$proj_dir" "skip" ""
            ((skipped++))
        else
            print_status "$proj_dir" "rebuild" "$reason"
            if build_project "$proj_dir"; then
                print_success "$proj_dir"
                ((rebuilt++))
            else
                ((failed++))
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
    if [[ "$VERBOSE" == "true" ]]; then
        echo ""
        echo -e "${DIM}Binary info:${NC}"
        for proj_config in "${PROJECTS[@]}"; do
            local proj_dir="${proj_config%%:*}"
            local binary_name="${proj_config##*:}"
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

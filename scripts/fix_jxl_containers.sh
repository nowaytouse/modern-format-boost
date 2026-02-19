#!/opt/homebrew/bin/bash
# Modern Format Boost - JXL Container Fixer
# Converts JXL ISOBMFF containers to bare codestream for iCloud Photos compatibility
#
# 🔥 Features:
#    - Auto-detects container format JXL files
#    - Extracts bare codestream (no re-encoding, preserves quality)
#    - Preserves all metadata (EXIF, timestamps, xattr)
#    - In-place replacement with backup
#    - Integrated with Modern Format Boost workflow

# 🔥 安全路径处理：使用绝对路径，避免继承问题
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FIXER_TOOL="$SCRIPT_DIR/jxl_container_fixer.py"

# Color schemes
RESET='\033[0m'
BOLD='\033[1m'
DIM='\033[2m'
RED='\033[38;5;196m'
GREEN='\033[38;5;46m'
YELLOW='\033[38;5;226m'
BLUE='\033[38;5;39m'
CYAN='\033[38;5;51m'
GRAY='\033[38;5;240m'

# Ensure Python tool is executable
chmod +x "$FIXER_TOOL"

# Check if file is JXL container (not bare codestream)
is_jxl_container() {
    local file="$1"
    
    # 🔥 安全检查：确保文件存在且可读
    [[ ! -f "$file" ]] || [[ ! -r "$file" ]] && return 1
    
    local header=$(xxd -l 4 -p "$file" 2>/dev/null)
    
    # Container: 0000000c (ISOBMFF)
    if [[ "$header" == "0000000c" ]]; then
        return 0
    fi
    
    # Bare codestream: ff0a (already fixed)
    return 1
}

# Process single file
fix_jxl_file() {
    local input="$1"
    local backup_dir="$2"
    local temp_output="${input}.tmp.jxl"
    local filename=$(basename "$input")
    local backup="$backup_dir/$filename.container.backup"
    
    # 🔥 安全检查：确保文件存在且可读
    if [[ ! -f "$input" ]] || [[ ! -r "$input" ]]; then
        echo -e "   ${RED}✗ File not accessible${RESET}"
        return 1
    fi
    
    # Check if already processed (backup exists)
    if [[ -f "$backup" ]]; then
        echo -e "   ${DIM}⊘ Already processed (backup exists)${RESET}"
        return 0
    fi
    
    # 🔥 安全检查：确保有写权限
    local dir=$(dirname "$input")
    if [[ ! -w "$dir" ]]; then
        echo -e "   ${RED}✗ No write permission${RESET}"
        return 1
    fi
    
    # Extract codestream
    if ! "$FIXER_TOOL" "$input" "$temp_output" 2>/dev/null; then
        echo -e "   ${RED}✗ Extraction failed${RESET}"
        rm -f "$temp_output"
        return 1
    fi
    
    # 🔥 验证提取结果
    if [[ ! -f "$temp_output" ]] || [[ ! -s "$temp_output" ]]; then
        echo -e "   ${RED}✗ Output file invalid${RESET}"
        rm -f "$temp_output"
        return 1
    fi
    
    # Preserve metadata using shared_utils approach
    # 1. Copy EXIF metadata
    if command -v exiftool &> /dev/null; then
        exiftool -overwrite_original -TagsFromFile "$input" -all:all "$temp_output" 2>/dev/null
    fi
    
    # 2. Copy file timestamps (atime, mtime)
    touch -r "$input" "$temp_output"
    
    # 3. Copy macOS extended attributes
    if command -v xattr &> /dev/null; then
        for attr in $(xattr "$input" 2>/dev/null); do
            xattr -wx "$attr" "$(xattr -px "$attr" "$input" 2>/dev/null)" "$temp_output" 2>/dev/null
        done
    fi
    
    # 4. Copy macOS creation time and Date Added
    if command -v SetFile &> /dev/null && command -v GetFileInfo &> /dev/null; then
        local creation_date=$(GetFileInfo -d "$input" 2>/dev/null)
        if [[ -n "$creation_date" ]]; then
            SetFile -d "$creation_date" "$temp_output" 2>/dev/null
        fi
    fi
    
    # 🔥 原子替换：先复制到备份文件夹，再替换
    if ! cp -p "$input" "$backup"; then
        echo -e "   ${RED}✗ Failed to create backup${RESET}"
        rm -f "$temp_output"
        return 1
    fi
    
    if ! mv "$temp_output" "$input"; then
        echo -e "   ${RED}✗ Failed to replace file${RESET}"
        rm -f "$backup"
        rm -f "$temp_output"
        return 1
    fi
    
    # Verify
    local orig_size=$(ls -lh "$backup" | awk '{print $5}')
    local new_size=$(ls -lh "$input" | awk '{print $5}')
    
    echo -e "   ${GREEN}✓ Fixed${RESET} ${DIM}($orig_size → $new_size)${RESET}"
    return 0
}

# Process directory recursively
process_directory() {
    local dir="$1"
    local count=0
    local fixed=0
    local skipped=0
    local errors=0
    
    # 🔥 创建备份文件夹
    local backup_dir="$dir/.jxl_container_backups"
    mkdir -p "$backup_dir"
    
    # 🔥 保留备份文件夹的元数据
    touch -r "$dir" "$backup_dir" 2>/dev/null
    
    echo -e "${CYAN}🔍 Scanning for JXL container files...${RESET}"
    echo -e "${DIM}   Backup folder: $backup_dir${RESET}"
    echo ""
    
    # Find all JXL files
    while IFS= read -r -d '' file; do
        ((count++))
        
        if is_jxl_container "$file"; then
            echo -e "${YELLOW}📦 Container:${RESET} $(basename "$file")"
            if fix_jxl_file "$file" "$backup_dir"; then
                ((fixed++))
            else
                ((errors++))
            fi
        else
            ((skipped++))
        fi
    done < <(find "$dir" -type f -iname "*.jxl" ! -path "*/.jxl_container_backups/*" -print0 2>/dev/null)
    
    echo ""
    echo -e "${BOLD}Summary:${RESET}"
    echo -e "  Total JXL files: ${BOLD}$count${RESET}"
    echo -e "  Fixed containers: ${GREEN}${BOLD}$fixed${RESET}"
    echo -e "  Already codestream: ${DIM}$skipped${RESET}"
    
    if [[ $errors -gt 0 ]]; then
        echo -e "  ${RED}Errors: $errors${RESET}"
    fi
    
    if [[ $fixed -gt 0 ]]; then
        echo ""
        echo -e "${GREEN}✓ Container files converted to bare codestream${RESET}"
        echo -e "${DIM}  Backups saved in: $backup_dir${RESET}"
        echo ""
        echo -e "${YELLOW}📋 Backup Management:${RESET}"
        echo -e "   ${DIM}• Backups kept for safety (restore if needed)${RESET}"
        echo -e "   ${DIM}• To remove backup folder after verification:${RESET}"
        echo -e "     ${CYAN}rm -rf \"$backup_dir\"${RESET}"
    fi
}

# Main
main() {
    local target_dir="${1:-.}"
    
    # Header
    echo ""
    echo -e "${BLUE}╭$(printf '─%.0s' {1..60})╮${RESET}"
    echo -e "${BLUE}│${RESET}  ${BOLD}JXL Container Fixer${RESET}                                    ${BLUE}│${RESET}"
    echo -e "${BLUE}│${RESET}  ${DIM}Part of Modern Format Boost${RESET}                            ${BLUE}│${RESET}"
    echo -e "${BLUE}╰$(printf '─%.0s' {1..60})╯${RESET}"
    echo ""
    
    # Validate directory
    if [[ ! -d "$target_dir" ]]; then
        echo -e "${RED}✗ Directory not found: $target_dir${RESET}"
        exit 1
    fi
    
    echo -e "${DIM}Target: $target_dir${RESET}"
    echo ""
    
    # Process
    process_directory "$target_dir"
    
    echo ""
}

# Run if called directly
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    main "$@"
fi

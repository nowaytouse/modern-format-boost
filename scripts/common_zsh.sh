# common_zsh.sh - 目录时间戳保存/恢复（仅 zsh，依赖 typeset -A）
# 用法：在脚本开头 source "$(dirname "$0")/common_zsh.sh" 或先设 SCRIPT_DIR 再 source "$SCRIPT_DIR/common_zsh.sh"
# 调用 save_dir_timestamps "$TARGET_DIR"，执行操作后调用 restore_dir_timestamps

typeset -gA dir_mtimes
typeset -gA dir_btimes

save_dir_timestamps() {
    local target_dir="${1:?}"
    echo "🗂️  Saving directory timestamps..."
    dir_mtimes=()
    dir_btimes=()
    while IFS= read -r d; do
        local abs_d
        abs_d=$(realpath "$d")
        dir_mtimes["$abs_d"]=$(stat -f%m "$abs_d")
        dir_btimes["$abs_d"]=$(stat -f%B "$abs_d" 2>/dev/null || echo "0")
    done < <(find "$target_dir" -type d 2>/dev/null)
}

restore_dir_timestamps() {
    echo "🗂️  Restoring directory timestamps..."
    local keys=("${(@k)dir_mtimes}")
    local d m b
    for d in ${(f)"$(printf '%s\n' "${keys[@]}" | awk '{ print length, $0 }' | sort -rn | cut -d' ' -f2-)"}; do
        [[ -z "$d" ]] && continue
        m="${dir_mtimes[$d]}"
        b="${dir_btimes[$d]}"
        if [[ -d "$d" ]]; then
            touch -mt "$(date -r "$m" +%Y%m%d%H%M.%S)" "$d" 2>/dev/null || true
            [[ "$b" != "0" ]] && SetFile -d "$(date -r "$b" +%m/%d/%Y\ %H:%M:%S)" "$d" 2>/dev/null || true
        fi
    done
}

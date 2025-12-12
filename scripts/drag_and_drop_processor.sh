#!/opt/homebrew/bin/bash
# Modern Format Boost - Drag & Drop Processor
# 拖拽式一键处理脚本
# 
# 使用方法：将文件夹拖拽到此脚本上，或双击后选择文件夹
# Usage: Drag folder to this script, or double-click and select folder
#
# 🔥 v4.3: 增强测试模式
#   - 随机采样：每次运行选择不同的文件组合
#   - 多样性覆盖：每种格式最多2个文件（特殊+普通）
#   - 文件大小采样：包含小文件和大文件边缘案例
#   - 最多采样20个文件，覆盖更多场景
#   - 断点续传 + 原子操作保护

set -e

# 🔥 测试模式相关
TEST_MODE=false
TEST_OUTPUT_DIR=""
TEST_LOG_FILE=""

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
    echo "🚀 Modern Format Boost - 一键处理器 v4.3"
    echo "=================================================="
    if [[ "$TEST_MODE" == "true" ]]; then
        echo "🧪 【测试模式】安全预览，不修改原文件"
        echo "📁 输出目录：临时目录"
    else
        echo "📁 处理模式：原地转换（删除原文件）"
    fi
    echo "📋 XMP合并：自动检测并合并 sidecar 元数据"
    echo "🍎 Apple兼容：默认启用（AV1/VP9 → HEVC）"
    echo "🔄 断点续传：支持中断后继续处理"
    echo "=================================================="
    echo ""
}

# 🔥 选择运行模式
select_mode() {
    echo ""
    echo "请选择运行模式："
    echo "  [1] 🧪 测试模式 - 安全预览，输出到临时目录（推荐首次使用）"
    echo "  [2] 🚀 正式模式 - 原地转换，删除原文件"
    echo "  [Q] 退出"
    echo ""
    read -r MODE_CHOICE
    
    case "$MODE_CHOICE" in
        1)
            TEST_MODE=true
            echo "✅ 已选择测试模式"
            ;;
        2)
            TEST_MODE=false
            echo "✅ 已选择正式模式"
            ;;
        *)
            echo "❌ 用户取消"
            exit 0
            ;;
    esac
}

# 🔥 初始化测试模式
init_test_mode() {
    if [[ "$TEST_MODE" != "true" ]]; then
        return
    fi
    
    # 创建临时输出目录
    TEST_OUTPUT_DIR=$(mktemp -d -t "mfb_test_XXXXXX")
    TEST_LOG_FILE="$TEST_OUTPUT_DIR/test_log.txt"
    
    echo ""
    echo "🧪 测试模式初始化"
    echo "=================================================="
    echo "📂 临时输出目录: $TEST_OUTPUT_DIR"
    echo "📋 日志文件: $TEST_LOG_FILE"
    echo ""
    
    # 初始化日志
    {
        echo "========================================"
        echo "Modern Format Boost - 测试模式日志"
        echo "========================================"
        echo "时间: $(date '+%Y-%m-%d %H:%M:%S')"
        echo "源目录: $TARGET_DIR"
        echo "输出目录: $TEST_OUTPUT_DIR"
        echo "========================================"
        echo ""
    } > "$TEST_LOG_FILE"
}

# 🔥 测试日志记录
test_log() {
    local message="$1"
    if [[ "$TEST_MODE" == "true" ]] && [[ -n "$TEST_LOG_FILE" ]]; then
        echo "[$(date '+%H:%M:%S')] $message" >> "$TEST_LOG_FILE"
    fi
    echo "$message"
}

# 🔥 检查文件名是否包含特殊字符（边缘案例）
has_special_chars() {
    local name="$1"
    # 检查: 方括号、圆括号、空格、中文、日文、引号等
    [[ "$name" == *"["* ]] || [[ "$name" == *"("* ]] || [[ "$name" == *" "* ]] || \
    [[ "$name" == *"【"* ]] || [[ "$name" == *"（"* ]] || [[ "$name" =~ [一-龥] ]] || \
    [[ "$name" == *"'"* ]] || [[ "$name" == *'"'* ]] || [[ "$name" == *'&'* ]]
}

# 🔥 查找 XMP 对应的媒体文件
find_xmp_media() {
    local xmp_file="$1"
    local dir=$(dirname "$xmp_file")
    local stem=$(basename "${xmp_file%.xmp}")
    
    for ext in mp4 mov mkv gif png jpg jpeg webp avif heic tiff bmp; do
        local candidate="$dir/$stem.$ext"
        if [[ -f "$candidate" ]]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

# 🔥 随机选择文件（从数组中随机选一个）
random_pick() {
    local -n arr=$1
    local count=${#arr[@]}
    if [[ $count -eq 0 ]]; then
        return 1
    fi
    local idx=$((RANDOM % count))
    echo "${arr[$idx]}"
}

# 🔥 采样文件（v4.3: 随机采样 + 多样性覆盖）
sample_files() {
    local sample_dir="$TEST_OUTPUT_DIR/samples"
    mkdir -p "$sample_dir"
    
    # 初始化随机种子（基于时间）
    RANDOM=$$$(date +%s)
    
    test_log ""
    test_log "📊 采样文件用于测试（随机采样 v4.3）..."
    test_log "=================================================="
    
    local sample_count=0
    local max_samples=20  # 增加采样数量
    
    # ========== 1. 采样 XMP 文件（随机选择，最多3个）==========
    test_log ""
    test_log "📋 XMP 文件采样:"
    
    local xmp_special=()
    local xmp_normal=()
    
    while IFS= read -r -d '' xmp_file; do
        local media_file=$(find_xmp_media "$xmp_file")
        [[ -z "$media_file" ]] && continue
        
        local fname=$(basename "$xmp_file")
        if has_special_chars "$fname"; then
            xmp_special+=("$xmp_file")
        else
            xmp_normal+=("$xmp_file")
        fi
    done < <(find "$TARGET_DIR" -type f -iname "*.xmp" -print0 2>/dev/null)
    
    # 随机选择 XMP（特殊字符优先，最多3个）
    local xmp_picked=0
    for _ in 1 2; do
        if [[ ${#xmp_special[@]} -gt 0 ]] && [[ $xmp_picked -lt 3 ]] && [[ $sample_count -lt $max_samples ]]; then
            local pick=$(random_pick xmp_special)
            local media=$(find_xmp_media "$pick")
            cp "$pick" "$sample_dir/"
            cp "$media" "$sample_dir/"
            test_log "   ✓ XMP(特殊): $(basename "$pick")"
            test_log "      └─ 媒体: $(basename "$media")"
            ((sample_count++))
            ((xmp_picked++))
            # 从数组中移除已选择的
            xmp_special=("${xmp_special[@]/$pick}")
        fi
    done
    if [[ ${#xmp_normal[@]} -gt 0 ]] && [[ $xmp_picked -lt 3 ]] && [[ $sample_count -lt $max_samples ]]; then
        local pick=$(random_pick xmp_normal)
        local media=$(find_xmp_media "$pick")
        cp "$pick" "$sample_dir/"
        cp "$media" "$sample_dir/"
        test_log "   ✓ XMP(普通): $(basename "$pick")"
        test_log "      └─ 媒体: $(basename "$media")"
        ((sample_count++))
    fi
    
    # ========== 2. 采样图像文件（每种格式随机选1-2个）==========
    test_log ""
    test_log "🖼️  图像文件采样:"
    
    for ext in jpg jpeg png gif webp heic avif bmp tiff jxl; do
        [[ $sample_count -ge $max_samples ]] && break
        
        local special_files=()
        local normal_files=()
        local check_count=0
        
        while IFS= read -r -d '' img_file; do
            ((check_count++))
            [[ $check_count -gt 50 ]] && break  # 扩大搜索范围
            
            local fname=$(basename "$img_file")
            if has_special_chars "$fname"; then
                special_files+=("$img_file")
            else
                normal_files+=("$img_file")
            fi
        done < <(find "$TARGET_DIR" -type f -iname "*.$ext" -print0 2>/dev/null | sort -R 2>/dev/null || cat)
        
        # 随机选择（优先特殊字符）
        local picked=0
        if [[ ${#special_files[@]} -gt 0 ]]; then
            local pick=$(random_pick special_files)
            cp "$pick" "$sample_dir/"
            test_log "   ✓ $ext(特殊): $(basename "$pick")"
            ((sample_count++))
            ((picked++))
        fi
        
        # 如果还有配额，再选一个普通文件
        if [[ ${#normal_files[@]} -gt 0 ]] && [[ $picked -lt 2 ]] && [[ $sample_count -lt $max_samples ]]; then
            local pick=$(random_pick normal_files)
            cp "$pick" "$sample_dir/"
            test_log "   ✓ $ext: $(basename "$pick")"
            ((sample_count++))
        fi
    done
    
    # ========== 3. 采样视频文件（每种格式随机选1-2个）==========
    test_log ""
    test_log "🎬 视频文件采样:"
    
    for ext in mp4 mov mkv webm avi m4v flv; do
        [[ $sample_count -ge $max_samples ]] && break
        
        local special_files=()
        local normal_files=()
        local check_count=0
        
        while IFS= read -r -d '' vid_file; do
            ((check_count++))
            [[ $check_count -gt 50 ]] && break
            
            local fname=$(basename "$vid_file")
            if has_special_chars "$fname"; then
                special_files+=("$vid_file")
            else
                normal_files+=("$vid_file")
            fi
        done < <(find "$TARGET_DIR" -type f -iname "*.$ext" -print0 2>/dev/null | sort -R 2>/dev/null || cat)
        
        local picked=0
        if [[ ${#special_files[@]} -gt 0 ]]; then
            local pick=$(random_pick special_files)
            cp "$pick" "$sample_dir/"
            test_log "   ✓ $ext(特殊): $(basename "$pick")"
            ((sample_count++))
            ((picked++))
        fi
        
        if [[ ${#normal_files[@]} -gt 0 ]] && [[ $picked -lt 2 ]] && [[ $sample_count -lt $max_samples ]]; then
            local pick=$(random_pick normal_files)
            cp "$pick" "$sample_dir/"
            test_log "   ✓ $ext: $(basename "$pick")"
            ((sample_count++))
        fi
    done
    
    # ========== 4. 额外采样：不同文件大小的文件 ==========
    test_log ""
    test_log "📏 文件大小多样性采样:"
    
    # 找一个小文件（<100KB）和一个大文件（>5MB）
    local small_file=$(find "$TARGET_DIR" -type f \( -iname "*.jpg" -o -iname "*.png" -o -iname "*.gif" -o -iname "*.webp" \) -size -100k 2>/dev/null | head -1)
    local large_file=$(find "$TARGET_DIR" -type f \( -iname "*.jpg" -o -iname "*.png" -o -iname "*.mp4" -o -iname "*.mov" \) -size +5M 2>/dev/null | head -1)
    
    if [[ -n "$small_file" ]] && [[ $sample_count -lt $max_samples ]]; then
        local fname=$(basename "$small_file")
        # 检查是否已采样
        if [[ ! -f "$sample_dir/$fname" ]]; then
            cp "$small_file" "$sample_dir/"
            test_log "   ✓ 小文件(<100KB): $fname"
            ((sample_count++))
        fi
    fi
    
    if [[ -n "$large_file" ]] && [[ $sample_count -lt $max_samples ]]; then
        local fname=$(basename "$large_file")
        if [[ ! -f "$sample_dir/$fname" ]]; then
            cp "$large_file" "$sample_dir/"
            test_log "   ✓ 大文件(>5MB): $fname"
            ((sample_count++))
        fi
    fi
    
    test_log ""
    test_log "📊 采样完成: $sample_count 个文件"
    test_log ""
    
    if [[ $sample_count -eq 0 ]]; then
        test_log "⚠️  未找到可采样的文件！"
        return 1
    fi
    
    # 更新 TARGET_DIR 为采样目录
    TARGET_DIR="$sample_dir"
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
    # 测试模式跳过危险目录检查（因为不会修改原文件）
    if [[ "$TEST_MODE" != "true" ]]; then
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
    else
        echo "🧪 测试模式：跳过安全确认（不会修改原文件）"
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
                test_log "   ⏭️  跳过: $(basename "$xmp_file") (无对应媒体文件)"
                mark_file_completed "xmp:$xmp_file"
                ((XMP_SKIPPED++)) || true
                continue
            fi
        fi
        
        # 执行合并
        test_log "   🔄 合并: $(basename "$xmp_file") → $(basename "$media_file")"
        
        # 🔥 创建临时文件保存媒体文件的原始时间戳（在 exiftool 修改前）
        timestamp_ref=$(mktemp)
        touch -r "$media_file" "$timestamp_ref" 2>/dev/null || true
        
        if exiftool -P -overwrite_original -tagsfromfile "$xmp_file" -all:all "$media_file" > /dev/null 2>&1; then
            # 🔥 恢复媒体文件的原始时间戳（exiftool 会修改时间戳）
            touch -r "$timestamp_ref" "$media_file" 2>/dev/null || true
            rm -f "$timestamp_ref"
            
            # 删除 XMP 文件
            rm "$xmp_file"
            test_log "      ✅ 成功，已删除 XMP 文件"
            mark_file_completed "xmp:$xmp_file"
            ((XMP_SUCCESS++)) || true
        else
            rm -f "$timestamp_ref"
            test_log "      ❌ 合并失败"
            ((XMP_FAILED++)) || true
        fi
        
    done < <(find "$TARGET_DIR" -type f -iname "*.xmp" -print0 2>/dev/null)
    
    echo ""
    echo "📋 XMP 合并完成: ✅ $XMP_SUCCESS 成功, ❌ $XMP_FAILED 失败, ⏭️ $XMP_SKIPPED 跳过"
}

# 处理图像文件
process_images() {
    if [[ $IMG_COUNT -gt 0 ]]; then
        test_log ""
        test_log "🖼️  开始处理图像文件..."
        test_log "=================================================="
        
        if [[ "$TEST_MODE" == "true" ]]; then
            # 测试模式：记录详细输出
            test_log "命令: imgquality-hevc auto $TARGET_DIR --in-place --recursive --match-quality --explore --apple-compat"
            "$IMGQUALITY_HEVC" auto "$TARGET_DIR" \
                --in-place \
                --recursive \
                --match-quality \
                --explore \
                --apple-compat 2>&1 | tee -a "$TEST_LOG_FILE"
        else
            "$IMGQUALITY_HEVC" auto "$TARGET_DIR" \
                --in-place \
                --recursive \
                --match-quality \
                --explore \
                --apple-compat
        fi
        
        test_log "✅ 图像处理完成"
    fi
}

# 处理视频文件
process_videos() {
    if [[ $VID_COUNT -gt 0 ]]; then
        test_log ""
        test_log "🎬 开始处理视频文件..."
        test_log "=================================================="
        
        if [[ "$TEST_MODE" == "true" ]]; then
            # 测试模式：记录详细输出
            test_log "命令: vidquality-hevc auto $TARGET_DIR --in-place --recursive --match-quality true --explore --apple-compat"
            "$VIDQUALITY_HEVC" auto "$TARGET_DIR" \
                --in-place \
                --recursive \
                --match-quality true \
                --explore \
                --apple-compat 2>&1 | tee -a "$TEST_LOG_FILE"
        else
            "$VIDQUALITY_HEVC" auto "$TARGET_DIR" \
                --in-place \
                --recursive \
                --match-quality true \
                --explore \
                --apple-compat
        fi
        
        test_log "✅ 视频处理完成"
    fi
}

# 显示完成信息
show_completion() {
    echo ""
    echo "🎉 处理完成！"
    echo "=================================================="
    
    if [[ "$TEST_MODE" == "true" ]]; then
        echo "🧪 【测试模式】结果"
        echo "📂 输出目录: $TEST_OUTPUT_DIR"
        echo "📋 日志文件: $TEST_LOG_FILE"
        echo ""
        
        # 显示输出目录内容
        echo "📁 输出文件列表:"
        ls -la "$TEST_OUTPUT_DIR/samples/" 2>/dev/null || echo "   (无文件)"
        echo ""
        
        # 记录最终统计到日志
        {
            echo ""
            echo "========================================"
            echo "测试完成统计"
            echo "========================================"
            echo "XMP合并: ✅ $XMP_SUCCESS 成功, ❌ $XMP_FAILED 失败, ⏭️ $XMP_SKIPPED 跳过"
            echo "图像文件: $IMG_COUNT"
            echo "视频文件: $VID_COUNT"
            echo "完成时间: $(date '+%Y-%m-%d %H:%M:%S')"
            echo "========================================"
        } >> "$TEST_LOG_FILE"
        
        echo "💡 提示: 检查输出目录确认转换效果"
        echo "   如果测试通过，可以使用正式模式处理"
        echo ""
        echo "是否打开输出目录？(y/N): "
        read -r OPEN_DIR
        if [[ "$OPEN_DIR" =~ ^[Yy]$ ]]; then
            open "$TEST_OUTPUT_DIR" 2>/dev/null || echo "无法打开目录"
        fi
    else
        echo "📁 处理目录: $TARGET_DIR"
        if [[ $XMP_COUNT -gt 0 ]]; then
            echo "📋 XMP合并:  ✅ $XMP_SUCCESS 成功"
        fi
        echo "🖼️  图像文件: $IMG_COUNT"
        echo "🎬 视频文件: $VID_COUNT"
    fi
    
    echo "=================================================="
    echo ""
    echo "按任意键退出..."
    read -n 1
}

# 主函数
main() {
    check_tools
    get_target_directory "$@"
    
    # 🔥 选择运行模式（在获取目录后）
    select_mode
    
    # 🔥 测试模式：初始化并采样文件（必须在 select_mode 之后）
    if [[ "$TEST_MODE" == "true" ]]; then
        init_test_mode
        sample_files || exit 1
    fi
    
    show_welcome
    
    # 🔥 初始化断点续传（在 safety_check 之前，以便检测未完成任务）
    init_progress_tracking
    
    safety_check
    count_files
    merge_xmp_files  # 🔥 先合并 XMP 元数据
    process_images
    process_videos
    
    # 🔥 任务完成，清理进度文件（测试模式不清理，保留日志）
    if [[ "$TEST_MODE" != "true" ]]; then
        cleanup_progress
    fi
    
    show_completion
}

# 🔥 错误和中断处理
trap 'handle_interrupt' INT TERM
trap 'echo "❌ 处理过程中发生错误，进度已保存"; rm -f "$LOCK_FILE"; read -n 1' ERR

# 运行主函数
main "$@"
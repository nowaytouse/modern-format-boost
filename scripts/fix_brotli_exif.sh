#!/bin/zsh
# Fix corrupted Brotli EXIF data in JXL files
# 修复 JXL 文件中损坏的 Brotli EXIF 数据

set -euo pipefail

TARGET_DIR="${1:-.}"
BACKUP_DIR="$TARGET_DIR/.brotli_exif_backups"

echo "╔════════════════════════════════════════════════════════════════╗"
echo "║          JXL Brotli EXIF Repair Tool                          ║"
echo "╚════════════════════════════════════════════════════════════════╝"
echo ""
echo "Target: $TARGET_DIR"
echo "Backup: $BACKUP_DIR"
echo ""

mkdir -p "$BACKUP_DIR"

total=0
fixed=0
failed=0

echo "🔍 Scanning for corrupted files..."
echo ""

echo "🗂️  Saving directory timestamps to prevent metadata loss..."
typeset -A dir_mtimes
typeset -A dir_btimes
while IFS= read -r d; do
    abs_d=$(realpath "$d")
    dir_mtimes["$abs_d"]=$(stat -f%m "$abs_d")
    dir_btimes["$abs_d"]=$(stat -f%B "$abs_d" 2>/dev/null || echo "0")
done < <(find "$TARGET_DIR" -type d 2>/dev/null)


# Use a more reliable file iteration method with process substitution
# to ensure the variables total, fixed, failed are updated in the current shell
while IFS= read -r file; do
    if exiftool -validate -warning "$file" 2>&1 | grep -q "Corrupted Brotli"; then
        total=$((total+1))
        filename=$(basename "$file")
        echo "📦 $filename"
        
        # Backup
        cp -p "$file" "$BACKUP_DIR/$filename.backup"
        
        # Save timestamps
        mtime=$(stat -f%m "$file")
        btime=$(stat -f%B "$file" 2>/dev/null || echo "0")
        
        # Rebuild metadata while preserving MAXIMUM original data (ExifTool FAQ #20)
        # -all= clears everything, then we restore standard tags (-all:all),
        # plus proprietary/unsafe tags (-unsafe) and color profiles (-icc_profile)
        # This standardizes the metadata format and removes the Brotli corruption
        if exiftool -all= -tagsfromfile @ -all:all -unsafe -icc_profile -overwrite_original "$file" 2>/dev/null; then
            backup="$BACKUP_DIR/$filename.backup"
            
            # Restore xattr
            for attr in com.apple.metadata:kMDItemWhereFroms com.apple.metadata:_kMDItemUserTags com.apple.FinderInfo com.apple.metadata:kMDItemDateAdded; do
                val=$(xattr -px "$attr" "$backup" 2>/dev/null || echo "")
                [[ -n "$val" ]] && xattr -wx "$attr" "$val" "$file" 2>/dev/null || true
            done
            
            # Restore timestamps (CRITICAL: must be after exiftool to prevent overwrite)
            touch -mt "$(date -r "$mtime" +%Y%m%d%H%M.%S)" "$file" 2>/dev/null || true
            [[ "$btime" != "0" ]] && SetFile -d "$(date -r "$btime" +%m/%d/%Y\ %H:%M:%S)" "$file" 2>/dev/null || true
            
            # Verify
            if exiftool -validate -warning "$file" 2>&1 | grep -q "Corrupted Brotli"; then
                echo "   ❌ Failed, restored backup"
                cp -p "$backup" "$file"
                failed=$((failed+1))
            else
                echo "   ✓ Fixed"
                fixed=$((fixed+1))
            fi
        else
            echo "   ❌ exiftool failed"
            failed=$((failed+1))
        fi
        echo ""
    fi
done < <(find "$TARGET_DIR" -type f -iname "*.jxl" ! -path "*/.brotli_exif_backups/*" ! -path "*/.jxl_container_backups/*" 2>/dev/null)

echo "🗂️  Restoring directory timestamps..."
# Use an array to store keys and sort them by length descending (deepest directories first)
keys=("${(@k)dir_mtimes}")
for d in ${(f)"$(printf '%s\n' "${keys[@]}" | awk '{ print length, $0 }' | sort -rn | cut -d' ' -f2-)"}; do
    [[ -z "$d" ]] && continue
    m="${dir_mtimes[$d]}"
    b="${dir_btimes[$d]}"
    if [[ -d "$d" ]]; then
        touch -mt "$(date -r "$m" +%Y%m%d%H%M.%S)" "$d" 2>/dev/null || true
        [[ "$b" != "0" ]] && SetFile -d "$(date -r "$b" +%m/%d/%Y\ %H:%M:%S)" "$d" 2>/dev/null || true
    fi
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "  Detected: $total files"
echo "  Fixed: $fixed files"
echo "  Failed: $failed files"
echo ""
[[ $fixed -gt 0 ]] && echo "✅ Fixed files should now import to iCloud Photos"

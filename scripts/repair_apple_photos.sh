#!/bin/zsh
# Apple Photos Compatibility & Repair Tool (Unified)
# 苹果相册兼容性修复工具 (原地处理 + 隐藏备份模式)
#
# Merges functionality from:
# 1. repair_apple_photos.sh (Extension fixing, EOI repair)
# 2. fix_brotli_exif.sh (Brotli fix, hidden backups, in-place edit)

set -euo pipefail

TARGET_DIR="${1:-.}"
BACKUP_DIR="$TARGET_DIR/.apple_photos_repair_backups"

# Ensure we have required tools
if ! command -v exiftool &> /dev/null; then
    echo "❌ Error: exiftool is required. Please install it (brew install exiftool)."
    exit 1
fi

echo "╔════════════════════════════════════════════════════════════════╗"
echo "║          Apple Photos Ultimate Repair Tool 🍎                  ║"
echo "║          (In-Place Fix + Safe Hidden Backup)                   ║"
echo "╚════════════════════════════════════════════════════════════════╝"
echo ""
echo "Target: $TARGET_DIR"
echo "Backup: $BACKUP_DIR"
echo ""

mkdir -p "$BACKUP_DIR"

# 1. Save directory timestamps (to restore later)
echo "🗂️  Saving directory timestamps..."
typeset -A dir_mtimes
typeset -A dir_btimes
while IFS= read -r d; do
    abs_d=$(realpath "$d")
    dir_mtimes["$abs_d"]=$(stat -f%m "$abs_d")
    dir_btimes["$abs_d"]=$(stat -f%B "$abs_d" 2>/dev/null || echo "0")
done < <(find "$TARGET_DIR" -type d 2>/dev/null)

total=0
fixed_ext=0
fixed_meta=0
failed=0

echo "🔍 Scanning and repairing files..."
echo ""

# Find all files, excluding backup dir and hidden files
# Using process substitution to keep variables in scope
while IFS= read -r file; do
    # Skip if file is in backup dir
    if [[ "$file" == *"$BACKUP_DIR"* ]]; then continue; fi
    # Skip hidden files
    if [[ "$(basename "$file")" == .* ]]; then continue; fi

    # Basic info
    filename=$(basename "$file")
    
    # Calculate relative directory path from TARGET_DIR
    # Use realpath to ensure we get correct relative path
    abs_file=$(realpath "$file")
    abs_target=$(realpath "$TARGET_DIR")
    rel_path="${abs_file#$abs_target/}"
    rel_dir=$(dirname "$rel_path")
    
    # 1. Identification
    # Get current extension
    ext="${filename##*.}"
    ext=$(echo "$ext" | tr '[:upper:]' '[:lower:]')
    
    # Get real format via magic bytes
    # -s -S -FileTypeExtension outputs just the extension (e.g. "jpg")
    real_ext=$(exiftool -s -S -FileTypeExtension "$file" 2>/dev/null | tr '[:upper:]' '[:lower:]' || echo "")
    
    if [[ -z "$real_ext" ]]; then continue; fi
    
    total=$((total + 1))
    needs_repair=0
    reason=""
    is_mismatch=0
    check_meta=0

    # Check 1: Extension Mismatch
    if [[ "$ext" != "$real_ext" ]]; then
        # Allow jpg <-> jpeg
        if [[ ("$ext" == "jpg" && "$real_ext" == "jpeg") || ("$ext" == "jpeg" && "$real_ext" == "jpg") ]]; then
            is_mismatch=0
        else
            is_mismatch=1
            needs_repair=1
            reason+="[Bad Extension: .$ext -> .$real_ext] "
        fi
    fi

    # Check 2: Metadata Corruption / "Nuclear Rebuild" Candidates
    if [[ "$real_ext" == "jxl" || "$real_ext" == "webp" || "$real_ext" == "jpg" || "$real_ext" == "jpeg" ]]; then
        # 🔥 v8.2: Always enable metadata rebuild for these formats.
        # "Nuclear Rebuild" means we sanitize everything to ensure Apple Photos compatibility,
        # even if the file looks "valid" to ExifTool.
        check_meta=1
        needs_repair=1
        
        # Check for specific structural issues to trigger pre-emptive heavy repair (magick)
        warnings=$(exiftool -validate -warning "$file" 2>&1 || true)
        if echo "$warnings" | grep -q -E "JPEG EOI marker not found|JPEG format error|Corrupted Brotli"; then
             reason+="[Structure/Format Error] "
        else
             reason+="[Deep Clean] "
        fi
        
        if [[ "$is_mismatch" -eq 1 ]]; then
             reason+="[Extension Mismatch] "
        fi
    fi

    if [[ $needs_repair -eq 1 ]]; then
        echo "🔧 Fixing: $filename"
        echo "   Reason: $reason"
        
        # Prepare Backup Path
        # Structure: BACKUP_DIR/rel_dir/filename
        if [[ "$rel_dir" == "." ]]; then
            backup_subdir="$BACKUP_DIR"
        else
            backup_subdir="$BACKUP_DIR/$rel_dir"
        fi
        
        mkdir -p "$backup_subdir"
        backup_file="$backup_subdir/$filename"
        
        # Copy to backup (preserving attributes)
        cp -p "$file" "$backup_file"
        
        # Save original timestamps from the file itself
        mtime=$(stat -f%m "$file")
        btime=$(stat -f%B "$file" 2>/dev/null || echo "0")

        current_file="$file"

        # Step A: Fix Extension (Renaming)
        if [[ $is_mismatch -eq 1 ]]; then
            new_filename="${filename%.*}.$real_ext"
            new_file_path="$(dirname "$file")/$new_filename"
            
            # Rename the file
            mv "$file" "$new_file_path"
            current_file="$new_file_path"
            
            echo "   📝 Renamed to: $new_filename"
            fixed_ext=$((fixed_ext + 1))
        fi

        # Step B: Nuclear Metadata Rebuild
        if [[ $check_meta -eq 1 ]]; then
             # Attempt to fix structure first using ImageMagick if EOI missing or format error
             # (Only if it's a JPEG)
             if [[ "$real_ext" == "jpg" || "$real_ext" == "jpeg" ]]; then
                 if echo "$warnings" | grep -q -E "JPEG EOI marker not found|JPEG format error"; then
                     if command -v magick &> /dev/null; then
                         echo "   🧱 Structure broken, rebuilding with ImageMagick..."
                         magick "$current_file" "$current_file" 2>/dev/null || true
                     fi
                 fi
             fi
             
             # ExifTool Rebuild
             # -all= : Delete all tags
             # -tagsfromfile @ -all:all : Restore standard tags from source
             # -unsafe : Restore unsafe tags (needed for some formats)
             # -icc_profile : Keep color profile
             # -overwrite_original : In-place
             
             if exiftool -quiet -all= -tagsfromfile @ -all:all -unsafe -icc_profile -overwrite_original "$current_file" 2>/dev/null; then
                 echo "   ✨ Metadata Rebuilt"
                 fixed_meta=$((fixed_meta + 1))
             else
                 # Fallback: ExifTool failed. It might be severe structural damage.
                 # If it's a JPEG, try ImageMagick (if we haven't already, or even if we have) to force rewrite
                 if [[ "$real_ext" == "jpg" || "$real_ext" == "jpeg" ]] && command -v magick &> /dev/null; then
                      echo "   ⚠️ ExifTool failed. Attempting forced structural repair with ImageMagick..."
                      magick "$current_file" "$current_file" 2>/dev/null || true
                      
                      # Retry ExifTool
                      if exiftool -quiet -all= -tagsfromfile @ -all:all -unsafe -icc_profile -overwrite_original "$current_file" 2>/dev/null; then
                          echo "   ✨ Metadata Rebuilt (after structural repair)"
                          fixed_meta=$((fixed_meta + 1))
                      else
                          echo "   ❌ Failed to rebuild metadata (check backup)"
                          failed=$((failed + 1))
                      fi
                 else
                      echo "   ❌ ExifTool failed (check backup)"
                      failed=$((failed + 1))
                 fi
             fi
        fi
        
        # Step C: Restore Timestamps & Attributes
        # 1. Restore xattrs from backup
        for attr in com.apple.metadata:kMDItemWhereFroms com.apple.metadata:_kMDItemUserTags com.apple.FinderInfo com.apple.metadata:kMDItemDateAdded; do
            val=$(xattr -px "$attr" "$backup_file" 2>/dev/null || echo "")
            [[ -n "$val" ]] && xattr -wx "$attr" "$val" "$current_file" 2>/dev/null || true
        done
        
        # 2. Restore file times
        touch -mt "$(date -r "$mtime" +%Y%m%d%H%M.%S)" "$current_file" 2>/dev/null || true
        [[ "$btime" != "0" ]] && SetFile -d "$(date -r "$btime" +%m/%d/%Y\ %H:%M:%S)" "$current_file" 2>/dev/null || true
        
        echo "   ✅ Done"
        echo ""
    fi

done < <(find "$TARGET_DIR" -type f 2>/dev/null)

echo "🗂️  Restoring directory timestamps..."
# Use an array to store keys and sort them by length descending (deepest directories first)
keys=("${(@k)dir_mtimes}")
# Sort by string length descending
for d in ${(f)"$(printf '%s\n' "${keys[@]}" | awk '{ print length, $0 }' | sort -rn | cut -d' ' -f2-)"}; do
    [[ -z "$d" ]] && continue
    m="${dir_mtimes[$d]}"
    b="${dir_btimes[$d]}"
    
    if [[ -d "$d" ]]; then
        # Restore timestamps
        touch -mt "$(date -r "$m" +%Y%m%d%H%M.%S)" "$d" 2>/dev/null || true
        [[ "$b" != "0" ]] && SetFile -d "$(date -r "$b" +%m/%d/%Y\ %H:%M:%S)" "$d" 2>/dev/null || true
    fi
done

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 Summary"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Total Scanned: $total"
echo "  Extensions Fixed: $fixed_ext"
echo "  Metadata Rebuilt: $fixed_meta"
echo ""
echo "✅ Repairs complete."
echo "📦 Originals backed up in: $BACKUP_DIR"

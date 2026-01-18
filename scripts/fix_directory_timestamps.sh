#!/usr/bin/env bash
# 修复输出目录时间戳
# 用法: ./fix_directory_timestamps.sh <source_dir> <output_dir>

set -e

if [ $# -ne 2 ]; then
    echo "Usage: $0 <source_dir> <output_dir>"
    exit 1
fi

SRC="$1"
DST="$2"

if [ ! -d "$SRC" ]; then
    echo "Error: Source directory not found: $SRC"
    exit 1
fi

if [ ! -d "$DST" ]; then
    echo "Error: Output directory not found: $DST"
    exit 1
fi

echo "🔧 Fixing directory timestamps..."
echo "   Source: $SRC"
echo "   Output: $DST"

# 复制根目录时间戳
touch -r "$SRC" "$DST"

# 递归复制所有子目录时间戳
find "$SRC" -type d | while read -r src_dir; do
    rel_path="${src_dir#$SRC}"
    rel_path="${rel_path#/}"
    
    if [ -n "$rel_path" ]; then
        dst_dir="$DST/$rel_path"
        if [ -d "$dst_dir" ]; then
            touch -r "$src_dir" "$dst_dir"
        fi
    fi
done

echo "✅ Directory timestamps fixed"

#!/bin/bash
# 🔥 v7.3.3: Smart Build - 智能构建并验证版本
# 
# 功能：
# - 检测代码变更，自动重新编译
# - 验证二进制文件时间戳
# - 更新双击脚本路径
# - 防止使用旧版本

set -e

PROJECT_ROOT="/Users/user/Downloads/GitHub/modern_format_boost"
cd "$PROJECT_ROOT"

echo "🔨 Smart Build v7.3.3"
echo "===================="

# 1. 检查源代码最新修改时间
echo ""
echo "📅 Checking source code timestamps..."
LATEST_SRC=$(find . -name "*.rs" -type f -not -path "*/target/*" -exec stat -f "%m %N" {} \; | sort -rn | head -1)
LATEST_SRC_TIME=$(echo "$LATEST_SRC" | awk '{print $1}')
LATEST_SRC_FILE=$(echo "$LATEST_SRC" | cut -d' ' -f2-)

echo "   Latest source: $LATEST_SRC_FILE"
echo "   Modified: $(date -r $LATEST_SRC_TIME '+%Y-%m-%d %H:%M:%S')"

# 2. 检查现有二进制文件时间
BINARY_PATH="$PROJECT_ROOT/target/release/imgquality-hevc"
NEED_BUILD=false

if [ -f "$BINARY_PATH" ]; then
    BINARY_TIME=$(stat -f "%m" "$BINARY_PATH")
    echo ""
    echo "📦 Current binary: $BINARY_PATH"
    echo "   Built: $(date -r $BINARY_TIME '+%Y-%m-%d %H:%M:%S')"
    
    if [ "$LATEST_SRC_TIME" -gt "$BINARY_TIME" ]; then
        echo "   ⚠️  Binary is OLDER than source code!"
        NEED_BUILD=true
    else
        echo "   ✅ Binary is up-to-date"
    fi
else
    echo ""
    echo "   ⚠️  Binary not found!"
    NEED_BUILD=true
fi

# 3. 构建（如果需要）
if [ "$NEED_BUILD" = true ]; then
    echo ""
    echo "🔨 Building release binaries..."
    cargo build --release
    
    echo ""
    echo "✅ Build complete!"
    echo "   Binary: $BINARY_PATH"
    echo "   Built: $(date -r $(stat -f "%m" "$BINARY_PATH") '+%Y-%m-%d %H:%M:%S')"
else
    echo ""
    echo "⏭️  Skipping build (binary is up-to-date)"
fi

# 4. 验证二进制文件
echo ""
echo "🔍 Verifying binaries..."
for bin in imgquality-hevc imgquality-av1 vidquality-hevc vidquality-av1; do
    BIN_PATH="$PROJECT_ROOT/target/release/$bin"
    if [ -f "$BIN_PATH" ]; then
        BIN_TIME=$(stat -f "%m" "$BIN_PATH")
        echo "   ✅ $bin: $(date -r $BIN_TIME '+%Y-%m-%d %H:%M:%S')"
    else
        echo "   ❌ $bin: NOT FOUND"
    fi
done

# 5. 更新双击脚本路径
echo ""
echo "📝 Updating drag-and-drop script..."
DRAG_SCRIPT="$PROJECT_ROOT/scripts/drag_and_drop_processor.sh"

if [ -f "$DRAG_SCRIPT" ]; then
    # 确保路径正确
    if grep -q "target/release/imgquality-hevc" "$DRAG_SCRIPT"; then
        echo "   ✅ Script paths are correct"
    else
        echo "   ⚠️  Fixing script paths..."
        sed -i '' 's|imgquality_hevc/target/release/|target/release/|g' "$DRAG_SCRIPT"
        echo "   ✅ Paths updated"
    fi
else
    echo "   ⚠️  Drag-and-drop script not found"
fi

# 6. 显示版本信息
echo ""
echo "📋 Version Info:"
if [ -f "$BINARY_PATH" ]; then
    "$BINARY_PATH" --version 2>/dev/null || echo "   (Version command not available)"
fi

# 7. 最终检查
echo ""
echo "🎯 Final Check:"
echo "   Project root: $PROJECT_ROOT"
echo "   Binary path: $BINARY_PATH"
echo "   Binary exists: $([ -f "$BINARY_PATH" ] && echo "✅ YES" || echo "❌ NO")"
echo "   Binary timestamp: $([ -f "$BINARY_PATH" ] && date -r $(stat -f "%m" "$BINARY_PATH") '+%Y-%m-%d %H:%M:%S' || echo "N/A")"

echo ""
echo "✅ Smart build complete!"
echo ""
echo "💡 To use the latest binary:"
echo "   $BINARY_PATH auto <input> --output <output> --recursive"

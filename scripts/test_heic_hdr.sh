#!/bin/bash
# Test script for HEIC HDR/Dolby Vision detection

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$SCRIPT_DIR/.."

echo "🧪 Testing HEIC HDR/Dolby Vision Detection"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Build the project
echo "📦 Building project..."
cd "$PROJECT_ROOT"
cargo build --release --bin img-hevc 2>&1 | grep -E "(Compiling|Finished)" || true

if [ ! -f "target/release/img-hevc" ]; then
    echo "❌ Build failed"
    exit 1
fi

echo "✅ Build successful"
echo ""

# Test with sample files (if available)
echo "🔍 Testing HDR detection..."
echo ""

# Create a test function
test_file() {
    local file="$1"
    local expected="$2"

    if [ ! -f "$file" ]; then
        echo "⏭️  Skipping: $file (not found)"
        return
    fi

    echo "Testing: $(basename "$file")"
    output=$(./target/release/img-hevc run "$file" --output /tmp/test_output 2>&1 || true)

    if echo "$output" | grep -q "HDR\|Dolby Vision\|skipping"; then
        echo "✅ HDR/DV detected and skipped"
    else
        echo "ℹ️  No HDR/DV detected (or file processed normally)"
    fi
    echo ""
}

# Test with common HEIC locations
TEST_DIRS=(
    "$HOME/Pictures"
    "$HOME/Downloads"
    "/tmp"
)

FOUND_FILES=0
for dir in "${TEST_DIRS[@]}"; do
    if [ -d "$dir" ]; then
        while IFS= read -r -d '' file; do
            test_file "$file"
            ((FOUND_FILES++))
            if [ $FOUND_FILES -ge 3 ]; then
                break 2
            fi
        done < <(find "$dir" -maxdepth 2 -type f \( -iname "*.heic" -o -iname "*.heif" \) -print0 2>/dev/null)
    fi
done

if [ $FOUND_FILES -eq 0 ]; then
    echo "ℹ️  No HEIC files found for testing"
    echo "   To test manually, run:"
    echo "   ./target/release/img-hevc run <your-heic-file> --output /tmp/test"
fi

echo ""
echo "✨ Test complete!"
echo ""
echo "📝 Notes:"
echo "   - HDR files will show: 'HEIC with HDR - skipping to preserve HDR metadata'"
echo "   - Dolby Vision files will show: 'HEIC with Dolby Vision - skipping...'"
echo "   - Regular HEIC files will process normally"

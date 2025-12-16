#!/bin/bash
# v6.6 Real File Test Script

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BIN="$PROJECT_DIR/vidquality_hevc/target/release/vidquality-hevc"
TEST_VIDEO="$PROJECT_DIR/test_videos/test_short.mp4"
OUTPUT_DIR="/tmp/v6.6_test_output"

echo "🧪 v6.6 Real File Test"
echo ""

rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

# Test 1: --explore
echo "Test 1: --explore"
"$BIN" auto "$TEST_VIDEO" --explore -o "$OUTPUT_DIR/test1.mp4" 2>&1 | grep -E "(Result|PASSED|FAILED|CRF|SSIM)" | head -5
[ -f "$OUTPUT_DIR/test1.mp4" ] && echo "✅ Test 1 PASSED" || echo "❌ Test 1 FAILED"
echo ""

# Test 2: --explore --match-quality true
echo "Test 2: --explore --match-quality true"
"$BIN" auto "$TEST_VIDEO" --explore --match-quality true -o "$OUTPUT_DIR/test2.mp4" 2>&1 | grep -E "(Result|PASSED|FAILED|CRF|SSIM)" | head -5
[ -f "$OUTPUT_DIR/test2.mp4" ] && echo "✅ Test 2 PASSED" || echo "❌ Test 2 FAILED"
echo ""

# Test 3: --compress
echo "Test 3: --compress"
"$BIN" auto "$TEST_VIDEO" --compress -o "$OUTPUT_DIR/test3.mp4" 2>&1 | grep -E "(Result|PASSED|FAILED|CRF|SSIM)" | head -5
[ -f "$OUTPUT_DIR/test3.mp4" ] && echo "✅ Test 3 PASSED" || echo "❌ Test 3 FAILED"
echo ""

# Test 4: --explore --match-quality true --compress
echo "Test 4: --explore --match-quality true --compress"
"$BIN" auto "$TEST_VIDEO" --explore --match-quality true --compress -o "$OUTPUT_DIR/test4.mp4" 2>&1 | grep -E "(Result|PASSED|FAILED|CRF|SSIM)" | head -5
[ -f "$OUTPUT_DIR/test4.mp4" ] && echo "✅ Test 4 PASSED" || echo "❌ Test 4 FAILED"
echo ""

echo "📊 Output files:"
ls -la "$OUTPUT_DIR/"

#!/bin/bash
# debug/generate_test_media.sh
# Run this script to generate a real HDR10+ test video and verify if modern_format_boost retains the metadata.

set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
JSON_TEMPLATE="$DIR/dummy_hdr10plus.json"
INPUT_MKV="$DIR/test_hdr10plus_input.mkv"
OUTPUT_DIR="$DIR/output"

echo "=========================================================="
echo "🎬 1. Generating synthetic HDR10+ video (test_hdr10plus_input.mkv)..."
echo "=========================================================="
ffmpeg -y -hide_banner -loglevel warning -f lavfi -i color=c=blue:s=320x240:r=24:d=2 \
  -c:v libx265 -x265-params "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:dhdr10-info=$JSON_TEMPLATE" \
  -pix_fmt yuv420p10le "$INPUT_MKV"

echo ""
echo "=========================================================="
echo "🚀 2. Running modern_format_boost to process the HDR10+ video..."
echo "=========================================================="
# Run the Rust binary directly via cargo with specific output dir
cd "$DIR/.."
mkdir -p "$OUTPUT_DIR"
cargo run --bin vid-hevc -- run "$INPUT_MKV" -o "$DIR/output" --force --apple-compat

echo ""
echo "=========================================================="
echo "🔎 3. Verifying output for HDR10+ metadata retention..."
echo "=========================================================="
# The output should be test_hdr10plus_input.MOV in the output dir because of --apple-compat
OUTPUT_MKV="$OUTPUT_DIR/test_hdr10plus_input.MOV"
if [ ! -f "$OUTPUT_MKV" ]; then
    echo "❌ modern_format_boost output file not found: $OUTPUT_MKV"
    exit 1
fi

EXTRACTED_HEVC="$DIR/extracted_output.hevc"
EXTRACTED_JSON="$DIR/extracted_output.json"

echo "Extracting raw HEVC stream from output..."
ffmpeg -y -hide_banner -loglevel error -i "$OUTPUT_MKV" -c:v copy -bsf:v hevc_mp4toannexb -f hevc "$EXTRACTED_HEVC"

echo "Running hdr10plus_tool extract on raw HEVC..."
hdr10plus_tool extract -i "$EXTRACTED_HEVC" -o "$EXTRACTED_JSON"

if [ -s "$EXTRACTED_JSON" ]; then
    echo "✅ SUCCESS: HDR10+ dynamic metadata was successfully accessed by hdr10plus_tool in the final output!"
    echo "Snippet of extracted JSON:"
    head -n 10 "$EXTRACTED_JSON"
    echo "Test successfully validated HDR10+ retention!"
    exit 0
else
    echo "❌ FAILURE: hdr10plus_tool could not find HDR10+ metadata in the output."
    exit 1
fi

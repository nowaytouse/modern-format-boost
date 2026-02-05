#!/bin/bash
# Video Remediation Script with Diagnostics
SRC_BASE="/Users/nyamiiko/Downloads/all/1"
OUT_BASE="/Users/nyamiiko/Downloads/all/1_optimized"
LIST="/Users/nyamiiko/Downloads/GitHub/modern_format_boost/vidquality_hevc/diff_videos_1.list"
BIN="/Users/nyamiiko/Downloads/GitHub/modern_format_boost/target/release/vidquality-hevc"

echo "Starting Video Remediation for $(wc -l < $LIST) files..."

# Ensure binary is ready
(cd /Users/nyamiiko/Downloads/GitHub/modern_format_boost && cargo build --release -p vidquality-hevc)

# Process only first 5 as a sample to show diagnostics quickly, then user can decide for more
head -n 5 "$LIST" | while read -r REL_PATH; do
    FULL_PATH="$SRC_BASE/$REL_PATH"
    echo "Processing $REL_PATH..."
    "$BIN" auto "$FULL_PATH" \
        --output "$OUT_BASE" \
        --base-dir "$SRC_BASE" \
        --force --verbose \
        --explore --match-quality --compress --apple-compat --allow-size-tolerance --ultimate
done

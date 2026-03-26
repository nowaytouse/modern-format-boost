#!/bin/bash
# debug/verify.sh
# Run this script to verify the HDR10+ metadata injection logic locally.

echo "🔍 Starting HDR10+ Metadata Injection Logic Verification..."

# Run the newly added unit test in vid_hevc
if cargo test -p vid-hevc --lib tests::test_hdr10plus_injection_logic -- --nocapture; then
	echo ""
	echo "🎉 SUCCESS: HDR10+ metadata injection logic is confirmed and reliable."
	echo "The x265-params are correctly constructed with :dhdr10-info=<path> when HDR10+ is detected."
else
	echo ""
	echo "❌ FAILURE: Logic verification failed. Please check the test output above."
	exit 1
fi

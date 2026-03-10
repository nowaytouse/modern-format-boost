#!/bin/bash

# Test script to verify timer cleanup on exit

echo "🧪 Testing timer cleanup..."

# Source the common functions
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/scripts/common.sh"

# Import the functions we need to test
source "$SCRIPT_DIR/scripts/drag_and_drop_processor.sh" 2>/dev/null || {
    echo "❌ Could not source drag_and_drop_processor.sh"
    exit 1
}

# Test 1: Check initial state
echo "1. Testing initial state..."
if [[ "$ELAPSED_START" -eq 0 ]]; then
    echo "   ✅ ELAPSED_START initially 0"
else
    echo "   ❌ ELAPSED_START initially $ELAPSED_START (should be 0)"
fi

# Test 2: Start timer
echo "2. Starting timer..."
start_elapsed_spinner
echo "   ELAPSED_START = $ELAPSED_START (should be > 0)"

# Test 3: Simulate cleanup
echo "3. Testing cleanup function..."
_cleanup_on_exit
echo "   ELAPSED_START = $ELAPSED_START (should be 0)"

# Test 4: Verify cleanup worked
if [[ "$ELAPSED_START" -eq 0 ]]; then
    echo "   ✅ Cleanup successful - timer reset"
else
    echo "   ❌ Cleanup failed - timer still $ELAPSED_START"
fi

echo "🎉 Timer cleanup test completed!"

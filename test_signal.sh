#!/bin/bash

echo "🧪 Testing signal handling..."

ELAPSED_START=$(date +%s)

_handle_sigint() {
    echo ""
    echo "🎯 Signal handler triggered!"
    local elapsed=0
    [[ "$ELAPSED_START" -gt 0 ]] && elapsed=$(( $(date +%s) - ELAPSED_START ))
    echo "   Elapsed time: ${elapsed}s"
    
    if [[ "$elapsed" -lt 5 ]]; then
        echo "   Under threshold, exiting immediately..."
        exit 130
    else
        echo "   Over threshold, asking for confirmation..."
        echo -n "   Confirm exit? [y/N]: "
        read -t 5 answer
        if [[ "$answer" == "y" || "$answer" == "Y" ]]; then
            echo "   Exiting..."
            exit 130
        else
            echo "   Resuming..."
        fi
    fi
}

trap '_handle_sigint' INT

echo "📊 Process started. Try Ctrl+C after 5 seconds to see confirmation prompt."
echo "   Current time: $(date)"
echo "   Process PID: $$"

# Simulate work
for i in {1..20}; do
    echo "Working... $i/20"
    sleep 1
done

echo "✅ Test completed without interruption."

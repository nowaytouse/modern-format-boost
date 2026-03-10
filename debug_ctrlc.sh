#!/bin/bash

echo "🔍 Debug Ctrl+C handling..."

# Test 1: Basic signal handling
echo "Test 1: Basic signal handling"
ELAPSED_START=$(date +%s)

_handle_sigint() {
    echo "🎯 Signal received!"
    local elapsed=$(( $(date +%s) - ELAPSED_START ))
    echo "   Elapsed: ${elapsed}s"
    
    if [[ $elapsed -lt 3 ]]; then
        echo "   Under 3s, exiting..."
        exit 130
    else
        echo "   Over 3s, asking confirmation..."
        echo -n "   Exit? [y/N]: "
        read -t 5 answer < /dev/tty
        if [[ "$answer" == "y" ]]; then
            echo "   Exiting..."
            exit 130
        else
            echo "   Resuming..."
        fi
    fi
}

trap '_handle_sigint' INT

echo "   Process PID: $$"
echo "   Try Ctrl+C after 3 seconds"

# Test 2: Pipeline signal handling
echo ""
echo "Test 2: Pipeline signal handling"
(
    ELAPSED_START=$(date +%s)
    trap '_handle_sigint' INT
    
    echo "   Subshell PID: $$"
    echo "   Working in pipeline..."
    for i in {1..10}; do
        echo "   Step $i/10"
        sleep 1
    done
) | tee /tmp/test_output.log

echo "✅ Tests completed"

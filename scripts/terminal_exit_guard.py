#!/usr/bin/env python3

import sys
import signal
import subprocess
import time

def confirm_exit():
    try:
        result = subprocess.run([
            'osascript', '-e',
            '''tell application "System Events"
                set theResponse to button returned of (display dialog "⚠️ Are you sure you want to exit terminal?" buttons {"❌ Cancel", "✅ OK"} default button "✅ OK" cancel button "❌ Cancel" with icon caution)
            end tell'''
        ], capture_output=True, text=True, timeout=30)
        
        if result.returncode == 0:
            return result.stdout.strip() == "✅ OK"
        else:
            return False
    except Exception:
        return False

def handle_exit(signum=None, frame=None):
    if confirm_exit():
        print("✅ Exit confirmed")
        sys.exit(0)
    else:
        print("❌ Exit cancelled")
        signal.signal(signal.SIGINT, handle_exit)
        signal.signal(signal.SIGTERM, handle_exit)

def main():
    print("🛡️ Terminal exit guard activated")
    print("Any exit method will show confirmation dialog")
    
    signal.signal(signal.SIGINT, handle_exit)
    signal.signal(signal.SIGTERM, handle_exit)
    
    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        handle_exit()

if __name__ == "__main__":
    main()

#!/usr/bin/env python3
# Fetch GNU MPC 1.4.1 with mirror fallbacks (CI apt libmpc-dev is 1.3.x).

import sys
import time
import urllib.request
import urllib.error

output = sys.argv[1] if len(sys.argv) > 1 else "mpc.tar.xz"
mirrors = [
    "https://ftpmirror.gnu.org/mpc/mpc-1.4.1.tar.xz",
    "https://ftp.gnu.org/gnu/mpc/mpc-1.4.1.tar.xz",
    "https://mirror.math.princeton.edu/pub/gnu/mpc/mpc-1.4.1.tar.xz",
]

success = False
for url in mirrors:
    for attempt in range(1, 4):
        print(
            f"Attempting to download MPC from {url} (attempt {attempt})...",
            file=sys.stderr,
        )
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
            with (
                urllib.request.urlopen(req, timeout=180) as response,
                open(output, "wb") as out_file,
            ):
                out_file.write(response.read())
            print(f"MPC tarball fetched from {url}", file=sys.stderr)
            success = True
            break
        except Exception as e:
            print(f"Download failed: {e}", file=sys.stderr)
            time.sleep(attempt * 5)
    if success:
        break

if success:
    sys.exit(0)
else:
    print("Failed to download MPC 1.4.1 from all mirrors", file=sys.stderr)
    sys.exit(1)

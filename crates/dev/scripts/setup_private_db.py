#!/usr/bin/env python3
# Modern Format Boost - Private Environment Setup Helper
# This script creates a local environment file that is IGNORED by Git.
# Use this to safely store your database credentials without leaking them.

import json
import sys

from fastmode_paths import default_mfb_state_root

# Colors
BLUE = "\033[1;34m"
GREEN = "\033[1;32m"
YELLOW = "\033[1;33m"
DIM = "\033[2m"
RESET = "\033[0m"

CONF_DIR = default_mfb_state_root()
CONF_FILE_JSON = CONF_DIR / "local_env.json"
CONF_FILE_SH = CONF_DIR / "local_env.sh"

CONF_DIR.mkdir(parents=True, exist_ok=True)

print(f"{BLUE}Modern Format Boost - Private Environment Setup (Python Edition){RESET}")
print("--------------------------------------------------")

existing_conn_str = None

# Try loading existing JSON first
if CONF_FILE_JSON.exists():
    print(f"{YELLOW}Existing JSON configuration found at: {CONF_FILE_JSON}{RESET}")
    try:
        with open(CONF_FILE_JSON) as f:
            config = json.load(f)
            existing_conn_str = config.get("MFB_PG_CONNSTR")
            print(json.dumps(config, indent=2))
    except Exception as e:
        print(f"Error reading existing JSON: {e}")
    print("--------------------------------------------------")
# Try loading old sh config as fallback
elif CONF_FILE_SH.exists():
    print(f"{YELLOW}Existing shell configuration found at: {CONF_FILE_SH}{RESET}")
    try:
        with open(CONF_FILE_SH) as f:
            content = f.read()
            print(content)
            for line in content.splitlines():
                if "MFB_PG_CONNSTR" in line and "export " in line:
                    parts = line.replace("export ", "", 1).split("=", 1)
                    if len(parts) == 2:
                        existing_conn_str = parts[1].strip().strip("\"'")
    except Exception as e:
        print(f"Error reading existing sh: {e}")
    print("--------------------------------------------------")

default_conn_str = existing_conn_str or "postgresql:///modern_format_boost"

print("Enter your PostgreSQL connection string (Press Enter for default):")
print(f"{DIM}Default: {default_conn_str}{RESET}")
try:
    conn_str = input("> ").strip()
except (KeyboardInterrupt, EOFError):
    print("\nCancelled.")
    sys.exit(0)

if not conn_str:
    conn_str = default_conn_str

# Write to JSON
config_data = {"MFB_PG_CONNSTR": conn_str}

try:
    with open(CONF_FILE_JSON, "w") as f:
        json.dump(config_data, f, indent=2)
    CONF_FILE_JSON.chmod(0o755)
    print(f"\n{GREEN}✅ Configuration saved to: {CONF_FILE_JSON}{RESET}")
    print(
        f"{DIM}The drag-and-drop processor will now load this file automatically.{RESET}"
    )
except Exception as e:
    print(f"\nError writing configuration: {e}")
    sys.exit(1)

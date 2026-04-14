#!/usr/bin/env python3
import sys
import subprocess
import shutil
import platform
import time

GREEN = '\033[0;32m'
BLUE = '\033[0;34m'
YELLOW = '\033[1;33m'
RED = '\033[0;31m'
NC = '\033[0m'

DB_NAME = "modern_format_boost"
OS_TYPE = platform.system().lower()

def print_c(color, text, end='\n'):
    print(f"{color}{text}{NC}", end=end)

def command_exists(cmd):
    return shutil.which(cmd) is not None

def run_cmd(cmd, check=False):
    return subprocess.run(cmd, shell=True, check=check, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)

def check_deps():
    if not command_exists("psql"):
        print_c(RED, "❌ PostgreSQL not found. Please run 'python3 scripts/install_deps.py' first.")
        sys.exit(1)

def start_service():
    print_c(YELLOW, "🔄 Attempting to start PostgreSQL service...")
    if OS_TYPE == "darwin":
        if command_exists("brew"):
            res = run_cmd("brew services list | awk '/^postgresql/ {print $1}' | head -n 1")
            pg_service = res.stdout.strip()
            if pg_service:
                print(f"Starting service '{pg_service}' via Homebrew...")
                run_cmd(f"brew services start {pg_service}")
            else:
                print_c(YELLOW, "⚠️  No PostgreSQL service found in 'brew services'. Trying to install default...")
                run_cmd("brew install postgresql && brew services start postgresql")
        else:
            print("Homebrew not found. Trying pg_ctl...")
            run_cmd("pg_ctl start")
    elif OS_TYPE == "linux":
        if command_exists("systemctl"):
            run_cmd("sudo systemctl start postgresql")
        else:
            run_cmd("sudo service postgresql start")
    time.sleep(2)

def setup_db():
    print_c(BLUE, f"🏗️  Setting up database: {DB_NAME}")
    
    res = run_cmd(f"psql -lqt | cut -d \\| -f 1 | grep -qw '{DB_NAME}'")
    if res.returncode == 0:
        print_c(GREEN, f"✅ Database '{DB_NAME}' already exists.")
    else:
        print(f"Creating database '{DB_NAME}'...")
        if run_cmd(f"createdb \"{DB_NAME}\"").returncode != 0:
            run_cmd(f"psql -c \"CREATE DATABASE {DB_NAME};\"")
            
    print("Ensuring pgvector extension is enabled...")
    res = run_cmd(f"psql -d \"{DB_NAME}\" -c \"CREATE EXTENSION IF NOT EXISTS vector;\"")
    if res.returncode != 0:
        print_c(RED, "❌ Failed to create 'vector' extension. Is 'pgvector' installed?")
        print("Try: brew install pgvector (macOS) or refer to your Linux distribution's pgvector package.")
        sys.exit(1)
        
    print_c(GREEN, "✅ Database setup complete!")

def get_status():
    if run_cmd("pg_isready").returncode == 0:
        print(f"Status: {GREEN}RUNNING{NC}")
        res = run_cmd(f"psql -lqt | cut -d \\| -f 1 | grep -qw '{DB_NAME}'")
        if res.returncode == 0:
            print(f"Database: {GREEN}{DB_NAME} (Ready){NC}")
        else:
            print(f"Database: {YELLOW}{DB_NAME} (Missing, run 'setup'){NC}")
    else:
        print(f"Status: {RED}STOPPED{NC}")

def main():
    print_c(BLUE, "🐘 Modern Format Boost - Database Manager v0.1.0")
    print("--------------------------------------------------------")
    
    if len(sys.argv) < 2:
        print(f"Usage: {sys.argv[0]} {{start|setup|status}}")
        sys.exit(1)
        
    action = sys.argv[1]
    
    if action == "start":
        check_deps()
        start_service()
        get_status()
    elif action == "setup":
        check_deps()
        start_service()
        setup_db()
    elif action == "status":
        get_status()
    else:
        print(f"Usage: {sys.argv[0]} {{start|setup|status}}")
        sys.exit(1)

if __name__ == "__main__":
    main()

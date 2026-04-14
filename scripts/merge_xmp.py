#!/usr/bin/env python3
"""
Merge XMP sidecar files into media files using exiftool.
References the Rust implementation in crates/shared_utils/src/xmp_merger.rs
"""

import sys
import os
import subprocess
from pathlib import Path
import shutil
from typing import Optional

# Console Output Colors
if sys.stdout.isatty():
    RED = '\033[38;5;196m'
    GREEN = '\033[38;5;46m'
    CYAN = '\033[38;5;51m'
    YELLOW = '\033[38;5;226m'
    BOLD = '\033[1m'
    DIM = '\033[2m'
    RESET = '\033[0m'
else:
    RED = GREEN = CYAN = YELLOW = BOLD = DIM = RESET = ''

EXCLUDED_EXTENSIONS = {
    ".xmp", ".txt", ".md", ".json", ".xml", ".yaml", ".yml", ".toml", ".ini", ".cfg",
    ".conf", ".log", ".rs", ".py", ".js", ".ts", ".html", ".css", ".sh", ".bash",
    ".zsh", ".c", ".cpp", ".h", ".hpp", ".java", ".zip", ".tar", ".gz", ".bz2",
    ".xz", ".7z", ".rar", ".ds_store", ".thumbs.db", ".desktop.ini"
}

def print_header():
    print()
    print(f"{CYAN}{BOLD}Modern Format Boost - XMP Merger Tool{RESET}")
    print(f"{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}\n")

def check_exiftool():
    if not shutil.which("exiftool"):
        print(f"{RED}❌ ExifTool not found. Please install it first: brew install exiftool{RESET}")
        sys.exit(1)

def is_potential_media(ext: str) -> bool:
    return ext.lower() not in EXCLUDED_EXTENSIONS and bool(ext)

def find_media_match(xmp_path: Path) -> Optional[Path]:
    parent = xmp_path.parent
    xmp_name = xmp_path.name
    xmp_stem = xmp_path.stem

    # Strategy 1: Direct match (e.g. image.jpg.xmp -> image.jpg)
    if xmp_name.lower().endswith('.xmp'):
        base_name = xmp_name[:-4]
        direct_candidate = parent / base_name
        if direct_candidate.is_file() and is_potential_media(direct_candidate.suffix):
            return direct_candidate

    # Strategy 2: Same stem, different extension (e.g. image.xmp -> image.jpg)
    if not parent.exists() or not parent.is_dir():
        return None
        
    for p in parent.iterdir():
        if not p.is_file():
            continue
        ext = p.suffix.lower()
        if not is_potential_media(ext):
            continue
            
        file_stem = p.stem
        # Match "image" from "image.xmp" to "image" from "image.jpg"
        if file_stem == xmp_stem:
            return p
            
        # Match "image" from "image.xmp" to "image" from "image.jpg.heic"
        file_root_stem = file_stem.split('.')[0]
        xmp_root_stem = xmp_stem.split('.')[0]
        if file_root_stem == xmp_root_stem and file_root_stem:
            return p
            
    return None

def merge_xmp(xmp_path: Path, media_path: Path) -> bool:
    print(f"  {DIM}Merging:{RESET} {xmp_path.name} {CYAN}➜{RESET} {media_path.name}")
    
    # Safe merge command modeled after Rust logic
    # -tagsfromfile: copy all tags
    # -all:all: copy namespace tags
    # -unsafe: copy unsafe tags
    # -FileModifyDate<FileModifyDate: preserve file time
    # -overwrite_original: don't create _original backup
    cmd = [
        "exiftool",
        "-charset", "filename=utf8",
        "-api", "windowsunicode=1",
        "-api", "LargeFileSupport=1",
        "-tagsfromfile", str(xmp_path),
        "-all:all",
        "-unsafe",
        "-FileModifyDate<FileModifyDate",
        "-overwrite_original",
        str(media_path)
    ]
    
    # Run the command
    try:
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if res.returncode != 0:
            stderr = res.stderr.strip()
            # Minor warnings are acceptable, check for real errors
            is_real_error = ("Error:" in stderr or "Error opening" in stderr or 
                             "File not found" in stderr or "not writing image" in stderr)
            if is_real_error and not "[minor]" in stderr.lower():
                print(f"  {RED}❌ Failed: {stderr}{RESET}")
                return False
                
        print(f"  {GREEN}✅ Success{RESET}")
        return True
    except Exception as e:
        print(f"  {RED}❌ Error executing exiftool: {e}{RESET}")
        return False

def main():
    if len(sys.argv) > 1:
        target_dir = Path(sys.argv[1]).resolve()
    else:
        print(f"{RED}❌ Error: Please provide a target directory.{RESET}")
        print(f"Usage: python3 scripts/merge_xmp.py /path/to/files")
        sys.exit(1)
        
    print_header()
    check_exiftool()
    
    if not target_dir.is_dir():
        print(f"{RED}❌ Directory not found: {target_dir}{RESET}")
        sys.exit(1)

    print(f"{BOLD}Scanning for XMP files in:{RESET} {target_dir}\n")
    
    xmp_files = []
    for root, _, files in os.walk(target_dir):
        for file in files:
            if file.lower().endswith('.xmp'):
                xmp_files.append(Path(root) / file)
                
    if not xmp_files:
        print(f"{YELLOW}No .xmp files found in the target directory.{RESET}")
        sys.exit(0)
        
    print(f"Found {len(xmp_files)} XMP file(s). Looking for media matches...\n")
    
    success_count = 0
    fail_count = 0
    skip_count = 0
    
    for xmp_path in xmp_files:
        media_path = find_media_match(xmp_path)
        if media_path:
            if merge_xmp(xmp_path, media_path):
                success_count += 1
            else:
                fail_count += 1
        else:
            print(f"  {YELLOW}⚠️  Skipped (No match):{RESET} {xmp_path.name}")
            skip_count += 1

    print(f"\n{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}")
    print(f"{BOLD}Summary:{RESET}")
    print(f"  {GREEN}Merged   :{RESET} {success_count}")
    
    if fail_count > 0:
        print(f"  {RED}Failed   :{RESET} {fail_count}")
    else:
        print(f"  {DIM}Failed   :{RESET} {fail_count}")
        
    if skip_count > 0:
        print(f"  {YELLOW}Skipped  :{RESET} {skip_count}")
    else:
        print(f"  {DIM}Skipped  :{RESET} {skip_count}")

    if fail_count > 0 or skip_count > 0:
        sys.exit(1)

if __name__ == "__main__":
    main()

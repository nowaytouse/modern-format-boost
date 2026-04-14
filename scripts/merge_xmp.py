#!/usr/bin/env python3
"""
Merge XMP sidecar files into media files using exiftool.
References the complete Rust implementation in crates/shared_utils/src/xmp_merger.rs
"""

import sys
import os
import subprocess
from pathlib import Path
import shutil
import time
from typing import Optional, Tuple, Dict, List

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

class XmpInfo:
    def __init__(self, doc_id: str = "", derived: str = "", source: str = ""):
        self.doc_id = doc_id
        self.derived = derived
        self.source = source

def print_header():
    print()
    print(f"{CYAN}{BOLD}Modern Format Boost - XMP Merger Tool (Deep Search Edition){RESET}")
    print(f"{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}\n")

def check_exiftool():
    if not shutil.which("exiftool"):
        print(f"{RED}❌ ExifTool not found. Please install it first: brew install exiftool{RESET}")
        sys.exit(1)

def is_potential_media(ext: str) -> bool:
    return ext.lower() not in EXCLUDED_EXTENSIONS and bool(ext)

def extract_xmp_metadata(xmp_path: Path) -> XmpInfo:
    """Strategy 3 & 4 helper: extracts DocumentID, DerivedFrom, Source from XMP using exiftool."""
    cmd = [
        "exiftool", "-charset", "filename=utf8", "-api", "windowsunicode=1",
        "-api", "LargeFileSupport=1", "-s3",
        "-DocumentID", "-DerivedFrom", "-Source", "-OriginalDocumentID",
        str(xmp_path)
    ]
    try:
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if res.returncode != 0:
            return XmpInfo()
        
        lines = [line.strip() for line in res.stdout.split('\n') if line.strip()]
        doc_id = lines[0] if len(lines) > 0 else ""
        derived = lines[1] if len(lines) > 1 else ""
        source = lines[2] if len(lines) > 2 else ""
        
        return XmpInfo(doc_id=doc_id, derived=derived, source=source)
    except Exception:
        return XmpInfo()

def is_uuid_format(name: str) -> bool:
    parts = name.split('-')
    if len(parts) != 5:
        return False
    expected = [8, 4, 4, 4, 12]
    return all(len(p) == expected[i] and all(c in "0123456789abcdefABCDEF" for c in p) 
               for i, p in enumerate(parts))

def generate_candidates(parent: Path) -> List[Path]:
    if not parent.exists() or not parent.is_dir():
        return []
    return [p for p in parent.iterdir() if p.is_file() and is_potential_media(p.suffix)]

def normalize_filename(name: str) -> str:
    return "".join(c for c in name if c.isalnum()).lower()

def extract_media_doc_id(media_path: Path) -> str:
    cmd = ["exiftool", "-s3", "-DocumentID", str(media_path)]
    try:
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        return res.stdout.strip()
    except Exception:
        return ""

def scan_xmp_ref(media_path: Path, target_xmp: str) -> bool:
    cmd = ["exiftool", "-s3", "-SidecarForExtension", "-XMPFileRef", str(media_path)]
    try:
        res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        if res.returncode == 0 and target_xmp in res.stdout:
            return True
        return False
    except Exception:
        return False

def find_media_match(xmp_path: Path) -> Tuple[Optional[Path], str]:
    parent = xmp_path.parent
    xmp_name = xmp_path.name
    xmp_stem = xmp_path.stem
    xmp_ext = xmp_path.suffix.lower()
    
    candidates = generate_candidates(parent)

    # Strategy 1: Direct match (e.g. image.jpg.xmp -> image.jpg)
    if xmp_name.lower().endswith('.xmp'):
        base_name = xmp_name[:-4]
        direct_candidate = parent / base_name
        if direct_candidate.is_file() and is_potential_media(direct_candidate.suffix):
            return direct_candidate, "Direct match"

    # Strategy 2: Same name different ext
    xmp_root_stem = xmp_stem.split('.')[0].lower()
    for p in candidates:
        file_stem_lower = p.stem.lower()
        file_root_stem = file_stem_lower.split('.')[0]
        if file_stem_lower == xmp_stem.lower() or file_root_stem == xmp_root_stem:
            return p, "Same name, diff ext"

    # Strategy 2.5: Case insensitive
    xmp_stem_lower = xmp_stem.lower()
    for p in candidates:
        if p.stem.lower() == xmp_stem_lower:
            return p, "Case insensitive match"

    # Extract XMP structured data
    xmp_info = extract_xmp_metadata(xmp_path)

    # Strategy 3: XMP Metadata references
    if xmp_info.derived and not "uuid:" in xmp_info.derived:
        candidate = parent / xmp_info.derived
        if candidate.is_file():
            return candidate, "DerivedFrom match"
            
    if xmp_info.source:
        candidate = parent / xmp_info.source
        if candidate.is_file():
            return candidate, "Source meta match"
            
    # Strategy 4: DocumentID
    if xmp_info.doc_id and is_uuid_format(xmp_stem):
        print(f"  🔍 Searching by DocumentID: {xmp_info.doc_id}")
        for p in candidates:
            if extract_media_doc_id(p) == xmp_info.doc_id:
                return p, "DocumentID match"

    # Strategy 5: Fuzzy match (alphanumeric only)
    norm_xmp = normalize_filename(xmp_stem)
    norm_xmp_root = normalize_filename(xmp_root_stem)
    if norm_xmp:
        for p in candidates:
            file_stem = p.stem
            norm_file = normalize_filename(file_stem)
            norm_file_root = normalize_filename(file_stem.split('.')[0])
            if norm_file == norm_xmp or norm_file_root == norm_xmp_root:
                return p, "Fuzzy match"

    # Strategy 6: XMP Reference scan (scan media files)
    for p in candidates:
        if scan_xmp_ref(p, xmp_name):
            return p, "XMP ref scan match"
            
    # Strategy 7: Partial containment match
    if len(xmp_stem) >= 4:
        for p in candidates:
            file_stem = p.stem
            if file_stem in xmp_stem or xmp_stem in file_stem:
                shorter = min(len(xmp_stem), len(file_stem))
                longer = max(len(xmp_stem), len(file_stem))
                if (shorter * 100) / longer >= 70:
                    return p, "Partial string match"

    # Strategy 8: Subdirectory check (depth 2)
    for p in parent.rglob("*"):
        if not p.is_file() or p == xmp_path:
            continue
        # Max depth 2 approx
        try:
            rel = p.relative_to(parent)
            if len(rel.parts) > 2:
                continue
        except Exception:
            continue
            
        if is_potential_media(p.suffix) and p.stem.lower() == xmp_stem_lower:
            return p, "Subdirectory match"

    return None, "No match"

def get_timestamps(path: Path) -> Optional[Tuple[float, float]]:
    try:
        stat = path.stat()
        return (stat.st_atime, stat.st_mtime)
    except Exception:
        return None

def restore_timestamps(path: Path, times: Optional[Tuple[float, float]]):
    if times:
        try:
            os.utime(path, times)
        except Exception as e:
            print(f"  {YELLOW}⚠️  Failed to restore timestamps for {path.name}: {e}{RESET}")

def merge_xmp(xmp_path: Path, media_path: Path, strategy: str) -> bool:
    print(f"  {DIM}Merge [{strategy}]:{RESET} {xmp_path.name} {CYAN}➜{RESET} {media_path.name}")
    
    # 1. Snapshot timestamps (File & Parent Folder)
    original_file_times = get_timestamps(media_path)
    parent_dir = media_path.parent
    original_parent_times = get_timestamps(parent_dir)
    
    # 2. Read XMP data into memory (matching Rust's stdin approach)
    try:
        with open(xmp_path, 'rb') as f:
            xmp_data = f.read()
    except Exception as e:
        print(f"  {RED}❌ Failed to read XMP file: {e}{RESET}")
        return False

    # 3. Environment Checks
    apple_compat = os.environ.get("MODERN_FORMAT_BOOST_APPLE_COMPAT") is not None
    is_jxl = media_path.suffix.lower() == ".jxl"

    # 4. Build exact command parity with Rust xmp_merger.rs
    # builder.use_stdin().preserve_date().quiet().quiet().ignore_minor()
    cmd = [
        "exiftool",
        "-charset", "filename=utf8",
        "-api", "windowsunicode=1",
        "-api", "LargeFileSupport=1",
        "-q", "-q",
        "-m", # ignore minor errors
        "-tagsfromfile", "-", # pipe from stdin
        "-all:all",
        "-unsafe"
    ]
    
    if is_jxl and apple_compat:
        # Special JXL Apple-compat logic from Rust:
        # .strip_all().tags_from_file("@").arg("-all:all").unsafe_tags().arg("-icc_profile")
        cmd.extend([
            "-all=", # strip_all
            "-tagsfromfile", "@",
            "-all:all",
            "-unsafe",
            "-icc_profile"
        ])

    # Final args: preserve FileModifyDate and set target
    cmd.extend([
        "-FileModifyDate<FileModifyDate",
        "-overwrite_original",
        str(media_path)
    ])
    
    # 5. Run the command with stdin pipe
    try:
        process = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE
        )
        stdout, stderr = process.communicate(input=xmp_data)
        
        if process.returncode != 0:
            err_msg = stderr.decode('utf-8', errors='replace').strip()
            # Rust check logic for real errors vs minor noise
            is_real_error = ("Error:" in err_msg or "Error opening" in err_msg or 
                             "File not found" in err_msg or "not writing image" in err_msg)
            if is_real_error and not "[minor]" in err_msg.lower():
                print(f"  {RED}❌ Failed: {err_msg}{RESET}")
                return False
                
        # 6. Restore File Timestamps (Deep Parity)
        restore_timestamps(media_path, original_file_times)
        
        # 7. Restore Parent Directory Timestamps (Enhanced Hardening)
        # This prevents the parent folder's modification time from "refreshing" 
        # due to ExifTool's temp-file swap (overwrite_original).
        restore_timestamps(parent_dir, original_parent_times)

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

    print(f"{BOLD}Deep Scanning for XMP files in:{RESET} {target_dir}\n")
    
    xmp_files = []
    for root, _, files in os.walk(target_dir):
        for file in files:
            if file.lower().endswith('.xmp'):
                xmp_files.append(Path(root) / file)
                
    if not xmp_files:
        print(f"{YELLOW}No .xmp files found in the target directory.{RESET}")
        sys.exit(0)
        
    print(f"Found {len(xmp_files)} XMP file(s). Searching via Deep 8-Strategy Pipeline...\n")
    
    success_count = 0
    fail_count = 0
    skip_count = 0
    
    for xmp_path in xmp_files:
        media_path, strategy = find_media_match(xmp_path)
        if media_path:
            if merge_xmp(xmp_path, media_path, strategy):
                success_count += 1
            else:
                fail_count += 1
        else:
            print(f"  {YELLOW}⚠️  Skipped ({strategy}):{RESET} {xmp_path.name}")
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

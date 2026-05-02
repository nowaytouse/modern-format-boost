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
import ctypes
from ctypes import (
    c_char_p,
    c_uint32,
    c_int32,
    c_void_p,
    c_size_t,
    Structure,
    c_uint16,
    c_int64,
    byref,
)

# Console Output Colors
if sys.stdout.isatty():
    RED = "\033[38;5;196m"
    GREEN = "\033[38;5;46m"
    CYAN = "\033[38;5;51m"
    YELLOW = "\033[38;5;226m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    RESET = "\033[0m"
else:
    RED = GREEN = CYAN = YELLOW = BOLD = DIM = RESET = ""

EXCLUDED_EXTENSIONS = {
    ".xmp",
    ".txt",
    ".md",
    ".json",
    ".xml",
    ".yaml",
    ".yml",
    ".toml",
    ".ini",
    ".cfg",
    ".conf",
    ".log",
    ".rs",
    ".py",
    ".js",
    ".ts",
    ".html",
    ".css",
    ".sh",
    ".bash",
    ".zsh",
    ".c",
    ".cpp",
    ".h",
    ".hpp",
    ".java",
    ".zip",
    ".tar",
    ".gz",
    ".bz2",
    ".xz",
    ".7z",
    ".rar",
    ".ds_store",
    ".thumbs.db",
    ".desktop.ini",
}


class XmpInfo:
    def __init__(self, doc_id: str = "", derived: str = "", source: str = ""):
        self.doc_id = doc_id
        self.derived = derived
        self.source = source


def print_header():
    print()
    print(
        f"{CYAN}{BOLD}Modern Format Boost - XMP Merger Tool (Deep Search Edition){RESET}"
    )
    print(f"{DIM}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━{RESET}\n")


def check_exiftool():
    if not shutil.which("exiftool"):
        print(
            f"{RED}❌ ExifTool not found. Please install it first: brew install exiftool{RESET}"
        )
        sys.exit(1)


def is_potential_media(ext: str) -> bool:
    return ext.lower() not in EXCLUDED_EXTENSIONS and bool(ext)


def extract_xmp_metadata(xmp_path: Path) -> XmpInfo:
    """Strategy 3 & 4 helper: extracts DocumentID, DerivedFrom, Source from XMP using exiftool."""
    cmd = [
        "exiftool",
        "-charset",
        "filename=utf8",
        "-api",
        "windowsunicode=1",
        "-api",
        "LargeFileSupport=1",
        "-s3",
        "-DocumentID",
        "-DerivedFrom",
        "-Source",
        "-OriginalDocumentID",
        str(xmp_path),
    ]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True)
        if res.returncode != 0:
            return XmpInfo()

        lines = [line.strip() for line in res.stdout.split("\n") if line.strip()]
        doc_id = lines[0] if len(lines) > 0 else ""
        derived = lines[1] if len(lines) > 1 else ""
        source = lines[2] if len(lines) > 2 else ""

        return XmpInfo(doc_id=doc_id, derived=derived, source=source)
    except Exception:
        return XmpInfo()


def is_uuid_format(name: str) -> bool:
    parts = name.split("-")
    if len(parts) != 5:
        return False
    expected = [8, 4, 4, 4, 12]
    return all(
        len(p) == expected[i] and all(c in "0123456789abcdefABCDEF" for c in p)
        for i, p in enumerate(parts)
    )


def generate_candidates(parent: Path) -> list[Path]:
    if not parent.exists() or not parent.is_dir():
        return []
    return [p for p in parent.iterdir() if p.is_file() and is_potential_media(p.suffix)]


def normalize_filename(name: str) -> str:
    return "".join(c for c in name if c.isalnum()).lower()


def extract_media_doc_id(media_path: Path) -> str:
    cmd = ["exiftool", "-s3", "-DocumentID", str(media_path)]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True)
        return res.stdout.strip()
    except Exception:
        return ""


def scan_xmp_ref(media_path: Path, target_xmp: str) -> bool:
    cmd = ["exiftool", "-s3", "-SidecarForExtension", "-XMPFileRef", str(media_path)]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True)
        if res.returncode == 0 and target_xmp in res.stdout:
            return True
        return False
    except Exception:
        return False


def extract_batch_doc_ids(media_paths: list[Path]) -> dict[str, str]:
    """Batch extract DocumentID from multiple files using exiftool."""
    if not media_paths:
        return {}
    cmd = [
        "exiftool",
        "-j",
        "-DocumentID",
        "-charset",
        "filename=utf8",
        "-api",
        "windowsunicode=1",
    ] + [str(p) for p in media_paths]
    try:
        res = subprocess.run(cmd, capture_output=True, text=True)
        if res.returncode != 0:
            return {}
        import json

        data = json.loads(res.stdout)
        # ExifTool SourceFile might use forward slashes even on Windows, or backslashes.
        # We'll normalize to absolute paths for comparison.
        return {
            str(Path(item["SourceFile"]).resolve()): item.get("DocumentID", "")
            for item in data
        }
    except Exception:
        return {}


def find_media_match(xmp_path: Path) -> tuple[Path | None, str]:
    parent = xmp_path.parent
    xmp_name = xmp_path.name
    xmp_stem = xmp_path.stem
    xmp_path.suffix.lower()

    candidates = generate_candidates(parent)

    # Strategy 1: Direct match (e.g. image.jpg.xmp -> image.jpg)
    if xmp_name.lower().endswith(".xmp"):
        base_name = xmp_name[:-4]
        direct_candidate = parent / base_name
        if direct_candidate.is_file() and is_potential_media(direct_candidate.suffix):
            return direct_candidate, "Direct match"

    # Strategy 2: Same name different ext
    xmp_root_stem = xmp_stem.split(".")[0].lower()
    for p in candidates:
        file_stem_lower = p.stem.lower()
        file_root_stem = file_stem_lower.split(".")[0]
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
    if xmp_info.derived and "uuid:" not in xmp_info.derived:
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
        doc_ids = extract_batch_doc_ids(candidates)
        for p in candidates:
            if doc_ids.get(str(p.resolve())) == xmp_info.doc_id:
                return p, "DocumentID match"

    # Strategy 5: Fuzzy match (alphanumeric only)
    norm_xmp = normalize_filename(xmp_stem)
    norm_xmp_root = normalize_filename(xmp_root_stem)
    if norm_xmp:
        for p in candidates:
            file_stem = p.stem
            norm_file = normalize_filename(file_stem)
            norm_file_root = normalize_filename(file_stem.split(".")[0])
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


# Define libc for macOS
try:
    libc = ctypes.CDLL("libc.dylib")
    libc.getattrlist.argtypes = [c_char_p, c_void_p, c_void_p, c_size_t, c_uint32]
    libc.getattrlist.restype = c_int32
    libc.setattrlist.argtypes = [c_char_p, c_void_p, c_void_p, c_size_t, c_uint32]
    libc.setattrlist.restype = c_int32
except BaseException:
    libc = None

ATTR_CMN_CRTIME = 0x00000200
ATTR_CMN_ADDEDTIME = 0x10000000
ATTR_BIT_MAP_COUNT = 5


class AttrList(Structure):
    _fields_ = [
        ("bitmapcount", c_uint16),
        ("reserved", c_uint16),
        ("commonattr", c_uint32),
        ("volattr", c_uint32),
        ("dirattr", c_uint32),
        ("fileattr", c_uint32),
        ("forkattr", c_uint32),
    ]


# The struct used to pass time TO setattrlist
class TimeSpec(Structure):
    _fields_ = [("tv_sec", c_int64), ("tv_nsec", c_int64)]


# The struct used to receive time FROM getattrlist
class AttrBufTime(Structure):
    _fields_ = [("length", c_uint32), ("time", TimeSpec)]


def get_mac_time_attr(path: str, attr_flag: int) -> TimeSpec | None:
    if sys.platform != "darwin" or not libc:
        return None
    try:
        attr_list = AttrList(ATTR_BIT_MAP_COUNT, 0, attr_flag, 0, 0, 0, 0)
        buf = AttrBufTime()
        buf.length = 0
        buf.time.tv_sec = 0
        buf.time.tv_nsec = 0

        path_bytes = path.encode("utf-8")
        ret = libc.getattrlist(
            path_bytes, byref(attr_list), byref(buf), ctypes.sizeof(buf), 0
        )
        if ret == 0:
            return buf.time
        return None
    except Exception:
        return None


def set_mac_time_attr(path: str, attr_flag: int, timespec: TimeSpec):
    if sys.platform != "darwin" or not libc or timespec is None:
        return
    try:
        attr_list = AttrList(ATTR_BIT_MAP_COUNT, 0, attr_flag, 0, 0, 0, 0)
        path_bytes = path.encode("utf-8")
        libc.setattrlist(
            path_bytes,
            byref(attr_list),
            byref(timespec),  # Pass TimeSpec directly to setattrlist
            ctypes.sizeof(timespec),
            0,
        )
    except Exception:
        pass


class FileTimestamps:
    def __init__(self, stat_result):
        self.atime = stat_result.st_atime
        self.mtime = stat_result.st_mtime
        self.mac_crtime = None
        self.mac_addedtime = None


def get_timestamps(path: Path) -> FileTimestamps | None:
    try:
        stat = path.stat()
        ts = FileTimestamps(stat)
        if sys.platform == "darwin":
            ts.mac_crtime = get_mac_time_attr(str(path), ATTR_CMN_CRTIME)
            ts.mac_addedtime = get_mac_time_attr(str(path), ATTR_CMN_ADDEDTIME)
        return ts
    except Exception:
        return None


def restore_timestamps(path: Path, ts: FileTimestamps | None):
    if not ts:
        return
    try:
        # 1. macOS specific: set creation time BEFORE atime/mtime
        if sys.platform == "darwin" and ts.mac_crtime:
            set_mac_time_attr(str(path), ATTR_CMN_CRTIME, ts.mac_crtime)

        # 2. Set atime and mtime
        os.utime(path, (ts.atime, ts.mtime))

        # 3. macOS specific: re-apply creation time and added time
        # (Needed because utime may reset creation time in macOS)
        if sys.platform == "darwin":
            if ts.mac_crtime:
                set_mac_time_attr(str(path), ATTR_CMN_CRTIME, ts.mac_crtime)
            if ts.mac_addedtime:
                set_mac_time_attr(str(path), ATTR_CMN_ADDEDTIME, ts.mac_addedtime)

    except Exception as e:
        print(f"  {YELLOW}⚠️  Failed to restore timestamps for {path.name}: {e}{RESET}")


def merge_xmp(xmp_path: Path, media_path: Path, strategy: str) -> bool:
    print(
        f"  {DIM}Merge [{strategy}]:{RESET} {xmp_path.name} {CYAN}➜{RESET} {media_path.name}"
    )

    # 1. Snapshot ALL deep timestamps (File & Parent Folder)
    original_file_times = get_timestamps(media_path)
    parent_dir = media_path.parent
    original_parent_times = get_timestamps(parent_dir)

    apple_compat = os.environ.get("MODERN_FORMAT_BOOST_APPLE_COMPAT") is not None
    is_jxl = media_path.suffix.lower() == ".jxl"

    cmd = [
        "exiftool",
        "-charset",
        "filename=utf8",
        "-api",
        "windowsunicode=1",
        "-api",
        "LargeFileSupport=1",
        "-q",
        "-q",
        "-m",
        "-tagsfromfile",
        "-",
        "-all:all",
        "-unsafe",
    ]

    if is_jxl and apple_compat:
        cmd.extend(
            ["-all=", "-tagsfromfile", "@", "-all:all", "-unsafe", "-icc_profile"]
        )

    cmd.extend(
        [
            "-FileModifyDate<FileModifyDate",
            "-overwrite_original",  # Explicitly back to normal behavior, our ctypes logic protects us
            str(media_path),
        ]
    )

    try:
        with open(xmp_path, "rb") as xmp_file:
            process = subprocess.Popen(
                cmd, stdin=xmp_file, stdout=subprocess.PIPE, stderr=subprocess.PIPE
            )
            stdout, stderr = process.communicate()

        if process.returncode != 0:
            err_msg = stderr.decode("utf-8", errors="replace").strip()
            is_real_error = (
                "Error:" in err_msg
                or "Error opening" in err_msg
                or "File not found" in err_msg
                or "not writing image" in err_msg
            )
            if is_real_error and "[minor]" not in err_msg.lower():
                print(f"  {RED}❌ Failed: {err_msg}{RESET}")
                return False

        # Deep macOS native parity timestamp restoration
        restore_timestamps(media_path, original_file_times)
        restore_timestamps(parent_dir, original_parent_times)

        # Finally, delete the XMP sidecar now that merge is verified and successful
        try:
            xmp_path.unlink()
        except Exception as e:
            print(
                f"  {YELLOW}⚠️  XMP merge succeeded but sidecar delete failed: {e}{RESET}"
            )

        print(f"  {GREEN}✅ Success (XMP deleted){RESET}")
        return True
    except Exception as e:
        print(f"  {RED}❌ Error executing exiftool: {e}{RESET}")
        return False


def main():
    if len(sys.argv) > 1:
        target_dir = Path(sys.argv[1]).resolve()
    else:
        print(f"{RED}❌ Error: Please provide a target directory.{RESET}")
        print("Usage: python3 crates/dev/scripts/merge_xmp.py /path/to/files")
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
            if file.lower().endswith(".xmp"):
                xmp_files.append(Path(root) / file)

    if not xmp_files:
        print(f"{YELLOW}No .xmp files found in the target directory.{RESET}")
        sys.exit(0)

    print(
        f"Found {len(xmp_files)} XMP file(s). Searching via Deep 8-Strategy Pipeline...\n"
    )

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

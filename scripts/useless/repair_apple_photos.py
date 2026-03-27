#!/usr/bin/env python3
"""Apple Photos Compatibility & Repair Tool (Python Edition)
In-place repair + Hidden backup mode.

Merges functionality from:
1. Extension fixing, EOI repair
2. Brotli fix, hidden backups, in-place edit
"""

import os
import sys
import subprocess
import shutil
import glob
from pathlib import Path

# ANSI Colors
if sys.stdout.isatty():
    RED = '\033[0;31m'
    GREEN = '\033[0;32m'
    YELLOW = '\033[1;33m'
    BLUE = '\033[0;34m'
    BOLD = '\033[1m'
    DIM = '\033[2m'
    RESET = '\033[0m'
else:
    RED = GREEN = YELLOW = BLUE = BOLD = DIM = RESET = ''

def check_dependencies():
    if not shutil.which("exiftool"):
        print(f"❌ Error: exiftool is required. Please install it (brew install exiftool).")
        sys.exit(1)

def get_real_extension(filepath):
    try:
        res = subprocess.run(["exiftool", "-s", "-S", "-FileTypeExtension", str(filepath)], capture_output=True, text=True)
        if res.returncode == 0:
            return res.stdout.strip().lower()
    except Exception:
        pass
    return ""

def get_macos_xattr(filepath, attr_name):
    try:
        res = subprocess.run(["xattr", "-px", attr_name, str(filepath)], capture_output=True, text=True)
        if res.returncode == 0:
            return res.stdout.strip()
    except Exception:
        pass
    return ""

def set_macos_xattr(filepath, attr_name, val_hex):
    try:
        subprocess.run(["xattr", "-wx", attr_name, val_hex, str(filepath)], stderr=subprocess.DEVNULL)
    except Exception:
        pass

def get_mac_times(filepath):
    try:
        mtime = os.stat(filepath).st_mtime
    except Exception:
        mtime = 0.0
    
    btime = 0.0
    try:
        res = subprocess.run(["stat", "-f%B", str(filepath)], capture_output=True, text=True)
        if res.returncode == 0:
            btime = float(res.stdout.strip())
    except Exception:
        pass
    return mtime, btime

def set_mac_times(filepath, mtime, btime):
    # Set mtime
    try:
        os.utime(filepath, (mtime, mtime))
    except Exception:
        pass

    # Set btime (birth time) via SetFile
    if btime > 0 and shutil.which("SetFile"):
        import datetime
        bdate = datetime.datetime.fromtimestamp(btime).strftime("%m/%d/%Y %H:%M:%S")
        subprocess.run(["SetFile", "-d", bdate, str(filepath)], stderr=subprocess.DEVNULL)

def get_exiftool_warnings(filepath):
    try:
        res = subprocess.run(["exiftool", "-validate", "-warning", str(filepath)], capture_output=True, text=True)
        return res.stdout + res.stderr
    except Exception:
        return ""

def rebuild_metadata(filepath):
    try:
        res = subprocess.run(["exiftool", "-quiet", "-all=", "-tagsfromfile", "@", "-all:all", "-unsafe", "-icc_profile", "-overwrite_original", str(filepath)], stderr=subprocess.DEVNULL)
        return res.returncode == 0
    except Exception:
        return False

def force_magick_repair(filepath):
    if shutil.which("magick"):
        subprocess.run(["magick", str(filepath), str(filepath)], stderr=subprocess.DEVNULL)
        return True
    return False

def main():
    check_dependencies()

    target_dir_str = sys.argv[1] if len(sys.argv) > 1 else "."
    target_dir = Path(target_dir_str).resolve()
    backup_dir = target_dir / ".apple_photos_repair_backups"

    print("╔════════════════════════════════════════════════════════════════╗")
    print("║          Apple Photos Ultimate Repair Tool 🍎                  ║")
    print("║          (In-Place Fix + Safe Hidden Backup)                   ║")
    print("╚════════════════════════════════════════════════════════════════╝\n")
    print(f"Target: {target_dir}")
    print(f"Backup: {backup_dir}\n")

    os.makedirs(backup_dir, exist_ok=True)

    # Save directory timestamps
    dir_timestamps = {}
    for d in target_dir.rglob("*"):
        if d.is_dir() and ".apple_photos_repair_backups" not in str(d):
            dir_timestamps[str(d)] = get_mac_times(d)

    total = 0
    fixed_ext = 0
    fixed_meta = 0
    failed = 0

    print("🔍 Scanning and repairing files...\n")

    file_list = []
    for root, _, files in os.walk(target_dir):
        if str(backup_dir) in root:
            continue
        for f in files:
            if not f.startswith("."):
                file_list.append(os.path.join(root, f))

    for filepath_str in file_list:
        file = Path(filepath_str)
        filename = file.name
        rel_path = file.relative_to(target_dir)
        
        ext = file.suffix.lstrip(".").lower()
        real_ext = get_real_extension(file)

        if not real_ext:
            continue

        total += 1
        needs_repair = False
        reason = ""
        is_mismatch = False
        check_meta = False

        if ext != real_ext:
            if (ext == "jpg" and real_ext == "jpeg") or (ext == "jpeg" and real_ext == "jpg"):
                is_mismatch = False
            else:
                is_mismatch = True
                needs_repair = True
                reason += f"[Bad Extension: .{ext} -> .{real_ext}] "

        if real_ext in ["jxl", "webp", "jpg", "jpeg"]:
            check_meta = True
            needs_repair = True
            
            warnings = get_exiftool_warnings(file)
            if any(x in warnings for x in ["JPEG EOI marker not found", "JPEG format error", "Corrupted Brotli"]):
                reason += "[Structure/Format Error] "
            else:
                reason += "[Deep Clean] "

            if is_mismatch:
                reason += "[Extension Mismatch] "
                
        if needs_repair:
            print(f"🔧 Fixing: {filename}")
            print(f"   Reason: {reason}")
            
            backup_subdir = backup_dir / rel_path.parent
            backup_subdir.mkdir(parents=True, exist_ok=True)
            backup_file = backup_subdir / filename
            
            shutil.copy2(file, backup_file)
            mtime, btime = get_mac_times(file)
            
            current_file = file
            
            if is_mismatch:
                new_filename = f"{file.stem}.{real_ext}"
                new_file_path = file.parent / new_filename
                file.rename(new_file_path)
                current_file = new_file_path
                print(f"   📝 Renamed to: {new_filename}")
                fixed_ext += 1
                
            if check_meta:
                if real_ext in ["jpg", "jpeg"] and any(x in warnings for x in ["JPEG EOI marker not found", "JPEG format error"]):
                    print("   🧱 Structure broken, rebuilding with ImageMagick...")
                    force_magick_repair(current_file)
                    
                if rebuild_metadata(current_file):
                    print("   ✨ Metadata Rebuilt")
                    fixed_meta += 1
                else:
                    if real_ext in ["jpg", "jpeg"]:
                        print("   ⚠️ ExifTool failed. Attempting forced structural repair with ImageMagick...")
                        force_magick_repair(current_file)
                        if rebuild_metadata(current_file):
                            print("   ✨ Metadata Rebuilt (after structural repair)")
                            fixed_meta += 1
                        else:
                            print("   ❌ Failed to rebuild metadata (check backup)")
                            failed += 1
                    else:
                        print("   ❌ ExifTool failed (check backup)")
                        failed += 1

            for attr in ["com.apple.metadata:kMDItemWhereFroms", "com.apple.metadata:_kMDItemUserTags", "com.apple.FinderInfo", "com.apple.metadata:kMDItemDateAdded"]:
                val = get_macos_xattr(backup_file, attr)
                if val:
                    set_macos_xattr(current_file, attr, val)
                    
            set_mac_times(current_file, mtime, btime)
            print("   ✅ Done\n")

    print(f"{DIM}🗂️  Restoring directory timestamps...{RESET}")
    # Restore dir timestamps ordered by depth (deepest first) to not overwrite parent times
    for d in sorted(dir_timestamps.keys(), key=lambda x: x.count(os.sep), reverse=True):
        mtime, btime = dir_timestamps[d]
        set_mac_times(Path(d), mtime, btime)

    print("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
    print("📊 Summary")
    print("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
    print(f"  Total Scanned: {total}")
    print(f"  Extensions Fixed: {fixed_ext}")
    print(f"  Metadata Rebuilt: {fixed_meta}")
    print("\n✅ Repairs complete.")
    print(f"📦 Originals backed up in: {backup_dir}\n")

    print(f"{DIM}Press Enter to return to menu...{RESET}")
    try:
        input()
    except (EOFError, KeyboardInterrupt):
        pass

if __name__ == "__main__":
    main()

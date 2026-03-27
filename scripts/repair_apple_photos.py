#!/usr/bin/env python3
"""Apple Photos Compatibility & Repair Tool (Strict Parity Edition)"""

import sys
import subprocess
import shutil
import os
from pathlib import Path
from rich.console import Console
from rich.panel import Panel

console = Console()

class RepairStats:
    def __init__(self):
        self.total = 0
        self.fixed_ext = 0
        self.fixed_meta = 0
        self.failed = 0

class DirTimestampManager:
    """对应 Bash 版的 save/restore_dir_timestamps"""
    def __init__(self, target_dir):
        self.target_dir = target_dir
        self.timestamps = {}

    def save(self):
        for root, dirs, _ in os.walk(self.target_dir):
            for d in dirs:
                dir_path = Path(root) / d
                try:
                    stat = dir_path.stat()
                    self.timestamps[dir_path] = (stat.st_atime, stat.st_mtime)
                except Exception:
                    pass
        # Save root dir as well
        try:
            stat = self.target_dir.stat()
            self.timestamps[self.target_dir] = (stat.st_atime, stat.st_mtime)
        except Exception:
            pass

    def restore(self):
        for dir_path, times in self.timestamps.items():
            try:
                os.utime(dir_path, times)
            except Exception:
                pass

def get_real_extension(file_path):
    try:
        result = subprocess.run(
            ["exiftool", "-s", "-S", "-FileTypeExtension", str(file_path)],
            capture_output=True, text=True, timeout=5
        )
        return result.stdout.strip().lower() if result.returncode == 0 else ""
    except Exception:
        return ""

def get_exif_warnings(file_path):
    try:
        result = subprocess.run(
            ["exiftool", "-validate", "-warning", str(file_path)],
            capture_output=True, text=True, timeout=5
        )
        return result.stdout + result.stderr
    except Exception:
        return ""

def process_file(file_path, target_dir, backup_dir, stats):
    filename = file_path.name
    
    # Get current extension
    ext = file_path.suffix.lower().lstrip('.')
    real_ext = get_real_extension(file_path)

    if not real_ext:
        return

    stats.total += 1
    needs_repair = False
    is_mismatch = False
    check_meta = False
    reason_tags = []

    # Check 1: Extension Mismatch
    if ext != real_ext:
        if not ((ext == "jpg" and real_ext == "jpeg") or (ext == "jpeg" and real_ext == "jpg")):
            is_mismatch = True
            needs_repair = True
            reason_tags.append(f"Bad Extension: .{ext} -> .{real_ext}")

    # Check 2: Metadata Corruption / "Nuclear Rebuild" Candidates
    warnings_output = ""
    if real_ext in ("jxl", "webp", "jpg", "jpeg"):
        check_meta = True
        needs_repair = True
        
        warnings_output = get_exif_warnings(file_path)
        if any(x in warnings_output for x in ["JPEG EOI marker not found", "JPEG format error", "Corrupted Brotli"]):
            reason_tags.append("Structure/Format Error")
        else:
            reason_tags.append("Deep Clean")
            
        if is_mismatch:
            reason_tags.append("Extension Mismatch")

    if not needs_repair:
        return

    console.print(f"[yellow]🔧 Fixing:[/yellow] {filename}")
    console.print(f"   [dim]Reason: [{' | '.join(reason_tags)}][/dim]")

    # Prepare Backup Path
    rel_path = file_path.relative_to(target_dir)
    backup_file = backup_dir / rel_path
    backup_file.parent.mkdir(parents=True, exist_ok=True)
    
    # Copy to backup (preserving attributes like cp -p)
    shutil.copy2(file_path, backup_file)

    # Save original timestamps (mtime and macOS btime)
    mtime = file_path.stat().st_mtime
    btime_str = "0"
    try:
        btime_result = subprocess.run(["stat", "-f%B", str(file_path)], capture_output=True, text=True)
        if btime_result.returncode == 0:
            btime_str = btime_result.stdout.strip()
    except Exception:
        pass

    current_file = file_path

    # Step A: Fix Extension
    if is_mismatch:
        new_name = file_path.stem + f".{real_ext}"
        new_path = file_path.parent / new_name
        file_path.rename(new_path)
        current_file = new_path
        console.print(f"   [green]📝 Renamed to:[/green] {new_name}")
        stats.fixed_ext += 1

    # Step B: Nuclear Metadata Rebuild
    if check_meta:
        if real_ext in ("jpg", "jpeg") and any(x in warnings_output for x in ["JPEG EOI marker not found", "JPEG format error"]):
            if shutil.which("magick"):
                console.print("   [yellow]🧱 Structure broken, rebuilding with ImageMagick...[/yellow]")
                subprocess.run(["magick", str(current_file), str(current_file)], capture_output=True)
                
        # ExifTool Rebuild
        exif_cmd = ["exiftool", "-quiet", "-all=", "-tagsfromfile", "@", "-all:all", "-unsafe", "-icc_profile", "-overwrite_original", str(current_file)]
        exif_result = subprocess.run(exif_cmd, capture_output=True)
        
        if exif_result.returncode == 0:
            console.print("   [green]✨ Metadata Rebuilt[/green]")
            stats.fixed_meta += 1
        else:
            # Fallback
            if real_ext in ("jpg", "jpeg") and shutil.which("magick"):
                console.print("   [yellow]⚠️ ExifTool failed. Attempting forced structural repair with ImageMagick...[/yellow]")
                subprocess.run(["magick", str(current_file), str(current_file)], capture_output=True)
                
                exif_retry = subprocess.run(exif_cmd, capture_output=True)
                if exif_retry.returncode == 0:
                    console.print("   [green]✨ Metadata Rebuilt (after structural repair)[/green]")
                    stats.fixed_meta += 1
                else:
                    console.print("   [red]❌ Failed to rebuild metadata (check backup)[/red]")
                    stats.failed += 1
            else:
                console.print("   [red]❌ ExifTool failed (check backup)[/red]")
                stats.failed += 1

    # Step C: Restore Timestamps & Attributes (macOS specific)
    mac_attrs = [
        "com.apple.metadata:kMDItemWhereFroms",
        "com.apple.metadata:_kMDItemUserTags",
        "com.apple.FinderInfo",
        "com.apple.metadata:kMDItemDateAdded"
    ]
    for attr in mac_attrs:
        try:
            val = subprocess.run(["xattr", "-px", attr, str(backup_file)], capture_output=True, text=True).stdout.strip()
            if val:
                subprocess.run(["xattr", "-wx", attr, val, str(current_file)], capture_output=True)
        except Exception:
            pass

    # Restore mtime
    try:
        mtime_str = subprocess.check_output(["date", "-r", str(int(mtime)), "+%Y%m%d%H%M.%S"]).decode().strip()
        subprocess.run(["touch", "-mt", mtime_str, str(current_file)], capture_output=True)
    except Exception:
        pass
        
    # Restore btime (Creation time)
    if btime_str != "0":
        try:
            btime_formatted = subprocess.check_output(["date", "-r", btime_str, "+%m/%d/%Y %H:%M:%S"]).decode().strip()
            subprocess.run(["SetFile", "-d", btime_formatted, str(current_file)], capture_output=True)
        except Exception:
            pass

    console.print("   [green]✅ Done[/green]\n")

def main():
    target_dir = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    backup_dir = target_dir / ".apple_photos_repair_backups"

    if not shutil.which("exiftool"):
        console.print("[red]❌ Error: exiftool is required. Please install it (brew install exiftool).[/red]")
        sys.exit(1)

    console.print(Panel.fit("🍎 Apple Photos Ultimate Repair Tool\n(In-Place Fix + Safe Hidden Backup)", style="bold blue"))
    console.print(f"\n[cyan]Target:[/cyan] {target_dir}")
    console.print(f"[cyan]Backup:[/cyan] {backup_dir}\n")

    backup_dir.mkdir(exist_ok=True)
    stats = RepairStats()
    
    # Save directory timestamps before processing
    dir_manager = DirTimestampManager(target_dir)
    dir_manager.save()

    console.print("[yellow]🔍 Scanning and repairing files...[/yellow]\n")

    for file_path in target_dir.rglob("*"):
        if not file_path.is_file():
            continue
        if ".apple_photos_repair_backups" in str(file_path):
            continue
        if file_path.name.startswith("."):
            continue

        process_file(file_path, target_dir, backup_dir, stats)

    # Restore directory timestamps after processing
    dir_manager.restore()

    console.print("[bold]━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━[/bold]")
    console.print("[bold]📊 Summary[/bold]")
    console.print("[bold]━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━[/bold]")
    console.print(f"  Total Scanned: {stats.total}")
    console.print(f"  Extensions Fixed: {stats.fixed_ext}")
    console.print(f"  Metadata Rebuilt: {stats.fixed_meta}")
    console.print(f"\n[green]✅ Repairs complete.[/green]")
    console.print(f"[blue]📦 Originals backed up in: {backup_dir}[/blue]\n")
    
    # Wait for user input (parity with Bash read -rn1)
    console.input("[dim]Press Enter to return to menu...[/dim]")

if __name__ == "__main__":
    main()
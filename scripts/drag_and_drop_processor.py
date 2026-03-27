#!/usr/bin/env python3
"""Modern Format Boost - Drag & Drop Processor (Strict Parity Edition)"""

import os
import sys
import subprocess
import time
import re
import shutil
from pathlib import Path
from datetime import datetime
from rich.console import Console
from rich.panel import Panel
from rich.prompt import Confirm, Prompt
from rich.progress import Progress, SpinnerColumn, TextColumn

console = Console()

class Config:
    def __init__(self):
        self.script_dir = Path(__file__).parent.resolve()
        self.project_root = self.script_dir.parent
        self.img_tool = self.project_root / "target/release/img-hevc"
        self.vid_tool = self.project_root / "target/release/vid-hevc"
        
        self.log_dir = self.project_root / "logs"
        self.session_time = datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
        self.log_file = self.log_dir / f"drag_drop_{self.session_time}.log"
        
        self.target_dir = None
        self.output_dir = None
        self.output_mode = "inplace"
        self.ultimate_mode = True
        self.verbose_mode = False
        
        self.stats = {"img": {"succeeded": 0, "skipped": 0, "failed": 0},
                      "vid": {"succeeded": 0, "skipped": 0, "failed": 0}}

def write_log(config, text):
    config.log_dir.mkdir(parents=True, exist_ok=True)
    with open(config.log_file, "a", encoding="utf-8") as f:
        f.write(text + "\n")

def check_tools(config):
    result = subprocess.run([config.script_dir / "smart_build.sh"], capture_output=True)
    if result.returncode != 0:
        console.print("[red]❌ Build failed. Please check the logs.[/red]")
        sys.exit(1)

def safety_check(target_dir):
    """严格对齐 Bash 版的系统保护路径"""
    path = str(target_dir.resolve())
    home = str(Path.home())
    
    exact_blocks = ["/", home, f"{home}/Desktop", f"{home}/Documents"]
    prefix_blocks = ["/System", "/usr", "/bin", "/sbin"]
    
    if path in exact_blocks or any(path.startswith(p) for p in prefix_blocks):
        console.print("\n[bold red]⚠️  SAFETY BLOCK[/bold red]")
        console.print("   System or root directories cannot be processed directly.")
        sys.exit(1)

def get_target_directory(config):
    if not config.target_dir:
        console.print("[cyan]📂 Waiting for input...[/cyan]")
        console.print("[dim]   Please drag and drop a folder here, then press Enter.[/dim]")
        path = Prompt.ask("   [bold]>[/bold] ").strip().strip('"').strip("'").strip()
        config.target_dir = Path(path)

    if not config.target_dir.is_dir():
        console.print(f"\n[red]❌ Error: Directory not found.[/red]\n[dim]   Path: {config.target_dir}[/dim]")
        sys.exit(1)
        
    safety_check(config.target_dir)

def select_mode(config):
    """严格对齐 Bash 版的死循环菜单，执行 3、4 后返回菜单"""
    while True:
        console.clear()
        console.print(Panel.fit("🚀 MODERN FORMAT BOOST", style="bold blue"))
        console.print("\n[bold]Select Operation Mode:[/bold]\n")
        
        options = [
            ("1", "📂 Output to Adjacent Folder", "Safe mode. Keeps originals untouched."),
            ("2", "🚀 In-Place Optimization", "Replaces original files. Saves disk space."),
            ("3", "🩹 Fix iCloud Import Errors", "Fix corrupted Brotli EXIF metadata that prevents iCloud Photos import."),
            ("4", "🧹 Purge Processing Data", "Clear analysis cache, session logs, and ALL resume progress.")
        ]
        
        for num, title, desc in options:
            console.print(f"{num}. {title}\n   [dim]{desc}[/dim]\n")

        choice = Prompt.ask("Choice", choices=["1", "2", "3", "4"], default="2")

        if choice == "1":
            config.output_mode = "adjacent"
            config.output_dir = config.target_dir.parent / f"{config.target_dir.name}_optimized"
            console.print(f"\n[green]✅ ADJACENT MODE SELECTED[/green]")
            console.print(f"   Output: [dim]{config.output_dir}[/dim]")
            console.print("   [dim]Creating directory structure...[/dim]")
            create_directory_structure(config.target_dir, config.output_dir)
            break
            
        elif choice == "2":
            config.output_mode = "inplace"
            console.print(f"\n[yellow]⚠️  IN-PLACE MODE SELECTED[/yellow]")
            console.print("[dim]   Original files will be replaced after successful conversion.[/dim]")
            if Confirm.ask("   [bold]Are you sure?[/bold]"):
                break
            else:
                sys.exit(0)
                
        elif choice == "3":
            console.print(f"\n[magenta]🩹 ICLOUD IMPORT FIX MODE[/magenta]")
            script = config.script_dir / "repair_apple_photos.py"
            if script.exists():
                subprocess.run([script, str(config.target_dir)])
            console.print("\n[green]✅ Brotli EXIF Fix Completed[/green]\n")
            Prompt.ask("[dim]Press Enter to return to menu...[/dim]")
            continue # 返回主菜单
            
        elif choice == "4":
            console.print(f"\n[red]🔥 DATA PURGE MODE[/red]")
            script = config.script_dir / "cache_cleaner.py"
            if script.exists():
                subprocess.run([script])
            Prompt.ask("[dim]Press Enter to return to menu...[/dim]")
            continue # 返回主菜单

def create_directory_structure(src, dest):
    dest.mkdir(parents=True, exist_ok=True)
    shutil.copystat(src, dest)
    for dirpath, dirnames, _ in os.walk(src):
        for dirname in dirnames:
            src_dir = Path(dirpath) / dirname
            dest_dir = dest / src_dir.relative_to(src)
            dest_dir.mkdir(exist_ok=True)
            shutil.copystat(src_dir, dest_dir)

def count_files(config):
    console.print("\n[dim]── [bold white]Scanning Content[/bold white] ──────────────────────────────────────────────────[/dim]")
    
    img_exts = {".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".webp", ".heic", ".heif", ".avif", ".gif", ".tiff", ".tif", ".bmp"}
    vid_exts = {".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".wmv", ".flv"}
    
    total = img_count = vid_count = xmp_count = 0
    for f in config.target_dir.rglob("*"):
        if f.is_file() and not f.name.startswith("."):
            total += 1
            ext = f.suffix.lower()
            if ext in img_exts: img_count += 1
            elif ext in vid_exts: vid_count += 1
            elif ext == ".xmp": xmp_count += 1
            
    other_count = total - img_count - vid_count - xmp_count

    console.print(f"   📁 Total Files: [bold]{total}[/bold]")
    console.print(f"   🖼️  Images:      [bold cyan]{img_count}[/bold cyan]")
    console.print(f"   🎬 Videos:      [bold magenta]{vid_count}[/bold magenta]")
    console.print(f"   📋 Metadata:    [bold dim]{xmp_count}[/bold dim]")
    console.print(f"   📦 Others:      [bold dim]{other_count}[/bold dim] (Copy only)\n")
    return img_count, vid_count

def check_disk_space(config):
    """严格对齐 Bash 版：输入媒体总大小 + 1GB Headroom"""
    img_exts = {".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".webp", ".heic", ".heif", ".avif", ".gif", ".tiff", ".tif", ".bmp"}
    vid_exts = {".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".wmv", ".flv"}
    
    total_bytes = sum(f.stat().st_size for f in config.target_dir.rglob("*") if f.is_file() and f.suffix.lower() in (img_exts | vid_exts))
    
    check_path = config.output_dir if config.output_mode == "adjacent" else config.target_dir
    while not check_path.exists():
        check_path = check_path.parent

    avail_bytes = shutil.disk_usage(check_path).free
    required = total_bytes + (1024 * 1024 * 1024)

    if avail_bytes < required:
        console.print(f"[red]❌ Insufficient disk space[/red]")
        console.print(f"   Available: {avail_bytes // (1024**3)} GB")
        console.print(f"   Required:  {required // (1024**3)} GB")
        sys.exit(1)
        
    os.environ["MFB_SKIP_DISK_PRECHECK"] = "1"

def parse_stats(output, tool_type, config):
    if m := re.search(r'Succeeded:\s*(\d+)', output): config.stats[tool_type]["succeeded"] = int(m.group(1))
    if m := re.search(r'Skipped:\s*(\d+)', output): config.stats[tool_type]["skipped"] = int(m.group(1))
    if m := re.search(r'Failed:\s*(\d+)', output): config.stats[tool_type]["failed"] = int(m.group(1))

def process_media(tool, args, tool_type, config):
    cmd = [str(tool)] + args
    result = subprocess.run(cmd, capture_output=True, text=True)
    
    write_log(config, result.stdout)
    if result.stderr: write_log(config, result.stderr)

    if result.returncode == 130:
        sys.exit(130)
    elif result.returncode != 0:
        sys.exit(result.returncode)

    parse_stats(result.stdout + result.stderr, tool_type, config)

def process_images(config, img_count):
    if img_count == 0: return
    console.print("[dim]── [bold white]Processing Images[/bold white] ─────────────────────────────────────────────────[/dim]")
    args = ["run", "--recursive", "--allow-size-tolerance"]
    if config.ultimate_mode: args.append("--ultimate")
    if config.verbose_mode: args.append("--verbose")

    if config.output_mode == "inplace": args.extend(["--in-place", str(config.target_dir)])
    else: args.extend([str(config.target_dir), "--output", str(config.output_dir)])

    with Progress(SpinnerColumn(), TextColumn("[progress.description]{task.description}")) as progress:
        progress.add_task("Processing Images...", total=None)
        process_media(config.img_tool, args, "img", config)

def process_videos(config, vid_count):
    if vid_count == 0: return
    console.print("[dim]── [bold white]Processing Videos[/bold white] ─────────────────────────────────────────────────[/dim]")
    args = ["run", "--recursive", "--allow-size-tolerance"]
    if config.ultimate_mode: args.append("--ultimate")
    if config.verbose_mode: args.append("--verbose")

    if config.output_mode == "inplace": args.extend(["--in-place", str(config.target_dir)])
    else: args.extend([str(config.target_dir), "--output", str(config.output_dir)])

    with Progress(SpinnerColumn(), TextColumn("[progress.description]{task.description}")) as progress:
        progress.add_task("Processing Videos...", total=None)
        process_media(config.vid_tool, args, "vid", config)

def sync_non_media_files(config):
    """严格对齐 Bash 版的 rsync 与 --exclude 逻辑"""
    console.print("\n[dim]── [bold white]Syncing Non-Media Files[/bold white] ───────────────────────────────────────────[/dim]")
    
    excludes = [
        "--exclude=*.[jJ][pP][gG]", "--exclude=*.[jJ][pP][eE][gG]", "--exclude=*.[pP][nN][gG]", "--exclude=*.[wW][eE][bB][pP]",
        "--exclude=*.[hH][eE][iI][cC]", "--exclude=*.[hH][eE][iI][fF]", "--exclude=*.[aA][vV][iI][fF]", "--exclude=*.[gG][iI][fF]",
        "--exclude=*.[tT][iI][fF]", "--exclude=*.[tT][iI][fF][fF]", "--exclude=*.[jJ][pP][eE]", "--exclude=*.[jJ][fF][iI][fF]",
        "--exclude=*.[bB][mM][pP]", "--exclude=*.[jJ][xX][lL]",
        "--exclude=*.[mM][pP]4", "--exclude=*.[mM][oO][vV]", "--exclude=*.[mM][kK][vV]", "--exclude=*.[aA][vV][iI]",
        "--exclude=*.[wW][eE][bB][mM]", "--exclude=*.[mM]4[vV]", "--exclude=*.[wW][mM][vV]", "--exclude=*.[fF][lL][vV]",
        "--exclude=*.[xX][mM][pP]"
    ]
    
    rsync_cmd = "/opt/homebrew/opt/rsync/bin/rsync" if os.path.exists("/opt/homebrew/opt/rsync/bin/rsync") else "rsync"
    cmd = [rsync_cmd, "-av", "--ignore-existing"] + excludes + [f"{config.target_dir}/", f"{config.output_dir}/"]
    
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    console.print("   [green]✅ Non-media files synced.[/green]")
    
    # 严格对齐 Bash: 调用 Rust 后端恢复时间戳
    if config.img_tool.exists():
        result = subprocess.run([str(config.img_tool), "restore-timestamps", str(config.target_dir), str(config.output_dir)], capture_output=True)
        if result.returncode == 0:
            console.print("   [green]✅ Timestamps restored.[/green]")

def show_summary(config, elapsed):
    console.print("\n[dim]── [bold white]Task Completed[/bold white] ────────────────────────────────────────────────────[/dim]")
    
    img, vid = config.stats["img"], config.stats["vid"]
    total_success = img["succeeded"] + vid["succeeded"]
    total_skip = img["skipped"] + vid["skipped"]
    total_fail = img["failed"] + vid["failed"]
    total = total_success + total_skip + total_fail

    console.print("   [green]✅ Optimization Finished Successfully[/green]\n")
    console.print("   [bold]📊 Merged Statistics Report[/bold]")
    console.print("   [dim]───────────────────────────────────[/dim]")
    if img["succeeded"] + img["skipped"] + img["failed"] > 0:
        console.print(f"   [cyan]🖼️  Images:[/cyan] [green]{img['succeeded']}[/green] succeeded, [yellow]{img['skipped']}[/yellow] skipped, [red]{img['failed']}[/red] failed")
    if vid["succeeded"] + vid["skipped"] + vid["failed"] > 0:
        console.print(f"   [magenta]🎬 Videos:[/magenta] [green]{vid['succeeded']}[/green] succeeded, [yellow]{vid['skipped']}[/yellow] skipped, [red]{vid['failed']}[/red] failed")
    console.print("   [dim]───────────────────────────────────[/dim]")
    console.print(f"   [white]📦 Total:[/white]  [green]{total_success}[/green] succeeded, [yellow]{total_skip}[/yellow] skipped, [red]{total_fail}[/red] failed")

    if total > 0:
        console.print(f"   [white]📈 Success Rate:[/white] [green]{(total_success * 100) // total}%[/green]")

    if config.output_mode == "adjacent":
        console.print(f"\n   [blue]📂 Output: {config.output_dir}[/blue]")

    console.print(f"\n   Total time: {int(elapsed)}s")

def main():
    config = Config()

    for arg in sys.argv[1:]:
        if arg == "--ultimate": config.ultimate_mode = True
        elif arg in ("--verbose", "-v"): config.verbose_mode = True
        else: config.target_dir = Path(arg)

    check_tools(config)
    get_target_directory(config)
    select_mode(config)

    img_count, vid_count = count_files(config)

    if img_count > 0 or vid_count > 0:
        check_disk_space(config)

    start = time.time()
    process_images(config, img_count)
    process_videos(config, vid_count)

    if config.output_mode == "adjacent":
        sync_non_media_files(config)

    elapsed = time.time() - start
    show_summary(config, elapsed)

if __name__ == "__main__":
    main()
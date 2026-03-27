#!/usr/bin/env python3
"""Modern Format Boost - Drag & Drop Processor v7.0 (Python Edition)
Usage: Drag folder onto this script or double-click to select
"""

import os
import sys
import re
import time
import subprocess
import shutil
import threading
import datetime
from pathlib import Path

try:
    import psutil
    from rich.console import Console
    from rich.panel import Panel
    from rich.table import Table
    from watchdog.observers import Observer
    from watchdog.events import FileSystemEventHandler
    console = Console()
except ImportError:
    class DummyConsole:
        def print(self, *args, **kwargs):
            if 'style' in kwargs: # Basic support for rich-like calls
                print(*args)
            else:
                print(*args, **kwargs)
    console = DummyConsole()

# Basic ANSI
if sys.stdout.isatty():
    RED = '\033[0;31m'
    GREEN = '\033[0;32m'
    YELLOW = '\033[1;33m'
    BLUE = '\033[0;34m'
    MAGENTA = '\033[0;35m'
    CYAN = '\033[0;36m'
    WHITE = '\033[0;37m'
    BOLD = '\033[1m'
    DIM = '\033[2m'
    RESET = '\033[0m'
else:
    RED = GREEN = YELLOW = BLUE = MAGENTA = CYAN = WHITE = BOLD = DIM = RESET = ''

SCRIPT_DIR = Path(__file__).parent.resolve()
PROJECT_ROOT = SCRIPT_DIR.parent

IMGQUALITY_HEVC = PROJECT_ROOT / "target" / "release" / "img-hevc"
VIDQUALITY_HEVC = PROJECT_ROOT / "target" / "release" / "vid-hevc"

OUTPUT_MODE = "inplace"
TARGET_DIR = ""
OUTPUT_DIR = ""
ULTIMATE_MODE = True
VERBOSE_MODE = False

IMG_SUCCEEDED = 0
IMG_SKIPPED = 0
IMG_FAILED = 0
VID_SUCCEEDED = 0
VID_SKIPPED = 0
VID_FAILED = 0

spinner_event = threading.Event()
spinner_thread = None

LOG_DIR = PROJECT_ROOT / "logs"
LOG_FILE = ""
VERBOSE_LOG_FILE = ""
SESSION_START_TIME = ""
WATCH_MODE = False

# Threading & Control
stats_lock = threading.Lock()
watch_timer = None
is_processing = False
watch_debounce_seconds = 2.0

def hide_cursor():
    sys.stdout.write('\033[?25l')
    sys.stdout.flush()

def show_cursor():
    sys.stdout.write('\033[?25h')
    sys.stdout.flush()

def clear_screen():
    sys.stdout.write('\033[2J\033[H')
    sys.stdout.flush()

def drain_stdin():
    """Flush stdin buffer to prevent accidental menu triggers"""
    import termios
    try:
        termios.tcflush(sys.stdin.fileno(), termios.TCIFLUSH)
    except Exception:
        pass

def _fmt_elapsed(t):
    t = max(0, t)
    s = t % 60
    m = (t // 60) % 60
    h = (t // 3600) % 24
    d = (t // 86400) % 7
    w = (t // (7 * 86400)) % 4
    mo = (t // (30 * 86400)) % 12
    y = t // (365 * 86400)

    if y > 0: return f"{y:02d}Y   {mo:02d}M   {w:02d}W   {d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
    if mo > 0: return f"{mo:02d}M   {w:02d}W   {d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
    if w > 0: return f"{w:02d}W   {d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
    if d > 0: return f"{d:02d}D   {h:02d}h  {m:02d}m{s:02d}s"
    if h > 0: return f"{h:02d}h  {m:02d}m{s:02d}s"
    if m > 0: return f"{m:02d}m{s:02d}s"
    return f"{s:02d}s"

def spinner_run():
    start = time.time()
    while not spinner_event.is_set():
        elapsed = int(time.time() - start)
        if sys.stdout.isatty():
            sys.stdout.write(f"\033]0;⏱ {_fmt_elapsed(elapsed)}\007")
            sys.stdout.flush()
        time.sleep(0.15)
    
    elapsed = int(time.time() - start)
    print(f"   Total time: {_fmt_elapsed(elapsed)}")
    if sys.stdout.isatty():
        sys.stdout.write("\033]0;\007")
        sys.stdout.flush()

def start_elapsed_spinner():
    global spinner_thread
    spinner_event.clear()
    spinner_thread = threading.Thread(target=spinner_run, daemon=True)
    spinner_thread.start()

def stop_elapsed_spinner():
    if spinner_thread and spinner_thread.is_alive():
        spinner_event.set()
        spinner_thread.join(timeout=1.0)

def init_log():
    global SESSION_START_TIME, LOG_FILE, VERBOSE_LOG_FILE
    # Resize terminal to 40x100 (Legacy behavior)
    if sys.stdout.isatty():
        sys.stdout.write('\033[8;40;100t')
        sys.stdout.flush()
    
    SESSION_START_TIME = datetime.datetime.now().strftime("%Y-%m-%d_%H-%M-%S")
    LOG_DIR.mkdir(parents=True, exist_ok=True)
    # Start with a generic name, we will rename it to the project name later
    LOG_FILE = LOG_DIR / f"MFB_Session_{SESSION_START_TIME}.log"
    VERBOSE_LOG_FILE = LOG_DIR / f"verbose_{SESSION_START_TIME}.log"

def rename_log_to_project():
    """Rename the current session log to include the project/folder name"""
    global LOG_FILE
    if not TARGET_DIR or not LOG_FILE.exists(): return
    
    project_name = Path(TARGET_DIR).name
    new_name = LOG_DIR / f"MFB_{project_name}_{SESSION_START_TIME}.log"
    
    try:
        # If the file is already named correctly, skip
        if LOG_FILE == new_name: return
        
        # Rename the physical file
        os.rename(LOG_FILE, new_name)
        LOG_FILE = new_name
    except Exception:
        pass

def get_branch_tag():
    try:
        if (PROJECT_ROOT / ".git").is_dir():
            res = subprocess.run(["git", "-C", str(PROJECT_ROOT), "symbolic-ref", "--short", "HEAD"], capture_output=True, text=True)
            if res.returncode == 0:
                branch = res.stdout.strip()
            else:
                res = subprocess.run(["git", "-C", str(PROJECT_ROOT), "rev-parse", "--short", "HEAD"], capture_output=True, text=True)
                branch = res.stdout.strip()

            if branch == "nightly":
                return f" {BOLD}{MAGENTA}[NIGHTLY]{RESET}"
            elif branch == "main":
                return f" {BOLD}{CYAN}[MAIN]{RESET}"
            elif branch:
                return f" {DIM}[{branch}]{RESET}"
    except Exception:
        pass
    return ""

def draw_header():
    width = 70
    tag = get_branch_tag()
    
    version = "x.x.x"
    try:
        with open(PROJECT_ROOT / "Cargo.toml") as f:
            for line in f:
                if line.startswith("version ="):
                    version = line.split('"')[1]
                    break
    except Exception:
        pass

    title = f"🚀 MODERN FORMAT BOOST v{version}"
    
    if 'Panel' in globals():
        console.print(Panel(
            f"[bold #ffffff]{title}[/bold #ffffff]{tag}\n"
            f"[#888888]PREMIUM MEDIA OPTIMIZER[/#888888]\n"
            f"[#00ff00]●[/#00ff00] [#aaaaaa]No Data Loss[/#aaaaaa]   [#00ff00]●[/#00ff00] [#aaaaaa]Smart Conversion[/#aaaaaa]   [#00ff00]●[/#00ff00] [#aaaaaa]Auto-Repair[/#aaaaaa]",
            title="[bold #00aaff]Modern Format Boost[/bold #00aaff]",
            subtitle="[dim]Secure & High-Precision Pipeline[/dim]",
            expand=False,
            padding=(0, 4),
            border_style="#444444"
        ))
    else:
        padding = (width - len(title)) // 2
        print(f"\n{BLUE}╭{'─'*70}╮{RESET}")
        print(f"{BLUE}│{RESET}{' '*padding}{BOLD}{WHITE}{title}{RESET}{tag}{' '*((width-len(title)-8)//2)}{BLUE}│{RESET}")
        print(f"{BLUE}│{'─'*70}│{RESET}")
        print(f"{BLUE}│{RESET}  {DIM}PREMIUM MEDIA OPTIMIZER{RESET}{' '*(69-25)}{BLUE}│{RESET}")
        print(f"{BLUE}│{RESET}  {GREEN}●{RESET} {DIM}No Data Loss{RESET}   {GREEN}●{RESET} {DIM}Smart Conversion{RESET}   {GREEN}●{RESET} {DIM}Auto-Repair{' '*(69-58)}{BLUE}│{RESET}")
        print(f"{BLUE}╰{'─'*70}╯{RESET}")
    print(f"   {RED}⚠️  WARNING: Always keep a backup of your original media before optimization.{RESET}\n")

def check_tools():
    build_script = SCRIPT_DIR / "smart_build.sh"
    if not build_script.exists():
        print(f"{RED}❌ Build script not found: {build_script}{RESET}")
        print(f"{DIM}   Please ensure you are running from the repository root.{RESET}")
        sys.exit(1)
        
    res = subprocess.run([str(build_script)])
    if res.returncode != 0:
        print(f"{RED}❌ Build failed. Please check the logs.{RESET}")
        input("Press Enter to exit...")
        sys.exit(1)

def draw_separator(title):
    print(f"{DIM}── {BOLD}{WHITE}{title}{RESET} {DIM}{'─'*50}{RESET}\n")

def get_target_directory():
    global TARGET_DIR
    if not TARGET_DIR and not os.environ.get("FROM_APP"):
        draw_header()
        print(f"{CYAN}📂 Waiting for input...{RESET}")
        print(f"{DIM}   Please drag and drop a folder here, then press Enter.{RESET}")
        drain_stdin()
        TARGET_DIR = input(f"   {BOLD}> {RESET}").strip()
        TARGET_DIR = TARGET_DIR.strip("\"'")

    if '\n' in TARGET_DIR or '\r' in TARGET_DIR:
        print(f"\n{RED}❌ Error: Path contains unsupported control characters.{RESET}")
        sys.exit(1)

    p = Path(TARGET_DIR)
    if not p.is_dir():
        print(f"\n{RED}❌ Error: Directory not found.{RESET}")
        print(f"{DIM}   Path: {TARGET_DIR}{RESET}")
        sys.exit(1)

def safety_check():
    try:
        # Standardize path to avoid bypasses and ensure correct matching
        s = str(Path(TARGET_DIR).resolve())
    except Exception:
        s = str(TARGET_DIR)

    # System roots: block the directory and all its subdirectories
    system_unsafe = ["/", "/System", "/usr", "/bin", "/sbin"]
    for p in system_unsafe:
        if s == p or s.startswith(p + "/"):
            print(f"\n{RED}⚠️  SAFETY BLOCK{RESET}")
            print(f"   System or root directories cannot be processed directly.")
            sys.exit(1)

    # User roots: block only the directory itself, allow subdirectories
    user_unsafe = [str(Path.home()), str(Path.home() / "Desktop"), str(Path.home() / "Documents")]
    for p in user_unsafe:
        if s == p:
            print(f"\n{RED}⚠️  SAFETY BLOCK{RESET}")
            print(f"   Common user folders cannot be processed directly. Please process a subdirectory.")
            sys.exit(1)

def read_key():
    import tty, termios
    fd = sys.stdin.fileno()
    old_settings = termios.tcgetattr(fd)
    try:
        tty.setraw(sys.stdin.fileno())
        ch = sys.stdin.read(1)
        if ch == '\x1b':
            ch += sys.stdin.read(2)
        return ch
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old_settings)

def select_mode():
    global OUTPUT_MODE, OUTPUT_DIR
    selected = 0
    hide_cursor()

    options = [
        "📂 Output to Adjacent Folder",
        "🚀 In-Place Optimization",
        "🧹 Cleanup Cache & Logs"
    ]
    descriptions = [
        "Safe mode. Keeps originals untouched.",
        "Replaces original files. Saves disk space.",
        "Clear analysis cache, session logs, and ALL task progress."
    ]

    while True:
        clear_screen()
        draw_header()
        print(f"{BOLD}Select Operation Mode:{RESET}\n")

        for i, opt in enumerate(options):
            if i == selected:
                if 'Console' in globals():
                    console.print(f"  [bold #00aaff]➜[/bold #00aaff] [reverse #00aaff] {opt} [/reverse #00aaff]")
                    console.print(f"     [#00ccff]{descriptions[i]}[/#00ccff]\n")
                else:
                    print(f"  {CYAN}➜ {BOLD}{opt}{RESET}")
                    print(f"    {CYAN}{DIM}{descriptions[i]}{RESET}\n")
            else:
                if 'Console' in globals():
                    console.print(f"     [dim]○ {opt}[/dim]")
                    console.print(f"     [dim]{descriptions[i]}[/dim]\n")
                else:
                    print(f"    {DIM}○ {opt}{RESET}")
                    print(f"    {DIM}{descriptions[i]}{RESET}\n")

        print(f"{DIM}(Use ↑/↓ to navigate, Enter to select, q to quit){RESET}")

        if sys.stdin.isatty():
            key = read_key()
            if key in ('\x1b[A', '\x1b[D'):  # Up / Left
                selected = (selected - 1) % len(options)
            elif key in ('\x1b[B', '\x1b[C'):  # Down / Right
                selected = (selected + 1) % len(options)
            elif key in ('\r', '\n'):
                break
            elif key.lower() == 'q':
                show_cursor()
                sys.exit(0)
        else:
            # Fallback if not interactive
            selected = 0
            break

    show_cursor()

    if selected == 0:
        OUTPUT_MODE = "adjacent"
        tdir = Path(TARGET_DIR).resolve()
        OUTPUT_DIR = str(tdir.parent / (tdir.name + "_optimized"))
        print(f"\n{GREEN}✅ ADJACENT MODE SELECTED{RESET}")
        print(f"   Output: {DIM}{OUTPUT_DIR}{RESET}")
        print(f"   {DIM}Creating directory structure...{RESET}")
        create_directory_structure(TARGET_DIR, OUTPUT_DIR)
    elif selected == 1:
        OUTPUT_MODE = "inplace"
        print(f"\n{YELLOW}⚠️  IN-PLACE MODE SELECTED{RESET}")
        print(f"{DIM}   Original files will be replaced after successful conversion.{RESET}")
        drain_stdin()
        confirm = input(f"   {BOLD}Type 'yes' to confirm in-place optimization (yes/N): {RESET}")
        if confirm.lower() != 'yes':
            print(f"\n{RED}❌ In-place optimization cancelled.{RESET}")
            sys.exit(0)
    elif selected == 2:
        OUTPUT_MODE = "cache_clean"
        print(f"\n{RED}🧹 CACHE & LOG CLEANUP MODE{RESET}")
        print(f"{DIM}   Analysis cache and ALL task progress will be permanently deleted.{RESET}\n")

def create_directory_structure(src, dest):
    """Create directory structure and preserve timestamps"""
    src_path = Path(src)
    dest_path = Path(dest)
    dest_path.mkdir(parents=True, exist_ok=True)
    try:
        shutil.copystat(src_path, dest_path)
    except Exception:
        pass

    for root, dirs, _ in os.walk(src):
        for d in dirs:
            src_dir = Path(root) / d
            rel = src_dir.relative_to(src_path)
            dest_dir = dest_path / rel
            dest_dir.mkdir(parents=True, exist_ok=True)
            try:
                shutil.copystat(src_dir, dest_dir)
            except Exception:
                pass

IMG_COUNT = 0
VID_COUNT = 0
MEDIA_TOTAL_SIZE = 0

def count_files():
    global IMG_COUNT, VID_COUNT, MEDIA_TOTAL_SIZE
    draw_separator("Scanning Content")
    print(f"{DIM}   Analyzing directory structure...{RESET}")

    total, img, vid, xmp, media_size = 0, 0, 0, 0, 0
    img_exts = {".jpg", ".jpeg", ".jpe", ".jfif", ".png", ".webp", ".heic", ".heif", ".avif", ".gif", ".tiff", ".tif", ".bmp"}
    vid_exts = {".mp4", ".mov", ".mkv", ".avi", ".webm", ".m4v", ".wmv", ".flv"}
    media_exts = img_exts | vid_exts

    for root, _, files in os.walk(TARGET_DIR):
        for file in files:
            if file.startswith("."): continue
            total += 1
            p = Path(root) / file
            ext = p.suffix.lower()
            if ext in img_exts: img += 1
            elif ext in vid_exts: vid += 1
            elif ext == ".xmp": xmp += 1
            if ext in media_exts:
                try: media_size += p.stat().st_size
                except OSError: pass

    other = total - img - vid - xmp
    
    # Lock ONLY for the final state update to avoid blocking file detection threads
    with stats_lock:
        IMG_COUNT, VID_COUNT, MEDIA_TOTAL_SIZE = img, vid, media_size

    print(f"   📁 Total Files: {BOLD}{total}{RESET}")
    print(f"   🖼️  Images:      {BOLD}{CYAN}{img}{RESET}")
    print(f"   🎬 Videos:      {BOLD}{MAGENTA}{vid}{RESET}")
    print(f"   📋 Metadata:    {BOLD}{DIM}{xmp}{RESET}")
    print(f"   📦 Others:      {BOLD}{DIM}{other}{RESET} (Copy only)\n")

def check_system_resources(check_dir):
    """Safety checks for disk space, memory, and CPU load"""
    try:
        if 'psutil' in globals():
            # Detailed Disk Check
            usage = psutil.disk_usage(check_dir)
            free_gb = usage.free / (1024**3)
            required_gb = (MEDIA_TOTAL_SIZE / (1024**3)) + 1.0 # Buffer
            
            if free_gb < required_gb:
                console.print(f"[bold red]❌ Error: Insufficient disk space on {check_dir}[/bold red]")
                console.print(f"   Available: {free_gb:.2f} GB, Required: {required_gb:.2f} GB")
                sys.exit(1)
            
            # Memory Check
            mem = psutil.virtual_memory()
            if mem.percent > 95:
                console.print(f"[bold yellow]⚠️  Caution: System memory is very low ({mem.percent}% used).[/bold yellow]")

            # CPU Check
            cpu = psutil.cpu_percent(interval=0.1)
            if cpu > 90:
                console.print(f"[bold yellow]⚠️  Notice: System CPU usage is high ({cpu}%). Processing may take longer.[/bold yellow]")
        else:
            # Fallback
            free = shutil.disk_usage(check_dir).free
            required = MEDIA_TOTAL_SIZE + 1024**3
            if free < required:
                print(f"{RED}❌ Insufficient disk space{RESET}")
                sys.exit(1)

        os.environ["MFB_SKIP_DISK_PRECHECK"] = "1"
    except Exception:
        pass

def stream_and_log_process(cmd, parse_type):
    global IMG_SUCCEEDED, IMG_SKIPPED, IMG_FAILED, VID_SUCCEEDED, VID_SKIPPED, VID_FAILED
    tmp_out = ""
    res = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    lf = open(LOG_FILE, "ab") if LOG_FILE else None
    try:
        while True:
            # Zero-Interference Passthrough (Full TTY Support: Icons, \r, Colors)
            chunk = res.stdout.read(1024)
            if not chunk:
                if res.poll() is not None: break
                time.sleep(0.01) # Avoid busy-wait
                continue
            
            # Direct buffer write to preserve VT100 sequences precisely
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
            
            # Capture for stats parsing (without messing with terminal display)
            try:
                s = chunk.decode('utf-8', errors='ignore')
                tmp_out += s
            except Exception: pass
            
            if lf:
                lf.write(chunk)
                lf.flush()
    finally:
        if lf: lf.close()

    if res.returncode not in (0, 130):
        sys.exit(res.returncode)
    
    # Parse stats from trailing output
    succ = re.findall(r'Succeeded:\s*(\d+)', tmp_out)
    skip = re.findall(r'Skipped:\s*(\d+)', tmp_out)
    fail = re.findall(r'Failed:\s*(\d+)', tmp_out)
    
    s_val = int(succ[-1]) if succ else 0
    sk_val = int(skip[-1]) if skip else 0
    f_val = int(fail[-1]) if fail else 0
    
    if parse_type == "img":
        with stats_lock:
            IMG_SUCCEEDED, IMG_SKIPPED, IMG_FAILED = s_val, sk_val, f_val
    else:
        with stats_lock:
            VID_SUCCEEDED, VID_SKIPPED, VID_FAILED = s_val, sk_val, f_val

def process_images():
    if IMG_COUNT == 0: return
    draw_separator(f"Processing Images ({IMG_COUNT})")
    cmd = [str(IMGQUALITY_HEVC), "run", "--recursive", "--allow-size-tolerance"]
    if ULTIMATE_MODE: cmd.append("--ultimate")
    if VERBOSE_MODE: cmd.append("--verbose")
    
    if OUTPUT_MODE == "inplace":
        cmd.extend(["--in-place", str(TARGET_DIR)])
    else:
        cmd.extend([str(TARGET_DIR), "--output", str(OUTPUT_DIR)])

    stream_and_log_process(cmd, "img")
    print()

def process_videos():
    if VID_COUNT == 0: return
    draw_separator(f"Processing Videos ({VID_COUNT})")
    cmd = [str(VIDQUALITY_HEVC), "run", "--recursive", "--allow-size-tolerance"]
    if ULTIMATE_MODE: cmd.append("--ultimate")
    if VERBOSE_MODE: cmd.append("--verbose")
    
    if OUTPUT_MODE == "inplace":
        cmd.extend(["--in-place", str(TARGET_DIR)])
    else:
        cmd.extend([str(TARGET_DIR), "--output", str(OUTPUT_DIR)])

    stream_and_log_process(cmd, "vid")
    print()

def sync_non_media_files():
    draw_separator("Syncing Non-Media Files")
    excludes = [
        "--exclude=*.[jJ][pP][gG]", "--exclude=*.[jJ][pP][eE][gG]", "--exclude=*.[pP][nN][gG]", "--exclude=*.[wW][eE][bB][pP]",
        "--exclude=*.[hH][eE][iI][cC]", "--exclude=*.[hH][eE][iI][fF]", "--exclude=*.[aA][vV][iI][fF]", "--exclude=*.[gG][iI][fF]",
        "--exclude=*.[tT][iI][fF]", "--exclude=*.[jJ][pP][eE]", "--exclude=*.[jJ][fF][iI][fF]", "--exclude=*.[bB][mM][pP]", "--exclude=*.[jJ][xX][lL]",
        "--exclude=*.[mM][pP]4", "--exclude=*.[mM][oO][vV]", "--exclude=*.[mM][kK][vV]", "--exclude=*.[aA][vV][iI]",
        "--exclude=*.[wW][eE][bB][mM]", "--exclude=*.[mM]4[vV]", "--exclude=*.[wW][mM][vV]", "--exclude=*.[fF][lL][vV]",
        "--exclude=*.[xX][mM][pP]"
    ]
    rsync = "/opt/homebrew/opt/rsync/bin/rsync" if os.path.exists("/opt/homebrew/opt/rsync/bin/rsync") else "rsync"
    cmd = [rsync, "-av", "--ignore-existing"] + excludes + [f"{TARGET_DIR}/", f"{OUTPUT_DIR}/"]
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print(f"   {GREEN}✅ Non-media files synced.{RESET}")
    subprocess.run([str(IMGQUALITY_HEVC), "restore-timestamps", str(TARGET_DIR), str(OUTPUT_DIR)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    print(f"   {GREEN}✅ Timestamps restored.{RESET}")

def finish_log():
    if not LOG_FILE: return
    with open(LOG_FILE, "a") as f:
        f.write("\n========================================\n")
        f.write("📊 Final Statistics\n")
        f.write("========================================\n")
        f.write(f"End Time: {datetime.datetime.now().strftime('%Y-%m-%d_%H-%M-%S')}\n\n")
        f.write(f"Images:  {IMG_SUCCEEDED} succeeded, {IMG_SKIPPED} skipped, {IMG_FAILED} failed\n")
        f.write(f"Videos:  {VID_SUCCEEDED} succeeded, {VID_SKIPPED} skipped, {VID_FAILED} failed\n\n")

        tot_s = IMG_SUCCEEDED + VID_SUCCEEDED
        tot_sk = IMG_SKIPPED + VID_SKIPPED
        tot_f = IMG_FAILED + VID_FAILED
        tot_proc = tot_s + tot_sk + tot_f

        f.write(f"Total:   {tot_s} succeeded, {tot_sk} skipped, {tot_f} failed\n")
        if tot_proc > 0:
            f.write(f"Success Rate: {(tot_s*100)//tot_proc}%\n")
        f.write("\n========================================\nSession completed.\n========================================\n")
    print(f"   {DIM}📝 Session log:  {LOG_FILE}{RESET}")

def merge_run_logs():
    """Merge internal tool run logs into the session bundle and cleanup fragments with high precision"""
    if not LOG_FILE or not SESSION_START_TIME: return

    # Convert session start time string back to datetime for comparison
    try:
        session_dt = datetime.datetime.strptime(SESSION_START_TIME, "%Y-%m-%d_%H-%M-%S")
    except Exception:
        session_dt = datetime.datetime.fromtimestamp(os.path.getmtime(LOG_FILE))

    # Look for tool logs created DURING this session
    def is_current_session(f):
        # File must be newer than session start
        return datetime.datetime.fromtimestamp(f.stat().st_mtime) >= (session_dt - datetime.timedelta(seconds=5))

    img_logs = [f for f in LOG_DIR.glob("img_hevc_*.log") if is_current_session(f)]
    vid_logs = [f for f in LOG_DIR.glob("vid_hevc_*.log") if is_current_session(f)]

    if not img_logs and not vid_logs:
        return

    try:
        with open(LOG_FILE, "a") as mf:
            mf.write("\n" + "="*70 + "\n")
            mf.write(f"📋 ATTACHED INTERNAL TOOL LOGS (Session: {SESSION_START_TIME})\n")
            mf.write("="*70 + "\n")

            for log_path in sorted(img_logs + vid_logs, key=os.path.getmtime):
                try:
                    stats = log_path.stat()
                    mtime = datetime.datetime.fromtimestamp(stats.st_mtime).strftime('%Y-%m-%d %H:%M:%S')
                    
                    mf.write(f"\n[SOURCE: {log_path.name}] [MODIFIED: {mtime}]\n")
                    mf.write("-" * 70 + "\n")
                    
                    content = log_path.read_text(errors='ignore')
                    mf.write(content)
                    mf.write("\n" + "-" * 70 + "\n")
                    
                    # Cautious Deletion: Only delete if we successfully read the content
                    if len(content) > 0:
                        log_path.unlink()
                except Exception as e:
                    mf.write(f"\n⚠️  CRITICAL: Failed to merge/cleanup {log_path.name}: {e}\n")

            mf.write("\n" + "="*70 + "\n")
            mf.write("🏁 END OF SESSION BUNDLE\n")
            mf.write("="*70 + "\n")
    except Exception as e:
        print(f"   {RED}⚠️  Failed to merge logs: {e}{RESET}")

def main():
    global ULTIMATE_MODE, VERBOSE_MODE, WATCH_MODE, TARGET_DIR, OUTPUT_MODE, OUTPUT_DIR
    os.environ["MFB_GUI_LAUNCH"] = "1"
    os.environ["FORCE_COLOR"] = "1"
    os.environ["CLICOLOR_FORCE"] = "1"
    init_log()

    args = sys.argv[1:]
    non_flag_args = []
    
    for arg in args:
        if arg == "--ultimate": ULTIMATE_MODE = True
        elif arg in ("--verbose", "-v"): VERBOSE_MODE = True
        elif arg == "--watch": WATCH_MODE = True
        elif arg in ("--help", "-h"):
            print("Usage: drag_and_drop_processor.py [options] [target_directory]")
            print("\nOptions:")
            print("  --ultimate    Enable ultimate optimization mode")
            print("  --verbose, -v Enable verbose output")
            print("  --watch       Watch directory for new files")
            print("  --help, -h    Show this help message")
            sys.exit(0)
        else: non_flag_args.append(arg)
        
    if non_flag_args:
        TARGET_DIR = non_flag_args[0]
        
    check_tools()
    get_target_directory()
    rename_log_to_project() # Rename log once project name is known

    if not os.environ.get("FROM_APP"):
        if 'Table' in globals():
            # Dashboard Config
            table = Table(box=None, padding=(0, 2))
            table.add_column("Setting", style="dim", justify="right")
            table.add_column("Value", style="bold #00aaff")
            
            table.add_row("📂 Target Path", str(TARGET_DIR))
            table.add_row("🚀 Mode", "Ultimate" if ULTIMATE_MODE else "Standard")
            
            # System Snapshot
            if 'psutil' in globals():
                cpu = psutil.cpu_percent()
                mem = psutil.virtual_memory().percent
                table.add_row("🌡️  CPU Load", f"{cpu}%")
                table.add_row("📊 RAM Usage", f"{mem}%")
            
            console.print(Panel(table, title="[#888888]Runtime Configuration[/#888888]", border_style="#333333", expand=False))
            print()
        else:
            print()
            print(f"{CYAN}📋 Configuration:{RESET}")
            print(f"   {DIM}Target: {RESET}{BOLD}{TARGET_DIR}{RESET}")
            if ULTIMATE_MODE: print(f"   {MAGENTA}🔥 Ultimate Mode: {RESET}{GREEN}ENABLED{RESET}")
            if VERBOSE_MODE: print(f"   {CYAN}💬 Verbose: {RESET}{GREEN}ENABLED{RESET}")
            print()

    safety_check()

    while True:
        select_mode()
        
        if OUTPUT_MODE == "cache_clean":
            cache_script = SCRIPT_DIR / "cache_cleaner.py"
            subprocess.run([sys.executable, str(cache_script)])
            continue
            
        break

    count_files()

    if IMG_COUNT > 0 or VID_COUNT > 0:
        check_path = OUTPUT_DIR if OUTPUT_MODE == "adjacent" else TARGET_DIR
        check_system_resources(check_path)
        
    if WATCH_MODE:
        draw_separator("Watch Mode Enabled")
        console.print(f"[bold yellow]Monitoring:[/bold yellow] {TARGET_DIR}")
        console.print("[dim]Press Ctrl+C to stop. Debouncing active.[/dim]\n")
        
        def trigger_watch_processing():
            global is_processing, watch_timer
            with stats_lock:
                if is_processing: return
                is_processing = True
            
            try:
                count_files()
                process_images()
                process_videos()
            finally:
                with stats_lock:
                    is_processing = False
                    watch_timer = None

        class Handler(FileSystemEventHandler):
            def on_closed(self, event):
                global watch_timer
                if not event.is_directory:
                    p = Path(event.src_path)
                    # Support multiple extensions for triggering
                    if p.suffix.lower() in {".jpg", ".jpeg", ".png", ".heic", ".mp4", ".mov", ".mkv"}:
                        console.print(f"  [bold cyan]File Activity Detected:[/bold cyan] {p.name}")
                        with stats_lock:
                            if watch_timer:
                                watch_timer.cancel()
                            watch_timer = threading.Timer(watch_debounce_seconds, trigger_watch_processing)
                            watch_timer.start()
            
            def on_moved(self, event):
                global watch_timer
                if not event.is_directory:
                    p = Path(event.dest_path)
                    if p.suffix.lower() in {".jpg", ".jpeg", ".png", ".heic", ".mp4", ".mov", ".mkv"}:
                        console.print(f"  [bold cyan]File Activity Detected (Moved):[/bold cyan] {p.name}")
                        with stats_lock:
                            if watch_timer:
                                watch_timer.cancel()
                            watch_timer = threading.Timer(watch_debounce_seconds, trigger_watch_processing)
                            watch_timer.start()
        
        observer = Observer()
        observer.schedule(Handler(), str(TARGET_DIR), recursive=True)
        observer.start()
        try:
            while True: time.sleep(1)
        except KeyboardInterrupt:
            observer.stop()
        observer.join()
        sys.exit(0)

    start_elapsed_spinner()
        
    process_images()
    process_videos()
    if IMG_COUNT > 0 or VID_COUNT > 0:
        stop_elapsed_spinner()
        
    if OUTPUT_MODE == "adjacent":
        sync_non_media_files()
        
    draw_separator("Task Completed")
    
    tot_s = IMG_SUCCEEDED + VID_SUCCEEDED
    tot_sk = IMG_SKIPPED + VID_SKIPPED
    tot_f = IMG_FAILED + VID_FAILED
    tot_proc = tot_s + tot_sk + tot_f

    if 'Table' in globals():
        # Premium Rich Stats
        success_rate = (tot_s * 100) // tot_proc if tot_proc > 0 else 0
        rate_color = "green" if success_rate >= 90 else "yellow" if success_rate >= 50 else "red"
        
        table = Table(title="Optimization Summary Report", border_style="dim")
        table.add_column("Type", justify="left", style="bold #cccccc")
        table.add_column("Succeeded", justify="center", style="green")
        table.add_column("Skipped", justify="center", style="yellow")
        table.add_column("Failed", justify="center", style="red")
        
        if IMG_COUNT > 0:
            table.add_row("🖼️  Images", str(IMG_SUCCEEDED), str(IMG_SKIPPED), str(IMG_FAILED))
        if VID_COUNT > 0:
            table.add_row("🎬 Videos", str(VID_SUCCEEDED), str(VID_SKIPPED), str(VID_FAILED))
            
        table.add_section()
        table.add_row("📦 Total", f"[bold]{tot_s}[/bold]", str(tot_sk), str(tot_f))
        
        print()
        console.print(table)
        
        # Success Bar
        if tot_proc > 0:
            bar_len = 20
            filled = int((success_rate / 100) * bar_len)
            bar = "█" * filled + "░" * (bar_len - filled)
            console.print(f"   [bold #cccccc]Success Rate:[/bold #cccccc] [{rate_color}]{bar}[/{rate_color}] {success_rate}%")
        print()
    else:
        print(f"   {GREEN}✅ Optimization Finished Successfully{RESET}\n")
        print(f"   {BOLD}📊 Merged Statistics Report{RESET}")
        print(f"   {DIM}───────────────────────────────────{RESET}")
        if IMG_COUNT > 0:
            print(f"   {CYAN}🖼️  Images:{RESET} {GREEN}{IMG_SUCCEEDED}{RESET} succeeded, {YELLOW}{IMG_SKIPPED}{RESET} skipped, {RED}{IMG_FAILED}{RESET} failed")
        if VID_COUNT > 0:
            print(f"   {MAGENTA}🎬 Videos:{RESET} {GREEN}{VID_SUCCEEDED}{RESET} succeeded, {YELLOW}{VID_SKIPPED}{RESET} skipped, {RED}{VID_FAILED}{RESET} failed")
        print(f"   {DIM}───────────────────────────────────{RESET}")
        print(f"   {WHITE}📦 Total:{RESET}  {GREEN}{tot_s}{RESET} succeeded, {YELLOW}{tot_sk}{RESET} skipped, {RED}{tot_f}{RESET} failed")
        
        if tot_proc > 0:
            print(f"   {WHITE}📈 Success Rate:{RESET} {GREEN}{(tot_s*100)//tot_proc}%{RESET}\n")
        
    if OUTPUT_MODE == "adjacent":
        print(f"   {BLUE}📂 Output: {OUTPUT_DIR}{RESET}")
        try:
            subprocess.run(["open", str(OUTPUT_DIR)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception:
            pass
            
    try:
        drain_stdin()
        input(f"\n{DIM}Press Enter to exit...{RESET}")
    except (EOFError, KeyboardInterrupt):
        pass

    finish_log()
    merge_run_logs()

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        stop_elapsed_spinner()
        sys.exit(130)

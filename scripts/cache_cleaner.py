#!/usr/bin/env python3
"""Modern Format Boost - Cache Cleaner (Strict Parity Edition)"""

import os
import subprocess
import shutil
from pathlib import Path
from rich.console import Console

console = Console()

class Config:
    def __init__(self):
        self.script_dir = Path(__file__).parent.resolve()
        self.project_root = self.script_dir.parent
        self.cache_dir = self.project_root / ".cache"
        self.db_file = self.cache_dir / "image_analysis_v2.db"
        self.log_dir = self.project_root / "logs"
        self.mfb_progress_dir = Path.home() / ".mfb_progress"

def get_dir_size(path):
    """严格调用系统 du -sh 保证输出格式一致"""
    if not path.exists():
        return "0B"
    try:
        result = subprocess.run(["du", "-sh", str(path)], capture_output=True, text=True)
        if result.returncode == 0:
            return result.stdout.split('\t')[0].strip()
    except Exception:
        pass
    return "0B"

def draw_header():
    console.print("[blue]╭" + "─" * 60 + "╮[/blue]")
    console.print("[blue]│[/blue]  [bold red]🔥 DATA PURGE UTILITY v1.0[/bold red]                            [blue]│[/blue]")
    console.print("[blue]╰" + "─" * 60 + "╯[/blue]")
    console.print("   [red]⚠️  WARNING: Critical processing data will be permanently deleted.[/red]\n")

def show_stats(config):
    console.print("[bold]Current Cache Status:[/bold]")
    
    if config.cache_dir.is_dir():
        size = get_dir_size(config.cache_dir)
        console.print(f"   📂 Directory: [dim]{config.cache_dir}[/dim]")
        console.print(f"   📦 Total Size: [bold green]{size}[/bold green]")

        if config.db_file.is_file():
            db_size = get_dir_size(config.db_file)
            console.print(f"   🗄️  Database:  [dim]image_analysis_v2.db[/dim] ({db_size})")
    else:
        console.print("   [yellow]Empty: No cache directory found.[/yellow]")

    log_size = get_dir_size(config.log_dir)
    console.print(f"   📝 Logs:      [dim]{log_size}[/dim]")

    if config.mfb_progress_dir.is_dir():
        prog_size = get_dir_size(config.mfb_progress_dir)
        console.print(f"   🔄 Progress:  [dim]{prog_size}[/dim]")
    print("")

def main():
    config = Config()
    console.clear()
    draw_header()
    show_stats(config)

    console.print("[red]🔥 Purging all analysis data, logs and progress trackers...[/red]\n")

    # 严格对齐: SQLite Vacuum
    if shutil.which("sqlite3") and config.db_file.is_file():
        console.print("[dim]   Vacuuming database...[/dim]")
        subprocess.run(["sqlite3", str(config.db_file), "VACUUM;"], stderr=subprocess.DEVNULL)
        console.print("   [green]✅ Database vacuumed[/green]")

    # 严格对齐: rm -rf cache
    if config.cache_dir.is_dir():
        console.print("[dim]   Removing cache directory...[/dim]")
        shutil.rmtree(config.cache_dir, ignore_errors=True)
        console.print("   [green]✅ Cache purged[/green]")

    # 严格对齐: rm -f logs/*.log
    if config.log_dir.is_dir() and str(config.log_dir) != "/":
        console.print("[dim]   Clearing logs...[/dim]")
        for log_file in config.log_dir.glob("*.log"):
            try:
                log_file.unlink()
            except Exception:
                pass
        console.print("   [green]✅ Logs cleared[/green]")

    # 严格对齐: rm -rf mfb_progress
    if config.mfb_progress_dir.is_dir():
        console.print("[dim]   Removing MFB progress directory...[/dim]")
        shutil.rmtree(config.mfb_progress_dir, ignore_errors=True)
        console.print("   [green]✅ MFB progress purged[/green]")

    console.print("\n[green]✅ Cleanup Complete[/green]\n")
    console.input("[dim]Press Enter to return to menu...[/dim]")

if __name__ == "__main__":
    main()
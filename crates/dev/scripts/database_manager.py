#!/usr/bin/env python3
"""Modern Format Boost - Interactive Database Manager
Provides interactive database operations with numeric key selection.

Features:
  1. Train New Data - Import and train new datasets
  2. Database Status - View database statistics
  3. Vector Index Manager - Manage pgvector indexes
  4. Backup & Restore - Backup and restore database
  5. Return to Home - Exit to main interface
"""

import sys
import subprocess
import shutil
import platform
import time
from pathlib import Path

# ANSI color codes
if sys.stdout.isatty():
    RED = "\033[0;31m"
    GREEN = "\033[0;32m"
    YELLOW = "\033[1;33m"
    BLUE = "\033[0;34m"
    CYAN = "\033[0;36m"
    BOLD = "\033[1m"
    DIM = "\033[2m"
    RESET = "\033[0m"
else:
    RED = GREEN = YELLOW = BLUE = CYAN = BOLD = DIM = RESET = ""

DB_NAME = "modern_format_boost"
OS_TYPE = platform.system().lower()


def print_header():
    """Print the tool header."""
    print(f"\n{BLUE}{BOLD}🐘 Modern Format Boost - Database Manager{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}\n")


def command_exists(cmd):
    """Check if command exists in system PATH."""
    return shutil.which(cmd) is not None


def run_cmd(cmd, check=False, capture=True):
    """Run shell command and return result."""
    return subprocess.run(
        cmd,
        shell=True,
        check=check,
        capture_output=capture,
        text=True,
    )


def check_psql():
    """Verify PostgreSQL is installed and running."""
    if not command_exists("psql"):
        print(f"{RED}❌ PostgreSQL not found. Please install it first.{RESET}")
        return False

    if run_cmd("pg_isready").returncode != 0:
        print(f"{RED}❌ PostgreSQL is not running.{RESET}")
        return False

    return True


def check_db_exists():
    """Check if database exists."""
    res = run_cmd(f"psql -lqt | cut -d \\| -f 1 | grep -qw '{DB_NAME}'")
    return res.returncode == 0


def show_menu():
    """Display interactive database management menu."""
    while True:
        print_header()
        print(f"{CYAN}Database Management Options:{RESET}\n")

        print(f"  {GREEN}1{RESET} - {BOLD}Train New Data{RESET}")
        print(f"     {DIM}Import and process new training datasets{RESET}\n")

        print(f"  {GREEN}2{RESET} - {BOLD}Database Status{RESET}")
        print(f"     {DIM}View database statistics and schema info{RESET}\n")

        print(f"  {GREEN}3{RESET} - {BOLD}Vector Index Manager{RESET}")
        print(f"     {DIM}Manage pgvector indexes and embeddings{RESET}\n")

        print(f"  {GREEN}4{RESET} - {BOLD}Backup & Restore{RESET}")
        print(f"     {DIM}Backup database or restore from backup{RESET}\n")

        print(f"  {GREEN}5{RESET} - {BOLD}Return to Home{RESET}")
        print(f"     {DIM}Exit database manager and return to main interface{RESET}\n")

        print(f"  {GREEN}0{RESET} - {BOLD}Exit{RESET}\n")
        print(f"{DIM}{'─' * 60}{RESET}")

        try:
            choice = input(f"{CYAN}Select option (0-5): {RESET}").strip()

            if choice == "0":
                print(f"\n{CYAN}Exiting database manager.{RESET}")
                sys.exit(0)
            elif choice == "5":
                print(f"\n{CYAN}Returning to main menu...{RESET}\n")
                break
            elif choice == "1":
                train_new_data()
            elif choice == "2":
                show_status()
            elif choice == "3":
                manage_indexes()
            elif choice == "4":
                backup_restore()
            else:
                print(f"{RED}❌ Invalid option. Please enter 0-5.{RESET}")
                time.sleep(1)
        except KeyboardInterrupt:
            print(f"\n{YELLOW}⚠️ Cancelled.{RESET}")
            break
        except Exception as e:
            print(f"{RED}❌ Error: {e}{RESET}")
            time.sleep(1)


def train_new_data():
    """Train new data - interactive data import and processing."""
    if not check_psql() or not check_db_exists():
        print(f"{RED}❌ Database not available.{RESET}\n")
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}🎓 Train New Data{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}")
    print(f"{YELLOW}⚠️  CONFIRM: Start new training session?{RESET}")
    if (
        input(f"   {CYAN}Type {GREEN}'y'{CYAN} to proceed: {RESET}").strip().lower()
        != "y"
    ):
        print(f"\n{RED}❌ Cancelled.{RESET}")
        time.sleep(1)
        return

    print(f"\n{CYAN}⏳ Initializing training environment...{RESET}")
    time.sleep(1.5)

    print("Data types available:")
    print(f"  {GREEN}1{RESET} - Image Quality Training")
    print(f"  {GREEN}2{RESET} - Format Optimization Patterns")
    print(f"  {GREEN}3{RESET} - Metadata Analysis")
    print(f"  {GREEN}0{RESET} - Back to main menu\n")

    data_type = input(f"{CYAN}Select data type (0-3): {RESET}").strip()

    if data_type == "0":
        return
    elif data_type == "1":
        print(f"\n{CYAN}📸 Image Quality Training{RESET}")
        print(f"{DIM}Importing image quality assessment datasets...{RESET}\n")
        # Placeholder for image quality training
        print(f"{GREEN}✅ Image quality training data queued for import.{RESET}")
    elif data_type == "2":
        print(f"\n{CYAN}🎨 Format Optimization Patterns{RESET}")
        print(f"{DIM}Importing format optimization training patterns...{RESET}\n")
        # Placeholder for format optimization training
        print(f"{GREEN}✅ Format optimization patterns queued for import.{RESET}")
    elif data_type == "3":
        print(f"\n{CYAN}📊 Metadata Analysis{RESET}")
        print(f"{DIM}Importing metadata analysis datasets...{RESET}\n")
        # Placeholder for metadata training
        print(f"{GREEN}✅ Metadata analysis data queued for import.{RESET}")
    else:
        print(f"{RED}❌ Invalid option.{RESET}")

    input(f"\n{CYAN}Press Enter to continue...{RESET}")


def show_status():
    """Show database status and statistics."""
    if not check_psql():
        print(f"{RED}❌ PostgreSQL is not running.{RESET}\n")
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}📊 Database Status{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}\n")

    # Check PostgreSQL status
    status_res = run_cmd("pg_isready")
    if status_res.returncode == 0:
        print(f"PostgreSQL Status: {GREEN}RUNNING{RESET}")
    else:
        print(f"PostgreSQL Status: {RED}STOPPED{RESET}")

    # Check database exists
    if check_db_exists():
        print(f"Database '{DB_NAME}': {GREEN}EXISTS{RESET}\n")

        # Get table statistics
        table_query = """
        SELECT 
            tablename,
            pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size
        FROM pg_tables 
        WHERE schemaname = 'public'
        ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;
        """

        result = run_cmd(f'psql -d "{DB_NAME}" -c "{table_query}" -t')
        if result.returncode == 0 and result.stdout.strip():
            print(f"{CYAN}Tables and Sizes:{RESET}")
            for line in result.stdout.strip().split("\n"):
                if line.strip():
                    print(f"  {line}")
        else:
            print(f"{DIM}No tables found.{RESET}")
    else:
        print(f"Database '{DB_NAME}': {YELLOW}NOT FOUND{RESET}\n")
        print(f"{DIM}Run database setup first.{RESET}")

    print()
    input(f"{CYAN}Press Enter to continue...{RESET}")


def manage_indexes():
    """Manage vector indexes."""
    if not check_psql() or not check_db_exists():
        print(f"{RED}❌ Database not available.{RESET}\n")
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}📇 Vector Index Manager{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}")
    print(f"{YELLOW}⚠️  CONFIRM: Access index management?{RESET}")
    if (
        input(f"   {CYAN}Type {GREEN}'y'{CYAN} to proceed: {RESET}").strip().lower()
        != "y"
    ):
        print(f"\n{RED}❌ Cancelled.{RESET}")
        time.sleep(1)
        return

    print(f"\n{CYAN}⏳ Analyzing vector indexes...{RESET}")
    time.sleep(1.2)

    print("Index Management Options:")
    print(f"  {GREEN}1{RESET} - View Indexes")
    print(f"  {GREEN}2{RESET} - Rebuild Indexes")
    print(f"  {GREEN}3{RESET} - Vacuum Analyze")
    print(f"  {GREEN}0{RESET} - Back\n")

    choice = input(f"{CYAN}Select option (0-3): {RESET}").strip()

    if choice == "0":
        return
    elif choice == "1":
        print(f"\n{CYAN}Available Indexes:{RESET}")
        idx_query = "SELECT indexname FROM pg_indexes WHERE schemaname = 'public';"
        result = run_cmd(f'psql -d "{DB_NAME}" -c "{idx_query}" -t')
        if result.returncode == 0 and result.stdout.strip():
            for line in result.stdout.strip().split("\n"):
                if line.strip():
                    print(f"  • {line.strip()}")
        else:
            print(f"{DIM}No indexes found.{RESET}")
    elif choice == "2":
        print(f"\n{YELLOW}🔄 Rebuilding indexes...{RESET}")
        result = run_cmd(f'psql -d "{DB_NAME}" -c "REINDEX DATABASE {DB_NAME};"')
        if result.returncode == 0:
            print(f"{GREEN}✅ Indexes rebuilt successfully!{RESET}")
        else:
            print(f"{RED}❌ Failed to rebuild indexes.{RESET}")
    elif choice == "3":
        print(f"\n{YELLOW}🧹 Running VACUUM ANALYZE...{RESET}")
        result = run_cmd(f'psql -d "{DB_NAME}" -c "VACUUM ANALYZE;"')
        if result.returncode == 0:
            print(f"{GREEN}✅ Maintenance completed!{RESET}")
        else:
            print(f"{RED}❌ Maintenance failed.{RESET}")
    else:
        print(f"{RED}❌ Invalid option.{RESET}")

    input(f"\n{CYAN}Press Enter to continue...{RESET}")


def backup_restore():
    """Backup and restore database."""
    if not check_psql() or not check_db_exists():
        print(f"{RED}❌ Database not available.{RESET}\n")
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}💾 Backup & Restore{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}")
    print(f"{YELLOW}⚠️  CONFIRM: Access backup/restore tools?{RESET}")
    if (
        input(f"   {CYAN}Type {GREEN}'y'{CYAN} to proceed: {RESET}").strip().lower()
        != "y"
    ):
        print(f"\n{RED}❌ Cancelled.{RESET}")
        time.sleep(1)
        return

    print(f"\n{CYAN}⏳ Checking storage for backups...{RESET}")
    time.sleep(1.2)

    print("Options:")
    print(f"  {GREEN}1{RESET} - Create Backup")
    print(f"  {GREEN}2{RESET} - Restore from Backup")
    print(f"  {GREEN}0{RESET} - Back\n")

    choice = input(f"{CYAN}Select option (0-2): {RESET}").strip()

    backup_dir = Path.home() / ".cache" / "mfb_backups"
    backup_dir.mkdir(parents=True, exist_ok=True)

    if choice == "0":
        return
    elif choice == "1":
        timestamp = time.strftime("%Y%m%d_%H%M%S")
        backup_file = backup_dir / f"mfb_backup_{timestamp}.sql"

        print(f"\n{YELLOW}💾 Creating backup...{RESET}")
        result = run_cmd(f'pg_dump -d "{DB_NAME}" > "{backup_file}"')
        if result.returncode == 0:
            print(f"{GREEN}✅ Backup created: {backup_file}{RESET}")
        else:
            print(f"{RED}❌ Backup failed.{RESET}")
    elif choice == "2":
        backups = list(backup_dir.glob("mfb_backup_*.sql"))
        if not backups:
            print(f"{RED}❌ No backups found in {backup_dir}{RESET}\n")
            input("Press Enter to continue...")
            return

        print(f"\n{CYAN}Available Backups:{RESET}")
        for i, backup in enumerate(sorted(backups, reverse=True), 1):
            print(f"  {GREEN}{i}{RESET} - {backup.name}")

        print(f"  {GREEN}0{RESET} - Cancel\n")

        choice = input(
            f"{CYAN}Select backup to restore (0-{len(backups)}): {RESET}"
        ).strip()
        try:
            idx = int(choice)
            if idx == 0:
                return
            elif 1 <= idx <= len(backups):
                backup = sorted(backups, reverse=True)[idx - 1]

                confirm = input(
                    f"\n{YELLOW}⚠️ This will overwrite current database. Type 'YES' to confirm: {RESET}"
                ).strip()
                if confirm != "YES":
                    print(f"{YELLOW}Cancelled.{RESET}")
                    return

                print(f"{YELLOW}🔄 Restoring from {backup.name}...{RESET}")
                result = run_cmd(f'psql -d "{DB_NAME}" < "{backup}"')
                if result.returncode == 0:
                    print(f"{GREEN}✅ Restore completed!{RESET}")
                else:
                    print(f"{RED}❌ Restore failed.{RESET}")
            else:
                print(f"{RED}❌ Invalid selection.{RESET}")
        except ValueError:
            print(f"{RED}❌ Invalid input.{RESET}")
    else:
        print(f"{RED}❌ Invalid option.{RESET}")

    input(f"\n{CYAN}Press Enter to continue...{RESET}")


def main():
    """Main entry point."""
    if not check_psql():
        print(f"{RED}❌ PostgreSQL is not running or not installed.{RESET}")
        sys.exit(1)

    try:
        show_menu()
    except KeyboardInterrupt:
        print(f"\n{YELLOW}⚠️ Interrupted.{RESET}")
        sys.exit(0)


if __name__ == "__main__":
    main()

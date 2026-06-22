#!/usr/bin/env python3
"""Modern Format Boost - Interactive Database Manager
Provides interactive database operations with numeric key selection.

Features:
  1. Database Setup & Service Control - Start PostgreSQL and setup database
  2. Training & Audit Pipeline - delegates to training_pipeline.py
  3. Database Status - View database statistics
  4. Vector Index Manager - Manage pgvector indexes
  5. Backup & Restore - Backup and restore database
  6. Return to Home - Exit to main interface

Training Integration:
  The training functionality delegates to training_pipeline.py which provides:
  - Batch ingestion via run_training.py
  - Quality-regression verification
  - Loop-clustering verification
  - Loop-stat refresh
  - Dataset reporting
"""

import platform
import shutil
import subprocess
import sys
import time
from pathlib import Path

from mfb_ui_tokens import colors_enabled, pick_symbol

_SCRIPT_DIR = Path(__file__).resolve().parent
if str(_SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(_SCRIPT_DIR))

from mfb_entry_guard import guard_main, run_delegated  # noqa: E402

# ANSI color codes
if colors_enabled():
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
    print(
        f"\n{BLUE}{BOLD}{pick_symbol('🐘', ('[DB]'))} Modern Format Boost - Database Manager{RESET}"
    )
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
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL not found. Please install it first.{RESET}"
        )
        return False

    if run_cmd("pg_isready").returncode != 0:
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL is not running.{RESET}"
        )
        return False

    return True


def check_db_exists():
    """Check if database exists."""
    res = run_cmd(f"psql -lqt | cut -d \\| -f 1 | grep -qw '{DB_NAME}'")
    return res.returncode == 0


def start_postgres_service():
    """Start PostgreSQL service based on OS."""
    print(f"{YELLOW}{pick_symbol('🔄', ('~'))} Starting PostgreSQL service...{RESET}")

    if OS_TYPE == "darwin":
        if command_exists("brew"):
            res = run_cmd(
                "brew services list | awk '/^postgresql/ {print $1}' | head -n 1"
            )
            pg_service = res.stdout.strip()
            if pg_service:
                print(f"   Starting service '{pg_service}' via Homebrew...")
                run_cmd(f"brew services start {pg_service}")
            else:
                print(
                    f"{YELLOW}{pick_symbol('⚠️', '[WARN]')}  No PostgreSQL service found in 'brew services'.{RESET}"
                )
                print("   Trying to install default...")
                run_cmd("brew install postgresql && brew services start postgresql")
        else:
            print("   Homebrew not found. Trying pg_ctl...")
            run_cmd("pg_ctl start")
    elif OS_TYPE == "linux":
        if command_exists("systemctl"):
            run_cmd("sudo systemctl start postgresql")
        else:
            run_cmd("sudo service postgresql start")

    time.sleep(2)

    if run_cmd("pg_isready").returncode == 0:
        print(
            f"{GREEN}{pick_symbol('✅', ('[OK]'))} PostgreSQL started successfully!{RESET}"
        )
    else:
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} Failed to start PostgreSQL.{RESET}"
        )


def setup_database():
    """Setup database and pgvector extension."""
    print(
        f"\n{BLUE}{pick_symbol('🏗️', ('[BUILD]'))}  Setting up database: {DB_NAME}{RESET}"
    )
    print(f"{DIM}{'─' * 60}{RESET}\n")

    # Check if database exists
    res = run_cmd(f"psql -lqt | cut -d \\| -f 1 | grep -qw '{DB_NAME}'")
    if res.returncode == 0:
        print(
            f"{GREEN}{pick_symbol('✅', ('[OK]'))} Database '{DB_NAME}' already exists.{RESET}"
        )
    else:
        print(f"   Creating database '{DB_NAME}'...")
        if run_cmd(f'createdb "{DB_NAME}"').returncode != 0:
            run_cmd(f'psql -c "CREATE DATABASE {DB_NAME};"')
        print(f"{GREEN}{pick_symbol('✅', ('[OK]'))} Database created.{RESET}")

    # Enable pgvector extension
    print("\n   Ensuring pgvector extension is enabled...")
    res = run_cmd(f'psql -d "{DB_NAME}" -c "CREATE EXTENSION IF NOT EXISTS vector;"')
    if res.returncode != 0:
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} Failed to create 'vector' extension.{RESET}"
        )
        print(f"{YELLOW}   Is 'pgvector' installed?{RESET}")
        print(f"{DIM}   Try: brew install pgvector (macOS){RESET}")
        print(f"{DIM}   Or refer to your Linux distribution's pgvector package.{RESET}")
        input(f"\n{CYAN}Press Enter to continue...{RESET}")
        return False

    print(f"{GREEN}{pick_symbol('✅', ('[OK]'))} pgvector extension enabled.{RESET}")
    print(f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Database setup complete!{RESET}")
    input(f"\n{CYAN}Press Enter to continue...{RESET}")
    return True


def show_menu():
    """Display interactive database management menu."""
    while True:
        print_header()
        print(f"{CYAN}Database Management Options:{RESET}\n")

        print(f"  {GREEN}1{RESET} - {BOLD}Database Setup & Service Control{RESET}")
        print(f"     {DIM}Start PostgreSQL service and setup database{RESET}\n")

        print(f"  {GREEN}2{RESET} - {BOLD}Training & Audit Pipeline{RESET}")
        print(
            f"     {DIM}Batch ingest data and audit quality-regression / loop-clustering tasks{RESET}\n"
        )

        print(f"  {GREEN}3{RESET} - {BOLD}Database Status{RESET}")
        print(f"     {DIM}View database statistics and schema info{RESET}\n")

        print(f"  {GREEN}4{RESET} - {BOLD}Vector Index Manager{RESET}")
        print(f"     {DIM}Manage pgvector indexes and embeddings{RESET}\n")

        print(f"  {GREEN}5{RESET} - {BOLD}Backup & Restore{RESET}")
        print(f"     {DIM}Backup database or restore from backup{RESET}\n")

        print(f"  {GREEN}6{RESET} - {BOLD}Return to Home{RESET}")
        print(f"     {DIM}Exit database manager and return to main interface{RESET}\n")

        print(f"  {GREEN}0{RESET} - {BOLD}Exit{RESET}\n")
        print(f"{DIM}{'─' * 60}{RESET}")

        try:
            choice = input(f"{CYAN}Select option (0-6): {RESET}").strip()

            if choice == "0":
                print(f"\n{CYAN}Exiting database manager.{RESET}")
                sys.exit(0)
            elif choice == "6":
                print(f"\n{CYAN}Returning to main menu...{RESET}\n")
                break
            elif choice == "1":
                service_control_menu()
            elif choice == "2":
                train_new_data()
            elif choice == "3":
                show_status()
            elif choice == "4":
                manage_indexes()
            elif choice == "5":
                backup_restore()
            else:
                print(
                    f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid option. Please enter 0-6.{RESET}"
                )
                time.sleep(1)
        except KeyboardInterrupt:
            print(f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')} Cancelled.{RESET}")
            break
        except (
            OSError,
            ValueError,
            RuntimeError,
            TypeError,
            KeyError,
            IndexError,
            AttributeError,
            UnicodeError,
        ) as e:
            print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Error: {e}{RESET}")
            time.sleep(1)


def service_control_menu():
    """Database setup and service control submenu."""
    print(
        f"\n{BLUE}{pick_symbol('🔧', ('[TOOL]'))} Database Setup & Service Control{RESET}"
    )
    print(f"{DIM}{'─' * 60}{RESET}\n")

    print(f"  {GREEN}1{RESET} - Start PostgreSQL Service")
    print(f"  {GREEN}2{RESET} - Setup Database (create DB + pgvector)")
    print(f"  {GREEN}3{RESET} - Full Setup (start service + setup DB)")
    print(f"  {GREEN}0{RESET} - Back to main menu\n")

    choice = input(f"{CYAN}Select option (0-3): {RESET}").strip()

    if choice == "0":
        return
    elif choice == "1":
        if not command_exists("psql"):
            print(
                f"{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL not found. Please install it first.{RESET}\n"
            )
            input("Press Enter to continue...")
            return
        start_postgres_service()
        input(f"\n{CYAN}Press Enter to continue...{RESET}")
    elif choice == "2":
        if not check_psql():
            print(
                f"{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL is not running. Start it first (option 1).{RESET}\n"
            )
            input("Press Enter to continue...")
            return
        setup_database()
    elif choice == "3":
        if not command_exists("psql"):
            print(
                f"{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL not found. Please install it first.{RESET}\n"
            )
            input("Press Enter to continue...")
            return

        print(
            f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')}  CONFIRM: Run full database setup?{RESET}"
        )
        if input(
            f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
        ).strip().lower() not in ("y", "yes"):
            print(f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Cancelled.{RESET}")
            time.sleep(1)
            return

        start_postgres_service()
        print()
        setup_database()
    else:
        print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid option.{RESET}")
        time.sleep(1)


def train_new_data():
    """Launch training_pipeline.py for ingestion and task-family audits."""
    if not check_psql() or not check_db_exists():
        print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Database not available.{RESET}\n")
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}{pick_symbol('🎓', ('[TRAIN]'))} Training & Audit Pipeline{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}")

    print("\nTraining Options:")
    print(f"  {GREEN}1{RESET} - Full Task-Family Audit")
    print(f"  {GREEN}2{RESET} - Batch Ingest Training Data")
    print(f"  {GREEN}3{RESET} - Verify Quality Regression Tables")
    print(f"  {GREEN}4{RESET} - Verify Loop Clustering Table")
    print(f"  {GREEN}5{RESET} - Generate Combined Dataset Report")
    print(f"  {GREEN}6{RESET} - Refresh Loop Clustering Stats")
    print(f"  {GREEN}0{RESET} - Back to main menu\n")

    choice = input(f"{CYAN}Select option (0-6): {RESET}").strip()

    if choice == "0":
        return

    # Map choices to training_pipeline.py commands
    commands = {
        "1": ["evaluate"],
        "2": ["train"],
        "3": ["verify-quality-regression"],
        "4": ["verify-loop-clustering"],
        "5": ["report"],
        "6": ["refresh-loop-stats"],
    }

    if choice not in commands:
        print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid option.{RESET}")
        time.sleep(1)
        return

    print(
        f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')}  CONFIRM: Start training operation?{RESET}"
    )
    if input(
        f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
    ).strip().lower() not in ("y", "yes"):
        print(f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Cancelled.{RESET}")
        time.sleep(1)
        return

    print(f"\n{CYAN}⏳ Launching training/audit pipeline...{RESET}\n")
    time.sleep(1)

    # Get the script directory
    script_dir = Path(__file__).parent
    training_script = script_dir / "training_pipeline.py"

    if not training_script.exists():
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} training_pipeline.py not found at {training_script}{RESET}"
        )
        input("\nPress Enter to continue...")
        return

    # Run training_pipeline.py with the selected command
    cmd = commands[choice]
    try:
        result = run_delegated(
            ["python3", str(training_script)] + cmd,
            parent_script="database_manager.py",
            check=False,
        )

        if result.returncode == 0:
            print(
                f"\n{GREEN}{pick_symbol('✅', ('[OK]'))} Pipeline operation completed successfully!{RESET}"
            )
        else:
            print(
                f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')}  Pipeline operation completed with warnings.{RESET}"
            )
    except (
        OSError,
        ValueError,
        RuntimeError,
        TypeError,
        KeyError,
        IndexError,
        AttributeError,
        UnicodeError,
    ) as e:
        print(
            f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Error running training/audit pipeline: {e}{RESET}"
        )

    input(f"\n{CYAN}Press Enter to continue...{RESET}")


def show_status():
    """Show database status and statistics."""
    if not check_psql():
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL is not running.{RESET}\n"
        )
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}{pick_symbol('📊', ('[STATS]'))} Database Status{RESET}")
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
        print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Database not available.{RESET}\n")
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}{pick_symbol('📇', ('[INDEX]'))} Vector Index Manager{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}")
    print(
        f"{YELLOW}{pick_symbol('⚠️', '[WARN]')}  CONFIRM: Access index management?{RESET}"
    )
    if input(
        f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
    ).strip().lower() not in ("y", "yes"):
        print(f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Cancelled.{RESET}")
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
        print(f"\n{YELLOW}{pick_symbol('🔄', ('~'))} Rebuilding indexes...{RESET}")
        result = run_cmd(f'psql -d "{DB_NAME}" -c "REINDEX DATABASE {DB_NAME};"')
        if result.returncode == 0:
            print(
                f"{GREEN}{pick_symbol('✅', ('[OK]'))} Indexes rebuilt successfully!{RESET}"
            )
        else:
            print(
                f"{RED}{pick_symbol('❌', ('[ERROR]'))} Failed to rebuild indexes.{RESET}"
            )
    elif choice == "3":
        print(
            f"\n{YELLOW}{pick_symbol('🧹', ('[SWEEP]'))} Running VACUUM ANALYZE...{RESET}"
        )
        result = run_cmd(f'psql -d "{DB_NAME}" -c "VACUUM ANALYZE;"')
        if result.returncode == 0:
            print(f"{GREEN}{pick_symbol('✅', ('[OK]'))} Maintenance completed!{RESET}")
        else:
            print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Maintenance failed.{RESET}")
    else:
        print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid option.{RESET}")

    input(f"\n{CYAN}Press Enter to continue...{RESET}")


def backup_restore():
    """Backup and restore database."""
    if not check_psql() or not check_db_exists():
        print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Database not available.{RESET}\n")
        input("Press Enter to continue...")
        return

    print(f"\n{BLUE}{pick_symbol('💾', ('[SAVE]'))} Backup & Restore{RESET}")
    print(f"{DIM}{'─' * 60}{RESET}")
    print(
        f"{YELLOW}{pick_symbol('⚠️', '[WARN]')}  CONFIRM: Access backup/restore tools?{RESET}"
    )
    if input(
        f"   {CYAN}Type {GREEN}'yes'{CYAN} to proceed: {RESET}"
    ).strip().lower() not in ("y", "yes"):
        print(f"\n{RED}{pick_symbol('❌', ('[ERROR]'))} Cancelled.{RESET}")
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

        print(f"\n{YELLOW}{pick_symbol('💾', ('[SAVE]'))} Creating backup...{RESET}")
        result = run_cmd(f'pg_dump -d "{DB_NAME}" > "{backup_file}"')
        if result.returncode == 0:
            print(
                f"{GREEN}{pick_symbol('✅', ('[OK]'))} Backup created: {backup_file}{RESET}"
            )
        else:
            print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Backup failed.{RESET}")
    elif choice == "2":
        backups = list(backup_dir.glob("mfb_backup_*.sql"))
        if not backups:
            print(
                f"{RED}{pick_symbol('❌', ('[ERROR]'))} No backups found in {backup_dir}{RESET}\n"
            )
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
                    f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')} This will overwrite current database. Type 'YES' to confirm: {RESET}"
                ).strip()
                if confirm != "YES":
                    print(f"{YELLOW}Cancelled.{RESET}")
                    return

                print(
                    f"{YELLOW}{pick_symbol('🔄', ('~'))} Restoring from {backup.name}...{RESET}"
                )
                result = run_cmd(f'psql -d "{DB_NAME}" < "{backup}"')
                if result.returncode == 0:
                    print(
                        f"{GREEN}{pick_symbol('✅', ('[OK]'))} Restore completed!{RESET}"
                    )
                else:
                    print(
                        f"{RED}{pick_symbol('❌', ('[ERROR]'))} Restore failed.{RESET}"
                    )
            else:
                print(
                    f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid selection.{RESET}"
                )
        except ValueError:
            print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid input.{RESET}")
    else:
        print(f"{RED}{pick_symbol('❌', ('[ERROR]'))} Invalid option.{RESET}")

    input(f"\n{CYAN}Press Enter to continue...{RESET}")


def main():
    """Main entry point."""
    guard_main("database_manager.py")
    if not check_psql():
        print(
            f"{RED}{pick_symbol('❌', ('[ERROR]'))} PostgreSQL is not running or not installed.{RESET}"
        )
        sys.exit(1)

    try:
        show_menu()
    except KeyboardInterrupt:
        print(f"\n{YELLOW}{pick_symbol('⚠️', '[WARN]')} Interrupted.{RESET}")
        sys.exit(0)


if __name__ == "__main__":
    main()

//! Modern Format Boost - Interactive Database Manager.

use anyhow::{Context, Result, anyhow};
use dev::infra::ui_tokens::{colors_enabled, pick_symbol};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::thread;
use std::time::Duration;

const DB_NAME: &str = "modern_format_boost";

struct Colors {
    red: &'static str,
    green: &'static str,
    yellow: &'static str,
    blue: &'static str,
    cyan: &'static str,
    bold: &'static str,
    dim: &'static str,
    reset: &'static str,
}

const COLORS: Colors = Colors {
    red: "\x1b[0;31m",
    green: "\x1b[0;32m",
    yellow: "\x1b[1;33m",
    blue: "\x1b[0;34m",
    cyan: "\x1b[0;36m",
    bold: "\x1b[1m",
    dim: "\x1b[2m",
    reset: "\x1b[0m",
};

const PLAIN: Colors = Colors {
    red: "",
    green: "",
    yellow: "",
    blue: "",
    cyan: "",
    bold: "",
    dim: "",
    reset: "",
};

fn colors() -> &'static Colors {
    if colors_enabled() { &COLORS } else { &PLAIN }
}

fn command_exists(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cmd).is_file()))
}

fn run_cmd(cmd: &str) -> Result<std::process::Output> {
    Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("run shell command: {cmd}"))
}

fn run_status(cmd: &str) -> bool {
    match run_cmd(cmd) {
        Ok(out) => out.status.success(),
        Err(err) => {
            eprintln!("[DB-MGR] command failed ({cmd}): {err}");
            false
        }
    }
}

fn read_choice(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("flush stdout")?;
    let mut line = String::new();
    io::stdin().read_line(&mut line).context("read stdin")?;
    Ok(line.trim().to_string())
}

fn wait_enter() -> Result<()> {
    let c = colors();
    let _ = read_choice(&format!(
        "\n{}Press Enter to continue...{}",
        c.cyan, c.reset
    ))?;
    Ok(())
}

fn confirm(prompt: &str) -> Result<bool> {
    let c = colors();
    Ok(matches!(
        read_choice(&format!(
            "   {}Type {}'yes'{} to proceed: {}",
            c.cyan, c.green, c.cyan, c.reset
        ))?
        .to_lowercase()
        .as_str(),
        "y" | "yes"
    ) && {
        let _ = prompt;
        true
    })
}

fn print_header() {
    let c = colors();
    println!(
        "\n{}{}{} Modern Format Boost - Database Manager{}",
        c.blue,
        c.bold,
        pick_symbol("🐘", "[DB]"),
        c.reset
    );
    println!("{}{}{}\n", c.dim, "─".repeat(60), c.reset);
}

fn check_psql() -> bool {
    let c = colors();
    if !command_exists("psql") {
        println!(
            "{}{} PostgreSQL not found. Please install it first.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        return false;
    }
    if !run_status("pg_isready") {
        println!(
            "{}{} PostgreSQL is not running.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        return false;
    }
    true
}

fn check_db_exists() -> bool {
    run_status(&format!(
        "psql -lqt | cut -d \\| -f 1 | grep -qw '{DB_NAME}'"
    ))
}

fn start_postgres_service() {
    let c = colors();
    println!(
        "{}{} Starting PostgreSQL service...{}",
        c.yellow,
        pick_symbol("🔄", "~"),
        c.reset
    );
    match std::env::consts::OS {
        "macos" if command_exists("brew") => {
            let service =
                match run_cmd("brew services list | awk '/^postgresql/ {print $1}' | head -n 1") {
                    Ok(out) => match String::from_utf8(out.stdout) {
                        Ok(s) => s.trim().to_string(),
                        Err(err) => {
                            eprintln!("[DB-MGR] brew services output decode failed: {err}");
                            String::new()
                        }
                    },
                    Err(err) => {
                        eprintln!("[DB-MGR] brew services probe failed: {err}");
                        String::new()
                    }
                };
            if service.is_empty() {
                println!(
                    "{}{}  No PostgreSQL service found in 'brew services'.{}",
                    c.yellow,
                    pick_symbol("⚠️", "[WARN]"),
                    c.reset
                );
                let _ = run_cmd("brew install postgresql && brew services start postgresql");
            } else {
                println!("   Starting service '{service}' via Homebrew...");
                let _ = run_cmd(&format!("brew services start {service}"));
            }
        }
        "macos" => {
            println!("   Homebrew not found. Trying pg_ctl...");
            let _ = run_cmd("pg_ctl start");
        }
        "linux" if command_exists("systemctl") => {
            let _ = run_cmd("sudo systemctl start postgresql");
        }
        "linux" => {
            let _ = run_cmd("sudo service postgresql start");
        }
        _ => {}
    }
    thread::sleep(Duration::from_secs(2));
    if run_status("pg_isready") {
        println!(
            "{}{} PostgreSQL started successfully!{}",
            c.green,
            pick_symbol("✅", "[OK]"),
            c.reset
        );
    } else {
        println!(
            "{}{} Failed to start PostgreSQL.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
    }
}

fn setup_database() -> Result<bool> {
    let c = colors();
    println!(
        "\n{}{}  Setting up database: {DB_NAME}{}",
        c.blue,
        pick_symbol("🏗️", "[BUILD]"),
        c.reset
    );
    println!("{}{}{}\n", c.dim, "─".repeat(60), c.reset);

    if check_db_exists() {
        println!(
            "{}{} Database '{DB_NAME}' already exists.{}",
            c.green,
            pick_symbol("✅", "[OK]"),
            c.reset
        );
    } else {
        println!("   Creating database '{DB_NAME}'...");
        if !run_status(&format!("createdb \"{DB_NAME}\"")) {
            let _ = run_cmd(&format!("psql -c \"CREATE DATABASE {DB_NAME};\""));
        }
        println!(
            "{}{} Database created.{}",
            c.green,
            pick_symbol("✅", "[OK]"),
            c.reset
        );
    }

    println!("\n   Ensuring pgvector extension is enabled...");
    if !run_status(&format!(
        "psql -d \"{DB_NAME}\" -c \"CREATE EXTENSION IF NOT EXISTS vector;\""
    )) {
        println!(
            "{}{} Failed to create 'vector' extension.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        println!("{}   Is 'pgvector' installed?{}", c.yellow, c.reset);
        println!("{}   Try: brew install pgvector (macOS){}", c.dim, c.reset);
        wait_enter()?;
        return Ok(false);
    }
    println!(
        "{}{} pgvector extension enabled.{}",
        c.green,
        pick_symbol("✅", "[OK]"),
        c.reset
    );
    println!(
        "\n{}{} Database setup complete!{}",
        c.green,
        pick_symbol("✅", "[OK]"),
        c.reset
    );
    wait_enter()?;
    Ok(true)
}

fn service_control_menu() -> Result<()> {
    let c = colors();
    println!(
        "\n{}{} Database Setup & Service Control{}",
        c.blue,
        pick_symbol("🔧", "[TOOL]"),
        c.reset
    );
    println!("{}{}{}\n", c.dim, "─".repeat(60), c.reset);
    println!("  {}1{} - Start PostgreSQL Service", c.green, c.reset);
    println!(
        "  {}2{} - Setup Database (create DB + pgvector)",
        c.green, c.reset
    );
    println!(
        "  {}3{} - Full Setup (start service + setup DB)",
        c.green, c.reset
    );
    println!("  {}0{} - Back to main menu\n", c.green, c.reset);
    match read_choice(&format!("{}Select option (0-3): {}", c.cyan, c.reset))?.as_str() {
        "0" => {}
        "1" => {
            if command_exists("psql") {
                start_postgres_service();
            } else {
                println!(
                    "{}{} PostgreSQL not found. Please install it first.{}",
                    c.red,
                    pick_symbol("❌", "[ERROR]"),
                    c.reset
                );
            }
            wait_enter()?;
        }
        "2" => {
            if check_psql() {
                let _ = setup_database()?;
            } else {
                wait_enter()?;
            }
        }
        "3" => {
            if !command_exists("psql") {
                println!(
                    "{}{} PostgreSQL not found. Please install it first.{}",
                    c.red,
                    pick_symbol("❌", "[ERROR]"),
                    c.reset
                );
                wait_enter()?;
                return Ok(());
            }
            println!(
                "\n{}{}  CONFIRM: Run full database setup?{}",
                c.yellow,
                pick_symbol("⚠️", "[WARN]"),
                c.reset
            );
            if confirm("full setup")? {
                start_postgres_service();
                println!();
                let _ = setup_database()?;
            } else {
                println!(
                    "\n{}{} Cancelled.{}",
                    c.red,
                    pick_symbol("❌", "[ERROR]"),
                    c.reset
                );
            }
        }
        _ => println!(
            "{}{} Invalid option.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        ),
    }
    Ok(())
}

fn repo_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir().context("current dir")?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(anyhow!("could not find workspace root"));
        }
    }
}

fn run_training_pipeline(cmd: &[&str]) -> Result<ExitStatus> {
    let root = repo_root()?;
    let connstr = dev::training_pipeline::resolve_connstr(None);
    let mut command = dev::training_pipeline::training_pipeline_command(&root);
    command
        .arg("--connstr")
        .arg(&connstr)
        .args(cmd)
        .env("MFB_INVOKER", "database_manager")
        .status()
        .with_context(|| format!("run training_pipeline {}", cmd.join(" ")))
}

fn train_new_data() -> Result<()> {
    let c = colors();
    if !check_psql() || !check_db_exists() {
        println!(
            "{}{} Database not available.{}\n",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        wait_enter()?;
        return Ok(());
    }
    println!(
        "\n{}{} Training & Audit Pipeline{}",
        c.blue,
        pick_symbol("🎓", "[TRAIN]"),
        c.reset
    );
    println!("{}{}{}", c.dim, "─".repeat(60), c.reset);
    println!("\nTraining Options:");
    println!("  {}1{} - Full Task-Family Audit", c.green, c.reset);
    println!("  {}2{} - Batch Ingest Training Data", c.green, c.reset);
    println!(
        "  {}3{} - Verify Quality Regression Tables",
        c.green, c.reset
    );
    println!("  {}4{} - Verify Loop Clustering Table", c.green, c.reset);
    println!(
        "  {}5{} - Generate Combined Dataset Report",
        c.green, c.reset
    );
    println!("  {}6{} - Refresh Loop Clustering Stats", c.green, c.reset);
    println!("  {}0{} - Back to main menu\n", c.green, c.reset);
    let choice = read_choice(&format!("{}Select option (0-6): {}", c.cyan, c.reset))?;
    let cmd = match choice.as_str() {
        "0" => return Ok(()),
        "1" => &["evaluate"][..],
        "2" => &["train"][..],
        "3" => &["verify-quality-regression"][..],
        "4" => &["verify-loop-clustering"][..],
        "5" => &["report"][..],
        "6" => &["refresh-loop-stats"][..],
        _ => {
            println!(
                "{}{} Invalid option.{}",
                c.red,
                pick_symbol("❌", "[ERROR]"),
                c.reset
            );
            return Ok(());
        }
    };
    println!(
        "\n{}{}  CONFIRM: Start training operation?{}",
        c.yellow,
        pick_symbol("⚠️", "[WARN]"),
        c.reset
    );
    if !confirm("training")? {
        println!(
            "\n{}{} Cancelled.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        return Ok(());
    }
    println!(
        "\n{}{} Launching training/audit pipeline...{}\n",
        c.cyan,
        pick_symbol("⏳", "[WAIT]"),
        c.reset
    );
    let status = run_training_pipeline(cmd)?;
    if status.success() {
        println!(
            "\n{}{} Pipeline operation completed successfully!{}",
            c.green,
            pick_symbol("✅", "[OK]"),
            c.reset
        );
    } else {
        println!(
            "\n{}{}  Pipeline operation completed with warnings.{}",
            c.yellow,
            pick_symbol("⚠️", "[WARN]"),
            c.reset
        );
    }
    wait_enter()?;
    Ok(())
}

fn show_status() -> Result<()> {
    let c = colors();
    if !check_psql() {
        wait_enter()?;
        return Ok(());
    }
    println!(
        "\n{}{} Database Status{}",
        c.blue,
        pick_symbol("📊", "[STATS]"),
        c.reset
    );
    println!("{}{}{}\n", c.dim, "─".repeat(60), c.reset);
    println!("PostgreSQL Status: {}RUNNING{}", c.green, c.reset);
    if check_db_exists() {
        println!("Database '{DB_NAME}': {}EXISTS{}\n", c.green, c.reset);
        let query = "SELECT tablename, \
                     pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) as size \
                     FROM pg_tables WHERE schemaname = 'public' ORDER BY \
                     pg_total_relation_size(schemaname||'.'||tablename) DESC;";
        let out = run_cmd(&format!("psql -d \"{DB_NAME}\" -c \"{query}\" -t"))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if out.status.success() && !stdout.trim().is_empty() {
            println!("{}Tables and Sizes:{}", c.cyan, c.reset);
            for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
                println!("  {line}");
            }
        } else {
            println!("{}No tables found.{}", c.dim, c.reset);
        }
    } else {
        println!("Database '{DB_NAME}': {}NOT FOUND{}\n", c.yellow, c.reset);
        println!("{}Run database setup first.{}", c.dim, c.reset);
    }
    wait_enter()?;
    Ok(())
}

fn manage_indexes() -> Result<()> {
    let c = colors();
    if !check_psql() || !check_db_exists() {
        println!(
            "{}{} Database not available.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        wait_enter()?;
        return Ok(());
    }
    println!(
        "\n{}{} Vector Index Manager{}",
        c.blue,
        pick_symbol("📇", "[INDEX]"),
        c.reset
    );
    println!("{}{}{}", c.dim, "─".repeat(60), c.reset);
    println!(
        "{}{}  CONFIRM: Access index management?{}",
        c.yellow,
        pick_symbol("⚠️", "[WARN]"),
        c.reset
    );
    if !confirm("indexes")? {
        println!(
            "\n{}{} Cancelled.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        return Ok(());
    }
    println!("\nIndex Management Options:");
    println!("  {}1{} - View Indexes", c.green, c.reset);
    println!("  {}2{} - Rebuild Indexes", c.green, c.reset);
    println!("  {}3{} - Vacuum Analyze", c.green, c.reset);
    println!("  {}0{} - Back\n", c.green, c.reset);
    match read_choice(&format!("{}Select option (0-3): {}", c.cyan, c.reset))?.as_str() {
        "0" => {}
        "1" => {
            let out = run_cmd(&format!(
                "psql -d \"{DB_NAME}\" -c \"SELECT indexname FROM pg_indexes WHERE schemaname = \
                 'public';\" -t"
            ))?;
            let stdout = String::from_utf8_lossy(&out.stdout);
            if out.status.success() && !stdout.trim().is_empty() {
                for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
                    println!("  - {}", line.trim());
                }
            } else {
                println!("{}No indexes found.{}", c.dim, c.reset);
            }
        }
        "2" => {
            if run_status(&format!(
                "psql -d \"{DB_NAME}\" -c \"REINDEX DATABASE {DB_NAME};\""
            )) {
                println!(
                    "{}{} Indexes rebuilt successfully!{}",
                    c.green,
                    pick_symbol("✅", "[OK]"),
                    c.reset
                );
            } else {
                println!(
                    "{}{} Failed to rebuild indexes.{}",
                    c.red,
                    pick_symbol("❌", "[ERROR]"),
                    c.reset
                );
            }
        }
        "3" => {
            if run_status(&format!("psql -d \"{DB_NAME}\" -c \"VACUUM ANALYZE;\"")) {
                println!(
                    "{}{} Maintenance completed!{}",
                    c.green,
                    pick_symbol("✅", "[OK]"),
                    c.reset
                );
            } else {
                println!(
                    "{}{} Maintenance failed.{}",
                    c.red,
                    pick_symbol("❌", "[ERROR]"),
                    c.reset
                );
            }
        }
        _ => println!(
            "{}{} Invalid option.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        ),
    }
    wait_enter()?;
    Ok(())
}

fn backup_restore() -> Result<()> {
    let c = colors();
    if !check_psql() || !check_db_exists() {
        println!(
            "{}{} Database not available.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        wait_enter()?;
        return Ok(());
    }
    println!(
        "\n{}{} Backup & Restore{}",
        c.blue,
        pick_symbol("💾", "[SAVE]"),
        c.reset
    );
    println!("{}{}{}", c.dim, "─".repeat(60), c.reset);
    println!(
        "{}{}  CONFIRM: Access backup/restore tools?{}",
        c.yellow,
        pick_symbol("⚠️", "[WARN]"),
        c.reset
    );
    if !confirm("backup")? {
        println!(
            "\n{}{} Cancelled.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        return Ok(());
    }
    println!("Options:");
    println!("  {}1{} - Create Backup", c.green, c.reset);
    println!("  {}2{} - Restore from Backup", c.green, c.reset);
    println!("  {}0{} - Back\n", c.green, c.reset);
    let backup_dir = std::env::var_os("HOME").map_or_else(
        || PathBuf::from(".mfb_backups"),
        |home| PathBuf::from(home).join(".cache/mfb_backups"),
    );
    std::fs::create_dir_all(&backup_dir).context("create backup dir")?;
    match read_choice(&format!("{}Select option (0-2): {}", c.cyan, c.reset))?.as_str() {
        "0" => {}
        "1" => {
            let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            let backup = backup_dir.join(format!("mfb_backup_{stamp}.sql"));
            if run_status(&format!(
                "pg_dump -d \"{DB_NAME}\" > \"{}\"",
                backup.display()
            )) {
                println!(
                    "{}{} Backup created: {}{}",
                    c.green,
                    pick_symbol("✅", "[OK]"),
                    backup.display(),
                    c.reset
                );
            } else {
                println!(
                    "{}{} Backup failed.{}",
                    c.red,
                    pick_symbol("❌", "[ERROR]"),
                    c.reset
                );
            }
        }
        "2" => restore_backup(&backup_dir)?,
        _ => println!(
            "{}{} Invalid option.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        ),
    }
    wait_enter()?;
    Ok(())
}

fn restore_backup(backup_dir: &Path) -> Result<()> {
    let c = colors();
    let mut backups: Vec<_> = std::fs::read_dir(backup_dir)
        .with_context(|| format!("read backup dir {}", backup_dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("mfb_backup_") && n.ends_with(".sql"))
        })
        .collect();
    backups.sort();
    backups.reverse();
    if backups.is_empty() {
        println!(
            "{}{} No backups found in {}{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            backup_dir.display(),
            c.reset
        );
        return Ok(());
    }
    for (i, backup) in backups.iter().enumerate() {
        println!(
            "  {}{}{} - {}",
            c.green,
            i + 1,
            c.reset,
            backup
                .file_name()
                .unwrap_or(std::ffi::OsStr::new("(invalid)"))
                .to_string_lossy()
        );
    }
    println!("  {}0{} - Cancel\n", c.green, c.reset);
    let choice = read_choice(&format!(
        "{}Select backup to restore (0-{}): {}",
        c.cyan,
        backups.len(),
        c.reset
    ))?;
    let Ok(idx) = choice.parse::<usize>() else {
        println!(
            "{}{} Invalid input.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        return Ok(());
    };
    if idx == 0 {
        return Ok(());
    }
    let Some(backup) = backups.get(idx - 1) else {
        println!(
            "{}{} Invalid selection.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        return Ok(());
    };
    let confirm = read_choice(&format!(
        "\n{}{} This will overwrite current database. Type 'YES' to confirm: {}",
        c.yellow,
        pick_symbol("⚠️", "[WARN]"),
        c.reset
    ))?;
    if confirm != "YES" {
        println!("{}Cancelled.{}", c.yellow, c.reset);
        return Ok(());
    }
    if run_status(&format!("psql -d \"{DB_NAME}\" < \"{}\"", backup.display())) {
        println!(
            "{}{} Restore completed!{}",
            c.green,
            pick_symbol("✅", "[OK]"),
            c.reset
        );
    } else {
        println!(
            "{}{} Restore failed.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
    }
    Ok(())
}

fn show_menu() -> Result<()> {
    let c = colors();
    loop {
        print_header();
        println!("{}Database Management Options:{}\n", c.cyan, c.reset);
        println!(
            "  {}1{} - {}Database Setup & Service Control{}",
            c.green, c.reset, c.bold, c.reset
        );
        println!(
            "     {}Start PostgreSQL service and setup database{}\n",
            c.dim, c.reset
        );
        println!(
            "  {}2{} - {}Training & Audit Pipeline{}",
            c.green, c.reset, c.bold, c.reset
        );
        println!(
            "     {}Batch ingest data and audit task families{}\n",
            c.dim, c.reset
        );
        println!(
            "  {}3{} - {}Database Status{}",
            c.green, c.reset, c.bold, c.reset
        );
        println!(
            "     {}View database statistics and schema info{}\n",
            c.dim, c.reset
        );
        println!(
            "  {}4{} - {}Vector Index Manager{}",
            c.green, c.reset, c.bold, c.reset
        );
        println!(
            "     {}Manage pgvector indexes and embeddings{}\n",
            c.dim, c.reset
        );
        println!(
            "  {}5{} - {}Backup & Restore{}",
            c.green, c.reset, c.bold, c.reset
        );
        println!(
            "     {}Backup database or restore from backup{}\n",
            c.dim, c.reset
        );
        println!(
            "  {}6{} - {}Return to Home{}",
            c.green, c.reset, c.bold, c.reset
        );
        println!("  {}0{} - {}Exit{}\n", c.green, c.reset, c.bold, c.reset);
        println!("{}{}{}", c.dim, "─".repeat(60), c.reset);
        match read_choice(&format!("{}Select option (0-6): {}", c.cyan, c.reset))?.as_str() {
            "0" => {
                println!("\n{}Exiting database manager.{}", c.cyan, c.reset);
                break;
            }
            "6" => {
                println!("\n{}Returning to main menu...{}\n", c.cyan, c.reset);
                break;
            }
            "1" => service_control_menu()?,
            "2" => train_new_data()?,
            "3" => show_status()?,
            "4" => manage_indexes()?,
            "5" => backup_restore()?,
            _ => println!(
                "{}{} Invalid option. Please enter 0-6.{}",
                c.red,
                pick_symbol("❌", "[ERROR]"),
                c.reset
            ),
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    if !check_psql() {
        let c = colors();
        println!(
            "{}{} PostgreSQL is not running or not installed.{}",
            c.red,
            pick_symbol("❌", "[ERROR]"),
            c.reset
        );
        std::process::exit(1);
    }
    show_menu()
}

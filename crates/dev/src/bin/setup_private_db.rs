use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const DEFAULT_CONNSTR: &str = "postgresql:///modern_format_boost";

fn main() -> Result<()> {
    let conf_dir = foundation::process_lock::get_mfb_root().context("resolve MFB state root")?;
    let conf_json = conf_dir.join("local_env.json");
    let conf_sh = conf_dir.join("local_env.sh");
    fs::create_dir_all(&conf_dir).with_context(|| format!("create {}", conf_dir.display()))?;

    println!("Modern Format Boost - Private Environment Setup (Rust Edition)");

    let existing = load_existing_connstr(&conf_json, &conf_sh);
    let default_connstr = existing.as_deref().unwrap_or(DEFAULT_CONNSTR);

    println!("Enter your PostgreSQL connection string (Press Enter for default):");
    println!("Default: {default_connstr}");
    print!("> ");
    io::stdout().flush().context("flush prompt")?;

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(0) => {
            println!("\nCancelled.");
            return Ok(());
        }
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::Interrupted => {
            println!("\nCancelled.");
            return Ok(());
        }
        Err(err) => return Err(err).context("read connection string"),
    }

    let connstr = match input.trim() {
        "" => default_connstr.to_owned(),
        value => value.to_owned(),
    };
    let data = json!({ "MFB_PG_CONNSTR": connstr });
    fs::write(&conf_json, serde_json::to_string_pretty(&data)?)
        .with_context(|| format!("write {}", conf_json.display()))?;
    make_private_executable(&conf_json)?;

    println!("\nConfiguration saved to: {}", conf_json.display());
    println!("The drag-and-drop processor will now load this file automatically.");

    Ok(())
}

fn load_existing_connstr(json_path: &Path, sh_path: &Path) -> Option<String> {
    if json_path.is_file() {
        println!(
            "Existing JSON configuration found at: {}",
            json_path.display()
        );
        match fs::read_to_string(json_path) {
            Ok(text) => {
                println!("{text}");
                match serde_json::from_str::<Value>(&text) {
                    Ok(value) => {
                        return value
                            .get("MFB_PG_CONNSTR")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                    }
                    Err(err) => eprintln!(
                        "[SETUP-DB] JSON parse failed ({}): {err}",
                        json_path.display()
                    ),
                }
            }
            Err(err) => println!("Error reading existing JSON: {err}"),
        }
        return None;
    }

    if sh_path.is_file() {
        println!(
            "Existing shell configuration found at: {}",
            sh_path.display()
        );
        match fs::read_to_string(sh_path) {
            Ok(text) => {
                println!("{text}");
                return parse_shell_connstr(&text);
            }
            Err(err) => println!("Error reading existing sh: {err}"),
        }
    }
    None
}

fn parse_shell_connstr(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("export ") else {
            continue;
        };
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        if key.trim() == "MFB_PG_CONNSTR" {
            return Some(value.trim().trim_matches(['"', '\'']).to_owned());
        }
    }
    None
}

fn make_private_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("chmod 755 {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

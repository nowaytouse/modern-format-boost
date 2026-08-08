use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::tempdir;

#[test]
fn writes_default_connection_string_to_local_json() -> Result<()> {
    let repo = temp_git_repo()?;

    run_setup(repo.path(), "\n")?;

    assert_connstr(repo.path(), "postgresql:///modern_format_boost")?;
    assert_private_mode(repo.path())?;

    Ok(())
}

#[test]
fn keeps_existing_json_connection_when_input_is_empty() -> Result<()> {
    let repo = temp_git_repo()?;
    let conf_dir = repo.path().join(".modern_format_boost");
    std::fs::create_dir_all(&conf_dir)?;
    std::fs::write(
        conf_dir.join("local_env.json"),
        r#"{"MFB_PG_CONNSTR":"postgresql://existing/db"}"#,
    )?;

    run_setup(repo.path(), "\n")?;

    assert_connstr(repo.path(), "postgresql://existing/db")
}

#[test]
fn imports_legacy_shell_connection_when_json_is_missing() -> Result<()> {
    let repo = temp_git_repo()?;
    let conf_dir = repo.path().join(".modern_format_boost");
    std::fs::create_dir_all(&conf_dir)?;
    std::fs::write(
        conf_dir.join("local_env.sh"),
        "export MFB_PG_CONNSTR=\"postgresql://legacy/db\"\n",
    )?;

    run_setup(repo.path(), "\n")?;

    assert_connstr(repo.path(), "postgresql://legacy/db")
}

#[test]
fn typed_connection_string_overrides_default() -> Result<()> {
    let repo = temp_git_repo()?;

    run_setup(repo.path(), "postgresql://typed/db\n")?;

    assert_connstr(repo.path(), "postgresql://typed/db")
}

#[test]
fn eof_cancels_without_writing_default_config() -> Result<()> {
    let repo = temp_git_repo()?;

    run_setup(repo.path(), "")?;

    assert!(!repo.path().join(".modern_format_boost/local_env.json").exists());
    Ok(())
}

fn temp_git_repo() -> Result<tempfile::TempDir> {
    let repo = tempdir()?;
    let status = Command::new("git")
        .arg("init")
        .current_dir(repo.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("git init")?;
    if !status.success() {
        bail!("git init failed with status {status}");
    }
    Ok(repo)
}

fn run_setup(repo: &std::path::Path, stdin: &str) -> Result<()> {
    let bin = match std::env::var("CARGO_BIN_EXE_setup_private_db") {
        Ok(value) => value,
        Err(err) => bail!("missing CARGO_BIN_EXE_setup_private_db: {err}"),
    };
    let mut child = Command::new(bin)
        .current_dir(repo)
        .env("MFB_HOME_ROOT", repo.join(".modern_format_boost"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn setup_private_db")?;

    child
        .stdin
        .take()
        .context("stdin missing")?
        .write_all(stdin.as_bytes())?;

    let output = child.wait_with_output()?;
    if !output.status.success() {
        bail!(
            "setup_private_db failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

fn assert_connstr(repo: &std::path::Path, expected: &str) -> Result<()> {
    let config_path = repo.join(".modern_format_boost/local_env.json");
    let data: Value = serde_json::from_slice(&std::fs::read(&config_path)?)?;
    assert_eq!(
        data.get("MFB_PG_CONNSTR").and_then(Value::as_str),
        Some(expected)
    );
    Ok(())
}

fn assert_private_mode(repo: &std::path::Path) -> Result<()> {
    let config_path = repo.join(".modern_format_boost/local_env.json");
    let mode = std::fs::metadata(&config_path)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(mode.mode() & 0o777, 0o755);
    }
    Ok(())
}

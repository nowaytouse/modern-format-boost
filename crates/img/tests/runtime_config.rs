use std::path::Path;
use std::process::{Command, Output};

fn inspect(root: &Path, args: &[&str]) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_img"));
    command
        .env_clear()
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("MFB_INVOKER", "test-harness")
        .env("MFB_PHOTOS_IMPORT_BACKEND", "applescript")
        .env("MFB_TOOL_CJXL", "/environment/cjxl")
        .current_dir(root)
        .args(args);
    for key in ["PATH", "DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.output()
}

#[test]
fn configuration_precedence_and_failure_are_observable_without_media() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let user_dir = root.path().join("config/modern-format-boost");
    std::fs::create_dir_all(&user_dir)?;
    std::fs::write(
        user_dir.join("config.json"),
        r#"{"config_version":1,"img":{"jpeg_effort":8},"photos":{"backend":"native","import_root":"Saved"}}"#,
    )?;
    std::fs::write(
        root.path().join("mfb.json"),
        r#"{"config_version":1,"img":{"jpeg_effort":9},"photos":{"album_name":"Project"}}"#,
    )?;
    std::fs::write(
        root.path().join("explicit.json"),
        r#"{"config_version":1,"img":{"jpeg_effort":10},"photos":{"import_root":"Explicit"}}"#,
    )?;
    let output = inspect(
        root.path(),
        &[
            "config",
            "show",
            "--effective",
            "--config",
            "explicit.json",
            "--jpeg-effort",
            "7",
            "--tool",
            "cjxl=/cli/cjxl",
        ],
    )?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout)?;
    for expected in [
        "\"jpeg_effort\": 7",
        "\"backend\": \"native\"",
        "\"album_name\": \"Project\"",
        "\"import_root\": \"Explicit\"",
        "\"cjxl\": \"/cli/cjxl\"",
        "\"img.jpeg_effort\": \"CLI\"",
        "\"photos.import_root\": \"explicit.json\"",
    ] {
        anyhow::ensure!(report.contains(expected), "missing {expected}: {report}");
    }
    let defaults = inspect(root.path(), &["config", "show", "--no-config"])?;
    anyhow::ensure!(defaults.status.success());
    let report = String::from_utf8(defaults.stdout)?;
    anyhow::ensure!(report.contains("\"jpeg_effort\": 11"));
    anyhow::ensure!(report.contains("\"backend\": \"applescript\""));

    std::fs::write(
        root.path().join("mfb.json"),
        r#"{"config_version":1,"photos":{"typo":true}}"#,
    )?;
    let invalid = inspect(root.path(), &["config", "show"])?;
    anyhow::ensure!(!invalid.status.success());
    anyhow::ensure!(String::from_utf8_lossy(&invalid.stderr).contains("unknown field"));
    anyhow::ensure!(
        !inspect(root.path(), &["config", "validate"])?
            .status
            .success()
    );
    let paths = inspect(root.path(), &["config", "path"])?;
    anyhow::ensure!(paths.status.success());
    let paths: serde_json::Value = serde_json::from_slice(&paths.stdout)?;
    anyhow::ensure!(paths["files_enabled"] == true);
    anyhow::ensure!(
        paths["project"]
            == root
                .path()
                .canonicalize()?
                .join("mfb.json")
                .to_string_lossy()
                .as_ref()
    );
    anyhow::ensure!(
        inspect(root.path(), &["config", "init", "new.json"])?
            .status
            .success()
    );
    let original = std::fs::read(root.path().join("new.json"))?;
    anyhow::ensure!(
        !inspect(root.path(), &["config", "init", "new.json"])?
            .status
            .success()
    );
    anyhow::ensure!(std::fs::read(root.path().join("new.json"))? == original);
    anyhow::ensure!(
        inspect(root.path(), &["config", "validate", "--no-config"])?
            .status
            .success()
    );
    Ok(())
}

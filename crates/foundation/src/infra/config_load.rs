use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fs;
use std::path::Path;

fn config_path_label(path: &Path) -> String {
    if let Some(name) = path.file_name() {
        name.to_string_lossy().into_owned()
    } else {
        eprintln!("[CONFIG] path has no file_name: {}", path.display());
        path.display().to_string()
    }
}

/// Load a JSON config that declares exactly one runtime owner via `_consumer`.
///
/// Rejects missing/wrong consumer.
pub fn load_consumer_json(
    path: &Path,
    expected_consumer: &str,
) -> Result<serde_json::Map<String, Value>> {
    if !path.is_file() {
        bail!("config not found: {}", path.display());
    }
    let content = fs::read_to_string(path)?;
    let root: Value = serde_json::from_str(&content)
        .with_context(|| format!("{}: invalid JSON", config_path_label(path)))?;

    let obj = match root {
        Value::Object(map) => map,
        _ => bail!("{}: root must be a JSON object", config_path_label(path)),
    };

    let consumer = match obj.get("_consumer") {
        Some(Value::String(s)) => s,
        _ => bail!(
            "{}: _consumer must be {:?}, got None. See CONFIG_CONSUMERS.md",
            config_path_label(path),
            expected_consumer
        ),
    };

    if consumer != expected_consumer {
        bail!(
            "{}: _consumer must be {:?}, got {:?}. See CONFIG_CONSUMERS.md",
            config_path_label(path),
            expected_consumer,
            consumer
        );
    }

    Ok(obj)
}

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

pub fn load_consumer_json(
    path: &Path,
    expected_consumer: &str,
) -> Result<serde_json::Map<String, Value>> {
    if !path.is_file() {
        bail!("config not found: {}", path.display());
    }

    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;

    let root: Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse JSON in {}", path.display()))?;

    let obj = match root {
        Value::Object(map) => map,
        _ => bail!(
            "{}: root must be a JSON object",
            path.file_name().unwrap_or_default().to_string_lossy()
        ),
    };

    let consumer = obj.get("_consumer").and_then(|v| v.as_str());
    if consumer != Some(expected_consumer) {
        bail!(
            "{}: _consumer must be {:?}, got {:?}. See CONFIG_CONSUMERS.md",
            path.file_name().unwrap_or_default().to_string_lossy(),
            expected_consumer,
            consumer
        );
    }

    Ok(obj)
}

pub fn ensure_allowed_keys(
    obj: &serde_json::Map<String, Value>,
    allowed: &[&str],
    context: &str,
    optional: Option<&[&str]>,
) -> Result<()> {
    let allowed_set: HashSet<&str> = allowed.iter().copied().collect();
    let optional_set: HashSet<&str> = optional.unwrap_or(&[]).iter().copied().collect();

    let mut extra = Vec::new();
    for key in obj.keys() {
        if !allowed_set.contains(key.as_str()) && !optional_set.contains(key.as_str()) {
            extra.push(key.clone());
        }
    }

    if !extra.is_empty() {
        extra.sort();
        bail!("{}: unknown keys {:?}", context, extra);
    }

    Ok(())
}

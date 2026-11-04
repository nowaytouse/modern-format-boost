//! A read-only grouping identity for the admitted media bytes at scan time.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub const HISTORY_SOURCE_SET_EVENT: &str = "MFB_HISTORY_SOURCE_SET";
pub const HISTORY_SOURCE_SET_PREFIX: &str = "MFB_HISTORY_SOURCE_SET=";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySourceSet {
    schema_version: u8,
    source_root: String,
    file_count: u64,
    content_blake3: String,
}

impl HistorySourceSet {
    /// Build evidence from relative paths and already observed, full-file hashes.
    /// An incomplete, empty, or ambiguous inventory has no grouping identity.
    #[must_use]
    pub fn from_observed<'a, Digest: AsRef<str>>(
        root: &Path,
        expected_count: usize,
        files: impl IntoIterator<Item = (&'a Path, Digest)>,
    ) -> Option<Self> {
        let source_root = root.to_str()?;
        if !valid_source_root(source_root) || expected_count == 0 {
            return None;
        }

        let mut entries = BTreeMap::new();
        for (relative, digest) in files {
            if !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
                || relative.components().collect::<PathBuf>().as_os_str() != relative.as_os_str()
            {
                return None;
            }
            let relative = relative.to_str()?;
            if relative.is_empty() || relative.contains('\0') {
                return None;
            }
            let digest = match parse_hash(digest.as_ref()) {
                Ok(hash) => hash,
                Err(error) => {
                    tracing::debug!(%error, "invalid source digest prevents history grouping");
                    return None;
                }
            };
            if entries.insert(relative, digest).is_some() {
                return None;
            }
        }
        if entries.len() != expected_count {
            return None;
        }
        let Ok(file_count) = u64::try_from(expected_count) else {
            tracing::warn!("source file count exceeds history grouping limits");
            return None;
        };
        let mut hasher =
            blake3::Hasher::new_derive_key("Modern Format Boost history source set v1");
        hasher.update(&file_count.to_le_bytes());
        for (relative, digest) in entries {
            let Ok(relative_len) = u64::try_from(relative.len()) else {
                tracing::warn!("relative path length exceeds history grouping limits");
                return None;
            };
            hasher.update(&relative_len.to_le_bytes());
            hasher.update(relative.as_bytes());
            hasher.update(digest.as_bytes());
        }
        Some(Self {
            schema_version: 1,
            source_root: source_root.to_owned(),
            file_count,
            content_blake3: hasher.finalize().to_hex().to_string(),
        })
    }

    /// Validate the compact worker observation before persisting it as history.
    ///
    /// # Errors
    /// Rejects an unsupported, malformed, oversized or mismatched observation.
    pub fn parse_observation(payload: &str, expected_invocation: &str) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Observation {
            invocation_id: String,
            source_set: HistorySourceSet,
        }

        ensure!(
            payload.len() <= 64 * 1024,
            "source set marker exceeds its size limit"
        );
        let Observation {
            invocation_id,
            source_set,
        } = serde_json::from_str(payload).context("invalid source set marker")?;
        ensure!(
            !expected_invocation.is_empty()
                && invocation_id == expected_invocation
                && source_set.schema_version == 1
                && valid_source_root(&source_set.source_root)
                && source_set.file_count > 0,
            "invalid source set evidence"
        );
        parse_hash(&source_set.content_blake3)?;
        Ok(source_set)
    }
}

fn valid_source_root(root: &str) -> bool {
    let path = Path::new(root);
    path.is_absolute()
        && !root.contains('\0')
        && !path
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
}

fn parse_hash(value: &str) -> Result<blake3::Hash> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "history source digest must contain 64 lowercase hexadecimal characters"
    );
    blake3::Hash::from_hex(value).context("parse history source digest")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_identity_needs_every_name_and_full_hash() -> Result<()> {
        let root = std::env::temp_dir().join("mfb-source-set-synthetic");
        let first = PathBuf::from("one").join("a.jpg");
        let second = PathBuf::from("two").join("a.jpg");
        let renamed = PathBuf::from("one").join("renamed.jpg");
        let (first, second, renamed) = (first.as_path(), second.as_path(), renamed.as_path());
        let first_hash = blake3::hash(b"first").to_hex().to_string();
        let second_hash = blake3::hash(b"second").to_hex().to_string();
        let build = |files| HistorySourceSet::from_observed(&root, 2, files);
        let original = build([(first, first_hash.as_str()), (second, second_hash.as_str())])
            .context("complete evidence should have an identity")?;
        let marker = serde_json::json!({"invocation_id": "run-a", "source_set": &original});
        assert_eq!(
            HistorySourceSet::parse_observation(&marker.to_string(), "run-a")?,
            original
        );
        assert!(HistorySourceSet::parse_observation(&marker.to_string(), "run-b").is_err());
        assert_eq!(
            build([(second, second_hash.as_str()), (first, first_hash.as_str())]),
            Some(original.clone())
        );
        assert_ne!(
            build([(first, second_hash.as_str()), (second, first_hash.as_str())]),
            Some(original.clone())
        );
        assert_ne!(
            build([
                (renamed, first_hash.as_str()),
                (second, second_hash.as_str())
            ]),
            Some(original)
        );
        assert!(build([(first, first_hash.as_str()), (first, second_hash.as_str())]).is_none());
        assert!(build([(first, first_hash.as_str()), (second, "missing")]).is_none());
        assert!(
            HistorySourceSet::from_observed(&root, 2, [(first, first_hash.as_str())]).is_none()
        );
        Ok(())
    }
}

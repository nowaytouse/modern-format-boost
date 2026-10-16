use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

const MARKER_PREFIX: &str = "MFB_FAST_IMG_IGNORED=";
const MAX_MARKER_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IgnoredReason {
    LivePhotoPairMember,
    AnimatedImageContainer,
    OutsideCurrentPolicyScope,
    ModernTierNotAdmitted,
}

impl IgnoredReason {
    const fn reason_code(self) -> &'static str {
        match self {
            Self::LivePhotoPairMember => "live_photo_pair_member",
            Self::AnimatedImageContainer => "animated_image_container",
            Self::OutsideCurrentPolicyScope => "outside_current_policy_scope",
            Self::ModernTierNotAdmitted => "modern_tier_not_admitted_for_original_delivery",
        }
    }

    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LivePhotoPairMember => {
                "Live Photo pair member is not eligible for standalone fast-img processing"
            }
            Self::AnimatedImageContainer => {
                "Animated image container is outside the static-only fast-img policy"
            }
            Self::OutsideCurrentPolicyScope => {
                "Source format is outside the current fast-img strategy scope"
            }
            Self::ModernTierNotAdmitted => {
                "Modern-format source was not admitted for original-byte delivery by the current fast-img policy"
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IgnoredItem {
    source_path: String,
    absolute_path: String,
    reason: IgnoredReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IgnoredInventory {
    items: Vec<IgnoredItem>,
}

#[derive(Serialize)]
struct IgnoredMarker<'a> {
    schema_version: u8,
    pipeline: &'static str,
    run_id: &'a str,
    invocation_id: &'a str,
    source_path: &'a str,
    absolute_path: &'a str,
    reason_code: &'static str,
    message: &'static str,
    ordinal: usize,
    total: usize,
}

impl IgnoredInventory {
    pub fn new(
        src_root: &Path,
        candidates: Vec<(PathBuf, IgnoredReason)>,
        expected_count: usize,
    ) -> Result<Self> {
        ensure!(
            src_root.is_absolute(),
            "fast-img source root is not absolute"
        );
        ensure!(
            candidates.len() == expected_count,
            "fast-img ignored inventory count mismatch: classified={} expected={expected_count}",
            candidates.len()
        );

        let mut seen = BTreeSet::new();
        let mut items = Vec::with_capacity(candidates.len());
        for (path, reason) in candidates {
            ensure!(
                path.is_absolute(),
                "fast-img ignored path is not absolute: {}",
                path.display()
            );
            let relative = path.strip_prefix(src_root).with_context(|| {
                format!(
                    "fast-img ignored path escaped source root: source={} root={}",
                    path.display(),
                    src_root.display()
                )
            })?;
            ensure!(
                !relative.as_os_str().is_empty()
                    && relative
                        .components()
                        .all(|component| matches!(component, Component::Normal(_))),
                "fast-img ignored path is not a normalized source-relative path: {}",
                path.display()
            );
            let source_path = relative.to_str().with_context(|| {
                format!(
                    "fast-img ignored relative path is not UTF-8: {}",
                    path.display()
                )
            })?;
            let absolute_path = path.to_str().with_context(|| {
                format!(
                    "fast-img ignored absolute path is not UTF-8: {}",
                    path.display()
                )
            })?;
            ensure!(
                !source_path.contains('\0') && !absolute_path.contains('\0'),
                "fast-img ignored path contains a NUL byte"
            );
            ensure!(
                seen.insert(source_path.to_owned()),
                "fast-img ignored inventory contains duplicate source path: {source_path}"
            );
            items.push(IgnoredItem {
                source_path: source_path.to_owned(),
                absolute_path: absolute_path.to_owned(),
                reason,
            });
        }
        Ok(Self { items })
    }

    #[must_use]
    pub const fn count(&self) -> usize {
        self.items.len()
    }

    pub fn emit_markers_from_environment(&self) -> Result<()> {
        let run_id = environment_identity("MFB_RUN_ID")?;
        let invocation_id = environment_identity("MFB_INVOCATION_ID")?;
        let mut stdout = io::stdout().lock();
        if let Some((run_id, invocation_id)) = resolve_identity(run_id, invocation_id)? {
            let lines = self.marker_lines(&run_id, &invocation_id)?;
            write_lines(
                &mut stdout,
                &lines,
                "write MFB_FAST_IMG_IGNORED marker to stdout",
            )
        } else {
            let lines = self.cli_lines();
            write_lines(
                &mut stdout,
                &lines,
                "write fast-img ignored classification to stdout",
            )
        }
    }

    fn marker_lines(&self, run_id: &str, invocation_id: &str) -> Result<Vec<Vec<u8>>> {
        validate_identity("MFB_RUN_ID", run_id)?;
        validate_identity("MFB_INVOCATION_ID", invocation_id)?;

        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let marker = IgnoredMarker {
                    schema_version: 1,
                    pipeline: "img",
                    run_id,
                    invocation_id,
                    source_path: &item.source_path,
                    absolute_path: &item.absolute_path,
                    reason_code: item.reason.reason_code(),
                    message: item.reason.message(),
                    ordinal: index + 1,
                    total: self.items.len(),
                };
                let json =
                    serde_json::to_vec(&marker).context("serialize MFB_FAST_IMG_IGNORED marker")?;
                let line_len = MARKER_PREFIX
                    .len()
                    .checked_add(json.len())
                    .and_then(|length| length.checked_add(1))
                    .context("MFB_FAST_IMG_IGNORED marker length overflow")?;
                ensure!(
                    line_len <= MAX_MARKER_BYTES,
                    "MFB_FAST_IMG_IGNORED marker exceeds 64 KiB including prefix and newline"
                );
                let mut line = Vec::with_capacity(line_len - 1);
                line.extend_from_slice(MARKER_PREFIX.as_bytes());
                line.extend_from_slice(&json);
                Ok(line)
            })
            .collect()
    }

    fn cli_lines(&self) -> Vec<Vec<u8>> {
        self.items
            .iter()
            .map(|item| {
                format!(
                    "[IGNORED] {:?}: {}",
                    item.source_path,
                    item.reason.message()
                )
                .into_bytes()
            })
            .collect()
    }
}

fn environment_identity(name: &str) -> Result<Option<String>> {
    std::env::var_os(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("{name} is set but is not valid UTF-8"))
        })
        .transpose()
}

fn resolve_identity(
    run_id: Option<String>,
    invocation_id: Option<String>,
) -> Result<Option<(String, String)>> {
    match (run_id, invocation_id) {
        (None, None) => Ok(None),
        (Some(run_id), Some(invocation_id)) => {
            validate_identity("MFB_RUN_ID", &run_id)?;
            validate_identity("MFB_INVOCATION_ID", &invocation_id)?;
            Ok(Some((run_id, invocation_id)))
        }
        (Some(_), None) => bail!(
            "MFB_RUN_ID is set without MFB_INVOCATION_ID; refusing unbound fast-img ignored markers"
        ),
        (None, Some(_)) => bail!(
            "MFB_INVOCATION_ID is set without MFB_RUN_ID; refusing unbound fast-img ignored markers"
        ),
    }
}

fn write_lines(writer: &mut impl Write, lines: &[Vec<u8>], context: &'static str) -> Result<()> {
    for line in lines {
        writer
            .write_all(line)
            .and_then(|()| writer.write_all(b"\n"))
            .context(context)?;
    }
    Ok(())
}

fn validate_identity(name: &str, value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value.bytes().all(|byte| (33..=126).contains(&byte)),
        "{name} must contain 1..=256 visible ASCII bytes"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        IgnoredInventory, IgnoredReason, MARKER_PREFIX, MAX_MARKER_BYTES, resolve_identity,
        write_lines,
    };
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};

    fn inventory(entries: &[(&str, IgnoredReason)]) -> anyhow::Result<IgnoredInventory> {
        let root = Path::new("/tmp/fast-img-source");
        IgnoredInventory::new(
            root,
            entries
                .iter()
                .map(|(relative, reason)| (root.join(relative), *reason))
                .collect(),
            entries.len(),
        )
    }

    #[test]
    fn records_all_explicit_ignored_categories_and_emits_ordered_markers() {
        let values = inventory(&[
            ("live.heic", IgnoredReason::LivePhotoPairMember),
            ("animated.avif", IgnoredReason::AnimatedImageContainer),
            ("outside.mp4", IgnoredReason::OutsideCurrentPolicyScope),
            ("modern.jxl", IgnoredReason::ModernTierNotAdmitted),
        ])
        .expect("valid ignored inventory");
        assert_eq!(values.count(), 4);

        let lines = values
            .marker_lines("run-1", "invocation-1")
            .expect("valid markers");
        assert!(
            lines
                .iter()
                .all(|line| { MARKER_PREFIX.len() + line.len() + 1 <= MAX_MARKER_BYTES })
        );
        let decoded = lines
            .iter()
            .map(|line| {
                let json = std::str::from_utf8(&line["MFB_FAST_IMG_IGNORED=".len()..])
                    .expect("marker JSON is UTF-8");
                serde_json::from_str::<serde_json::Value>(json).expect("marker JSON parses")
            })
            .collect::<Vec<_>>();
        assert_eq!(decoded.len(), 4);
        assert_eq!(decoded[0]["reason_code"], "live_photo_pair_member");
        assert_eq!(decoded[1]["reason_code"], "animated_image_container");
        assert_eq!(decoded[2]["reason_code"], "outside_current_policy_scope");
        assert_eq!(
            decoded[3]["reason_code"],
            "modern_tier_not_admitted_for_original_delivery"
        );
        for (index, marker) in decoded.iter().enumerate() {
            assert_eq!(marker["ordinal"], index + 1);
            assert_eq!(marker["total"], 4);
            assert_eq!(marker["pipeline"], "img");
            assert_eq!(marker["schema_version"], 1);
        }
    }

    #[test]
    fn scan_classifies_synthetic_text_as_outside_policy_without_media_processing() {
        let directory = tempfile::tempdir().expect("temporary source directory");
        let source = directory.path().join("notes.txt");
        std::fs::write(&source, b"synthetic text header only\n").expect("write synthetic text");

        let scan =
            super::super::scan_fast_img_sources(vec![source.clone()], directory.path(), "jxl")
                .expect("text source has an explicit ignored classification");

        assert_eq!(scan.ignored.count(), 1);
        assert_eq!(scan.ignored.items[0].source_path, "notes.txt");
        assert_eq!(
            scan.ignored.items[0].reason,
            IgnoredReason::OutsideCurrentPolicyScope
        );
        assert!(scan.source_files.is_empty());
        assert_eq!(scan.ignored.count(), 1);
    }

    #[test]
    fn rejects_count_mismatch_duplicate_and_escaped_paths() {
        let root = Path::new("/tmp/fast-img-source");
        let reason = IgnoredReason::OutsideCurrentPolicyScope;
        assert!(IgnoredInventory::new(root, vec![(root.join("one.mp4"), reason)], 2).is_err());
        assert!(
            IgnoredInventory::new(
                root,
                vec![
                    (root.join("one.mp4"), reason),
                    (root.join("one.mp4"), reason)
                ],
                2,
            )
            .is_err()
        );
        assert!(
            IgnoredInventory::new(root, vec![(PathBuf::from("/tmp/other/one.mp4"), reason)], 1)
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        let root = Path::new("/tmp/fast-img-source");
        let path = root.join(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
        assert!(
            IgnoredInventory::new(
                root,
                vec![(path, IgnoredReason::OutsideCurrentPolicyScope)],
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_invalid_identity_and_oversized_marker_before_emission() {
        let values = inventory(&[("ignored.mp4", IgnoredReason::OutsideCurrentPolicyScope)])
            .expect("valid ignored inventory");
        assert!(values.marker_lines("", "invocation-1").is_err());
        assert!(resolve_identity(Some("run-1".to_owned()), None).is_err());
        assert!(resolve_identity(None, Some("invocation-1".to_owned())).is_err());
        assert_eq!(resolve_identity(None, None).expect("no identity"), None);

        let root = Path::new("/tmp/fast-img-source");
        let long_relative = format!("{}.mp4", "x".repeat(65 * 1024));
        let large = IgnoredInventory::new(
            root,
            vec![(
                root.join(long_relative),
                IgnoredReason::OutsideCurrentPolicyScope,
            )],
            1,
        )
        .expect("path is structurally valid");
        assert!(large.marker_lines("run-1", "invocation-1").is_err());
    }

    #[test]
    fn reports_marker_output_failure() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic output failure"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = write_lines(
            &mut FailingWriter,
            &[b"marker".to_vec()],
            "write MFB_FAST_IMG_IGNORED marker to stdout",
        )
        .expect_err("output error must be surfaced");
        assert!(
            error
                .to_string()
                .contains("write MFB_FAST_IMG_IGNORED marker")
        );
    }

    #[test]
    fn direct_cli_writer_emits_one_ignored_line_per_item() {
        let values = inventory(&[
            ("live.heic", IgnoredReason::LivePhotoPairMember),
            ("animated.avif", IgnoredReason::AnimatedImageContainer),
            ("outside.mp4", IgnoredReason::OutsideCurrentPolicyScope),
            (
                "line\n[FAIL] injected\x1b[31m\"é.mp4",
                IgnoredReason::OutsideCurrentPolicyScope,
            ),
        ])
        .expect("valid ignored inventory");
        let mut output = Vec::new();
        write_lines(
            &mut output,
            &values.cli_lines(),
            "write fast-img ignored classification to stdout",
        )
        .expect("write CLI inventory");
        let output = String::from_utf8(output).expect("CLI lines are UTF-8");
        let lines = output.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), values.count());
        assert!(lines.iter().all(|line| line.starts_with("[IGNORED] ")));
        assert!(lines.iter().all(|line| !line.starts_with(MARKER_PREFIX)));
        assert!(!output.contains('\x1b'));
        assert!(output.contains("line\\n[FAIL] injected"));
        assert!(output.contains("\\u{1b}[31m"));
        assert!(output.contains("\\\"é.mp4"));
        assert!(!lines.iter().any(|line| line.starts_with("[FAIL]")));
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("\"live.heic\":"))
                .count(),
            1
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("\"animated.avif\":"))
                .count(),
            1
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("\"outside.mp4\":"))
                .count(),
            1
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("line\\n[FAIL] injected"))
                .count(),
            1
        );
    }
}

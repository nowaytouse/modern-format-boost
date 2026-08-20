//! Siegfried (`sf`) + PRONOM sidecar: external format **identity** layer.
//!
//! Responsibility boundary (locked): this module answers "what is this
//! file?" — PUID, format name, MIME, class, evidence basis, warnings. It
//! never decides lossy/lossless state, validity, or decodability; those stay
//! with the per-format inspectors in this crate. The internal magic-byte
//! detectors remain the primary identifier; Siegfried is a fallback for
//! unknown/suspicious inputs and unsupported-format auditing.
//!
//! Failure doctrine: a missing binary, non-zero exit, or unparseable JSON is
//! an identification diagnostic failure — recorded, returned as
//! [`SiegfriedProbe::Unavailable`], and never a panic or pipeline abort.
//! JSON is parsed defensively (every field optional; see upstream JSON
//! escaping issue #280) and zero/one/N matches are all preserved.

use crate::unified_error::Result;
use crate::{SiegfriedBuilder, ToolBuilder};
use std::path::PathBuf;
use std::time::Duration;

/// One PRONOM match for one file. All descriptive fields are optional in the
/// `sf -json` output and default to empty rather than failing the parse.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct SiegfriedMatch {
    #[serde(default)]
    pub ns: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    #[serde(rename = "format")]
    pub format_name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub basis: String,
    #[serde(default)]
    pub warning: String,
}

impl SiegfriedMatch {
    /// A match backed only by the file extension carries no content evidence.
    #[must_use]
    pub fn is_extension_only(&self) -> bool {
        (self.basis.contains("extension match") && !self.basis.contains("byte match"))
            || self.warning.to_ascii_lowercase().contains("extension only")
    }
}

/// Per-file report: every match, plus the `errors` field sf uses for scan
/// failures on individual files.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct SiegfriedFileReport {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub errors: String,
    #[serde(default)]
    pub matches: Vec<SiegfriedMatch>,
}

/// Sidecar and signature-database versions, recorded for reproducibility.
/// Updates are a maintenance step (`sf -update`), never run from here.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct SiegfriedMeta {
    #[serde(default)]
    pub siegfried: String,
    #[serde(default)]
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
struct SiegfriedReport {
    #[serde(flatten)]
    meta: SiegfriedMeta,
    #[serde(default)]
    files: Vec<SiegfriedFileReport>,
}

/// Outcome of an identification batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiegfriedProbe {
    Identified {
        meta: SiegfriedMeta,
        files: Vec<SiegfriedFileReport>,
    },
    Unavailable {
        reason: String,
    },
}

/// Whether the optional `sf` sidecar is resolvable on a trusted tool path.
#[must_use]
pub fn siegfried_available() -> bool {
    SiegfriedBuilder::check_available()
}

/// Upper bound on paths per sidecar invocation. Batches stay batched (one
/// spawn covers up to this many files) while argv stays comfortably bounded.
const IDENTIFY_CHUNK: usize = 256;
const IDENTIFY_ARG_BYTES: usize = 16 * 1024;
const IDENTIFY_SOFT_TIMEOUT: Duration = Duration::from_mins(2);
const IDENTIFY_HARD_TIMEOUT: Duration = Duration::from_mins(10);

/// Identify `paths` with as few sidecar invocations as possible.
///
/// The command is assembled by [`crate::tooling::tool_builders::SiegfriedBuilder`]
/// — trusted-path resolution plus safe-path arguments for every input — so
/// untrusted filenames can never reach a shell position. Paths with no report
/// entry are recorded as per-file errors, not dropped.
///
/// # Errors
/// Returns an error only for programming-level failures; every sidecar-level
/// failure is carried inside [`SiegfriedProbe::Unavailable`].
pub fn identify_paths(paths: &[PathBuf]) -> Result<SiegfriedProbe> {
    if paths.is_empty() {
        return Ok(SiegfriedProbe::Unavailable {
            reason: "no paths to identify".to_string(),
        });
    }
    if !siegfried_available() {
        return Ok(SiegfriedProbe::Unavailable {
            reason: "siegfried (sf) is not installed or not on a trusted tool path".to_string(),
        });
    }

    let mut all_files = Vec::new();
    let mut meta = SiegfriedMeta::default();
    let mut first_failure = None;
    let mut successful_chunks = 0usize;
    let mut start = 0usize;
    while start < paths.len() {
        let mut end = start;
        let mut argv_bytes = 0usize;
        while end < paths.len() && end - start < IDENTIFY_CHUNK {
            let next = paths[end]
                .as_os_str()
                .as_encoded_bytes()
                .len()
                .saturating_add(1);
            if end > start && argv_bytes.saturating_add(next) > IDENTIFY_ARG_BYTES {
                break;
            }
            argv_bytes = argv_bytes.saturating_add(next);
            end += 1;
        }
        let chunk = &paths[start..end];
        match run_sf_chunk(chunk) {
            SiegfriedProbe::Identified {
                meta: chunk_meta,
                files,
            } => {
                if meta.siegfried.is_empty() {
                    meta = chunk_meta;
                }
                all_files.extend(files);
                successful_chunks += 1;
            }
            SiegfriedProbe::Unavailable { reason } => {
                first_failure.get_or_insert_with(|| reason.clone());
                all_files.extend(chunk.iter().map(|path| SiegfriedFileReport {
                    filename: path.to_string_lossy().into_owned(),
                    errors: format!("sf batch failed: {reason}"),
                    matches: Vec::new(),
                }));
            }
        }
        start = end;
    }
    if successful_chunks == 0 {
        return Ok(SiegfriedProbe::Unavailable {
            reason: first_failure.unwrap_or_else(|| "all siegfried batches failed".to_string()),
        });
    }

    // sf omits files it could not open; surface them as explicit per-file
    // errors instead of silent absence.
    let reported: std::collections::HashSet<String> =
        all_files.iter().map(|file| file.filename.clone()).collect();
    for path in paths {
        let name = path.to_string_lossy().into_owned();
        if !reported.contains(&name) {
            all_files.push(SiegfriedFileReport {
                filename: name,
                errors: "sf produced no report entry for this path".to_string(),
                matches: Vec::new(),
            });
        }
    }
    Ok(SiegfriedProbe::Identified {
        meta,
        files: all_files,
    })
}

/// One `sf -json <paths...>` invocation through the project's tool builder.
fn run_sf_chunk(chunk: &[PathBuf]) -> SiegfriedProbe {
    let mut builder = SiegfriedBuilder::new();
    builder.arg("-json");
    for path in chunk {
        builder.input(path);
    }

    let mut command = builder.build();
    let output = match crate::process_runner::run_command_with_liveness_timeout(
        &mut command,
        IDENTIFY_SOFT_TIMEOUT,
        IDENTIFY_HARD_TIMEOUT,
        "siegfried format identification",
    ) {
        Ok(output) => output,
        Err(err) => {
            return SiegfriedProbe::Unavailable {
                reason: format!("failed to execute siegfried sf: {err}"),
            };
        }
    };
    // sf exits non-zero for per-file access errors while still emitting a
    // valid report for the rest of the batch; parse first and degrade to
    // Unavailable only when there is nothing usable to parse.
    parse_sf_output(&output.stdout, &output.status.to_string(), &output.stderr)
}

fn parse_sf_output(stdout: &[u8], status: &str, stderr: &[u8]) -> SiegfriedProbe {
    let stderr_tail = String::from_utf8_lossy(stderr).trim().to_string();
    match serde_json::from_slice::<SiegfriedReport>(stdout) {
        Ok(report) => SiegfriedProbe::Identified {
            meta: report.meta,
            files: report.files,
        },
        Err(err) => SiegfriedProbe::Unavailable {
            reason: format!("sf unusable output (exit {status}: {stderr_tail}): {err}"),
        },
    }
}

/// Audit the unsupported-file bucket: one batched sidecar invocation, then a
/// per-file identity line. Turns "unknown image" into
/// `Unsupported format: <name> | PUID <id> | MIME <mime> | basis | warning`.
///
/// Normal-path contract: quiet when the sidecar is unavailable or the bucket
/// is empty — this only enriches diagnostics, it never gates anything.
pub fn audit_unsupported_identities(paths: &[PathBuf]) {
    if paths.is_empty() || !siegfried_available() {
        return;
    }
    let (meta, files) = match identify_paths(paths) {
        Ok(SiegfriedProbe::Identified { meta, files }) => (meta, files),
        Ok(SiegfriedProbe::Unavailable { reason }) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "siegfried_unsupported_audit",
                format!("external identification unavailable for unsupported bucket: {reason}"),
            );
            return;
        }
        // identify_paths reserves Err for programming-level failures; the
        // audit path degrades to silence rather than propagating them.
        Err(err) => {
            crate::media_conversion_gate::delivery_runtime_batch_audit(
                "siegfried_unsupported_audit",
                format!("external identification errored for unsupported bucket: {err}"),
            );
            return;
        }
    };
    if !meta.siegfried.is_empty() {
        tracing::debug!(
            target: "siegfried_audit",
            siegfried_version = %meta.siegfried,
            signature = %meta.signature,
            "unsupported-bucket identification ran"
        );
    }
    for file in files {
        if !file.errors.is_empty() {
            crate::log_stat!(
                crate::infra::static_logs::messages::LABEL_DETECTION,
                &format!(
                    "Unsupported file {}: external identification error: {}",
                    file.filename, file.errors
                )
            );
            continue;
        }
        match file.matches.as_slice() {
            [] => {
                crate::log_stat!(
                    crate::infra::static_logs::messages::LABEL_DETECTION,
                    &format!(
                        "Unsupported file {}: no format signature matched (unknown content)",
                        file.filename
                    )
                );
            }
            matches => {
                let primary = &matches[0];
                let extra = if matches.len() > 1 {
                    format!(
                        " (+{} more candidate(s): {})",
                        matches.len() - 1,
                        matches[1..]
                            .iter()
                            .map(|m| m.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                } else {
                    String::new()
                };
                let warning = if primary.warning.is_empty() {
                    extra
                } else {
                    format!(" | warning: {}{}", primary.warning, extra)
                };
                crate::log_stat!(
                    crate::infra::static_logs::messages::LABEL_DETECTION,
                    &format!(
                        "Unsupported format: {} | PUID {} | MIME {} | basis: {}{}",
                        primary.format_name,
                        primary.id,
                        if primary.mime.is_empty() {
                            "n/a"
                        } else {
                            primary.mime.as_str()
                        },
                        if primary.basis.is_empty() {
                            "n/a"
                        } else {
                            primary.basis.as_str()
                        },
                        warning
                    )
                );
            }
        }
    }
}

/// Map a PRONOM PUID onto the internal format family.
///
/// Central by design: business logic never compares PUIDs inline. Values are
/// the PUIDs for families `FormatKind` can represent, verified against
/// sf 1.11.6 / `DROID_SignatureFile_V124`. Unmapped PUIDs (the overwhelming
/// majority of PRONOM) intentionally stay `None`: identification without an
/// internal family is a `DetectOnly` result, not an error.
#[must_use]
pub fn puid_to_format_kind(puid: &str) -> Option<super::format_detect::FormatKind> {
    use super::format_detect::FormatKind;
    match puid {
        "fmt/4" => Some(FormatKind::Gif),
        "fmt/11" => Some(FormatKind::Png),
        "fmt/43" => Some(FormatKind::Jpeg),
        "fmt/353" => Some(FormatKind::Tiff),
        "fmt/566" => Some(FormatKind::WebP),
        "fmt/1101" => Some(FormatKind::Heic),
        "fmt/1484" => Some(FormatKind::Jxl),
        "fmt/2062" => Some(FormatKind::Avif),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SINGLE_MATCH_JSON: &str = r#"{
        "siegfried": "1.11.6",
        "signature": "default.sig",
        "files": [
            {
                "filename": "a.png",
                "filesize": 70,
                "matches": [
                    {
                        "ns": "pronom",
                        "id": "fmt/11",
                        "format": "Portable Network Graphics",
                        "version": "1.0",
                        "mime": "image/png",
                        "class": "Image (Raster)",
                        "basis": "byte match at [[0 16] [57 12]]",
                        "warning": ""
                    }
                ]
            }
        ]
    }"#;

    #[test]
    fn parses_single_match_report_with_meta() {
        let report: SiegfriedReport = serde_json::from_str(SINGLE_MATCH_JSON).expect("valid json");
        assert_eq!(report.meta.siegfried, "1.11.6");
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].matches.len(), 1);
        assert_eq!(report.files[0].matches[0].id, "fmt/11");
        assert_eq!(report.files[0].matches[0].mime, "image/png");
        assert!(!report.files[0].matches[0].is_extension_only());
    }

    #[test]
    fn zero_matches_and_missing_fields_parse_defensively() {
        // Minimal shape sf uses for unidentifiable files, plus absent fields.
        let json = r#"{ "files": [ { "filename": "mystery.bin" } ] }"#;
        let report: SiegfriedReport = serde_json::from_str(json).expect("sparse json parses");
        assert_eq!(report.files[0].matches, Vec::new());
        assert_eq!(report.files[0].errors, "");
    }

    #[test]
    fn multiple_matches_are_all_preserved() {
        let json = r#"{
            "files": [
                { "filename": "container", "matches": [ {"id": "fmt/1"}, {"id": "fmt/2"} ] }
            ]
        }"#;
        let report: SiegfriedReport = serde_json::from_str(json).expect("valid json");
        assert_eq!(report.files[0].matches.len(), 2, "no silent matches[0]");
    }

    #[test]
    fn extension_only_detection_uses_basis_and_warning() {
        let mut m = SiegfriedMatch {
            basis: "extension match jxl".to_string(),
            ..SiegfriedMatch::default()
        };
        assert!(m.is_extension_only());
        m.basis = "extension match jxl; byte match at 0, 2".to_string();
        assert!(!m.is_extension_only(), "byte evidence upgrades the match");
        m.basis = String::new();
        m.warning = "match on extension only".to_string();
        assert!(m.is_extension_only());
    }

    #[test]
    fn puid_mapping_covers_internal_families_only() {
        use super::super::format_detect::FormatKind;
        assert_eq!(puid_to_format_kind("fmt/11"), Some(FormatKind::Png));
        assert_eq!(puid_to_format_kind("fmt/2062"), Some(FormatKind::Avif));
        assert_eq!(puid_to_format_kind("fmt/999999"), None);
    }

    #[test]
    fn identify_paths_handles_empty_batch_without_sidecar() {
        let probe = identify_paths(&[]).expect("empty batch is not an error");
        assert!(matches!(probe, SiegfriedProbe::Unavailable { .. }));
    }

    #[test]
    fn nonzero_status_with_valid_json_preserves_partial_report() {
        let probe = parse_sf_output(SINGLE_MATCH_JSON.as_bytes(), "exit status: 2", b"warning");
        assert!(matches!(
            probe,
            SiegfriedProbe::Identified { ref files, .. } if files.len() == 1
        ));
    }

    #[test]
    fn malformed_output_is_unavailable_without_panicking() {
        let probe = parse_sf_output(b"not-json", "exit status: 1", b"broken");
        assert!(matches!(probe, SiegfriedProbe::Unavailable { .. }));
    }
}

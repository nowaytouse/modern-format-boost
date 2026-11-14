//! Persist the actual winning encoder invocation beside its committed media outcome.

use super::{
    PrimaryResultObservation, WorkingCopyMarker, as_io, canonical_bytes, canonical_text,
    checked_marker, in_snapshot, inspect_store_schema_version, key_for_canonical, open_readonly,
    open_store_connection, write_observed_snapshot,
};
use crate::constants::{
    AVIFENC_ARG_JOBS, AVIFENC_ARG_LOSSLESS, AVIFENC_ARG_QUALITY, AVIFENC_ARG_SPEED, JXL_ARG_EFFORT,
    JXL_ARG_LOSSLESS_JPEG, JXL_ARG_THREADS,
};
use crate::pipeline::verification::{FastImgProcessingContract, validate_marker_relative_path};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};

pub const JPEG_EXECUTION_SCHEMA_VERSION: i32 =
    super::super::mfb_sqlite_store::FAST_IMG_JPEG_EXECUTION_SCHEMA_VERSION;
pub const AVIF_EXECUTION_SCHEMA_VERSION: i32 =
    super::super::mfb_sqlite_store::FAST_IMG_AVIF_EXECUTION_SCHEMA_VERSION;
pub const EXECUTION_TABLES: [(&str, i32); 2] = [
    ("fast_img_jpeg_execution", JPEG_EXECUTION_SCHEMA_VERSION),
    ("fast_img_avif_execution", AVIF_EXECUTION_SCHEMA_VERSION),
];
const MAX_RECEIPT_BYTES: usize = 32 * 1024;
const MAX_SOURCE_RECEIPTS: usize = 64;

#[derive(Clone, Copy)]
enum Encoder {
    Jpeg,
    Avif,
}

impl Encoder {
    const fn table(self) -> &'static str {
        self.storage().0
    }

    const fn storage(self) -> (&'static str, i32) {
        EXECUTION_TABLES[self as usize]
    }

    const fn strategy(self) -> &'static str {
        match self {
            Self::Jpeg => "jxl",
            Self::Avif => "avif",
        }
    }

    fn candidate(self, argv: &[String]) -> Option<&str> {
        match self {
            Self::Jpeg => argv.get(1),
            Self::Avif => argv.last(),
        }
        .map(String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncoderInvocation {
    pub executable: String,
    pub executable_blake3: String,
    pub argv: Vec<String>,
    pub idle_timeout_seconds: u64,
    pub hard_timeout_seconds: u64,
    pub exit_code: i32,
}

pub type JpegEncoderInvocation = EncoderInvocation;
pub type AvifEncoderInvocation = EncoderInvocation;

/// Verified private JPEG input and metadata-prepared candidate for existing-JXL replacement.
#[derive(Debug)]
pub struct ReconstructedJpegExecution {
    pub reconstructed_jpeg_blake3: String,
    pub candidate_blake3: String,
    pub encoder: JpegEncoderInvocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JpegExecutionAttempt {
    pub run_id: Option<String>,
    pub invocation_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncoderExecutionReceipt {
    pub schema_version: u8,
    pub task_key: String,
    pub logical_task_key: String,
    pub source_rel: String,
    pub source_blake3: String,
    pub output_rel: String,
    pub output_blake3: String,
    pub attempt: Option<JpegExecutionAttempt>,
    pub processing_contract: FastImgProcessingContract,
    pub encoder: EncoderInvocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_blake3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconstructed_jpeg_blake3: Option<String>,
}

pub type JpegExecutionReceipt = EncoderExecutionReceipt;
pub type AvifExecutionReceipt = EncoderExecutionReceipt;

fn validate_digest(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "Fast IMG execution evidence has an invalid BLAKE3 digest"
    );
    Ok(())
}

fn validate_invocation(invocation: &EncoderInvocation) -> Result<()> {
    validate_digest(&invocation.executable_blake3)?;
    ensure!(
        Path::new(&invocation.executable).is_absolute()
            && !invocation.executable.contains('\0')
            && invocation.executable.len() <= 16 * 1024
            && (2..=32).contains(&invocation.argv.len())
            && invocation.argv.iter().all(|arg| !arg.contains('\0'))
            && invocation.argv.iter().map(String::len).sum::<usize>() <= 16 * 1024
            && invocation.idle_timeout_seconds > 0
            && invocation.hard_timeout_seconds > 0
            && invocation.exit_code == 0,
        "Fast IMG execution must describe one successful bounded encoder invocation"
    );
    Ok(())
}

fn numeric_flag(flags: &[String], flag: &str) -> Option<u32> {
    flags
        .windows(2)
        .find(|pair| pair[0] == flag)
        .and_then(|pair| pair[1].parse().ok())
}

fn validate_jpeg_invocation(invocation: &EncoderInvocation) -> Result<()> {
    let flags = &invocation.argv[2..];
    ensure!(
        flags.iter().filter(|arg| *arg == JXL_ARG_EFFORT).count() == 1
            && flags.iter().filter(|arg| *arg == JXL_ARG_THREADS).count() == 1
            && flags
                .iter()
                .filter(|arg| *arg == JXL_ARG_LOSSLESS_JPEG)
                .count()
                == 1
            && numeric_flag(flags, JXL_ARG_EFFORT).is_some_and(|effort| (1..=11).contains(&effort))
            && numeric_flag(flags, JXL_ARG_THREADS).is_some_and(|threads| threads > 0),
        "Fast IMG JPEG execution flags do not identify reversible JPEG encoding"
    );
    ensure!(
        Path::new(&invocation.argv[1]).is_absolute(),
        "Fast IMG JPEG candidate command has no absolute output path"
    );
    Ok(())
}

fn validate_avif_invocation(invocation: &EncoderInvocation) -> Result<()> {
    let (flags, paths) = invocation.argv.split_at(invocation.argv.len() - 2);
    let count = |flag: &str| flags.iter().filter(|arg| *arg == flag).count();
    let lossless = count(AVIFENC_ARG_LOSSLESS);
    let quality = count(AVIFENC_ARG_QUALITY);
    let jobs = count(AVIFENC_ARG_JOBS);
    ensure!(
        paths.iter().all(|path| Path::new(path).is_absolute())
            && paths[0] != paths[1]
            && count(AVIFENC_ARG_SPEED) == 1
            && numeric_flag(flags, AVIFENC_ARG_SPEED).is_some_and(|speed| speed <= 10)
            && (jobs == 0
                || (jobs == 1
                    && flags
                        .windows(2)
                        .any(|pair| pair[0] == AVIFENC_ARG_JOBS && pair[1] == "all")))
            && lossless <= 1
            && quality <= 1
            && (lossless == 1 || quality == 1)
            && (quality == 0
                || numeric_flag(flags, AVIFENC_ARG_QUALITY)
                    .is_some_and(|value| (10..=100).contains(&value))),
        "Fast IMG AVIF execution flags do not identify an admitted avifenc invocation"
    );
    Ok(())
}

fn logical_task_key(conn: &Connection, key: &str) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT logical_task_key FROM fast_img_retry_tasks WHERE task_key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| key.to_owned()))
}

fn validate_receipt(
    encoder: Encoder,
    receipt: &EncoderExecutionReceipt,
    key: &str,
    logical_key: &str,
    marker: &WorkingCopyMarker,
    source_root: &Path,
) -> Result<()> {
    ensure!(
        receipt.task_key == key
            && receipt.logical_task_key == logical_key
            && marker.strategy == encoder.strategy(),
        "Fast IMG execution evidence does not match the owning task"
    );
    receipt
        .processing_contract
        .validate(encoder.strategy())
        .map_err(anyhow::Error::msg)?;
    validate_marker_relative_path("encoder execution source", &receipt.source_rel)
        .map_err(anyhow::Error::msg)?;
    validate_marker_relative_path("encoder execution output", &receipt.output_rel)
        .map_err(anyhow::Error::msg)?;
    validate_digest(&receipt.source_blake3)?;
    validate_digest(&receipt.output_blake3)?;
    validate_invocation(&receipt.encoder)?;
    match encoder {
        Encoder::Jpeg => {
            validate_jpeg_invocation(&receipt.encoder)?;
            let source = source_root.join(&receipt.source_rel);
            let input = Path::new(&receipt.encoder.argv[0]);
            match (receipt.schema_version, &receipt.reconstructed_jpeg_blake3) {
                (1, None) => ensure!(
                    receipt.candidate_blake3.is_none()
                        && crate::safe_path_arg(&source) == receipt.encoder.argv[0],
                    "Fast IMG JPEG execution command does not match its admitted source"
                ),
                (2, Some(reconstructed_blake3)) => {
                    validate_digest(reconstructed_blake3)?;
                    let candidate = Path::new(&receipt.encoder.argv[1]);
                    let scratch = candidate
                        .parent()
                        .context("Missing JPEG effort scratch path")?;
                    let effort = numeric_flag(&receipt.encoder.argv[2..], JXL_ARG_EFFORT)
                        .context("Missing JPEG effort")?;
                    ensure!(
                        receipt.candidate_blake3.as_ref() == Some(&receipt.output_blake3)
                            && input.is_absolute()
                            && crate::safe_path_arg(&source) != receipt.encoder.argv[0]
                            && input.parent() == scratch.parent()
                            && input
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(
                                    |name| name.starts_with("mfb-jxl-") && name.ends_with(".jpg")
                                )
                            && scratch
                                .file_name()
                                .and_then(|name| name.to_str())
                                .is_some_and(|name| name.starts_with("mfb-jpeg-full-effort-"))
                            && candidate.file_name()
                                == Some(std::ffi::OsStr::new(&format!("e{effort}.jxl")))
                            && Path::new(&receipt.output_rel)
                                .extension()
                                .is_some_and(|suffix| suffix.eq_ignore_ascii_case("jxl")),
                        "Fast IMG reconstructed JPEG execution does not match its verified candidate and output"
                    );
                }
                _ => anyhow::bail!("Unsupported Fast IMG JPEG execution evidence"),
            }
        }
        Encoder::Avif => {
            validate_avif_invocation(&receipt.encoder)?;
            ensure!(
                receipt.schema_version == 1
                    && receipt.reconstructed_jpeg_blake3.is_none()
                    && receipt.candidate_blake3.as_deref() == Some(receipt.output_blake3.as_str())
                    && Path::new(&receipt.output_rel)
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("avif")),
                "Fast IMG AVIF execution candidate does not match its published AVIF output"
            );
        }
    }
    if let Some(attempt) = &receipt.attempt {
        for id in attempt
            .run_id
            .iter()
            .chain(std::iter::once(&attempt.invocation_id))
        {
            ensure!(
                !id.is_empty() && id.len() <= 256 && id.bytes().all(|byte| byte.is_ascii_graphic()),
                "Fast IMG execution attempt identity is invalid"
            );
        }
    }
    Ok(())
}

fn receipt_key(encoder: Encoder, receipt: &EncoderExecutionReceipt) -> Result<blake3::Hash> {
    let candidate = encoder
        .candidate(&receipt.encoder.argv)
        .context("Fast IMG execution lacks a candidate path")?;
    Ok(blake3::hash(&canonical_bytes(&(
        &receipt.task_key,
        &receipt.source_rel,
        &receipt.attempt,
        candidate,
    ))?))
}

fn decode_receipt(
    encoder: Encoder,
    key: &[u8],
    bytes: &[u8],
    digest: &[u8],
) -> Result<EncoderExecutionReceipt> {
    ensure!(
        bytes.len() <= MAX_RECEIPT_BYTES && blake3::hash(bytes).as_bytes().as_slice() == digest,
        "Fast IMG execution payload failed BLAKE3 verification"
    );
    let receipt: EncoderExecutionReceipt = serde_json::from_slice(bytes)?;
    ensure!(
        receipt.encoder.argv.len() >= 2
            && receipt_key(encoder, &receipt)?.as_bytes().as_slice() == key,
        "Fast IMG execution identity failed BLAKE3 verification"
    );
    Ok(receipt)
}

fn read_receipt_row(
    row: &rusqlite::Row<'_>,
    encoder: Encoder,
    key: &str,
    logical_key: &str,
    marker: &WorkingCopyMarker,
) -> Result<(Vec<u8>, EncoderExecutionReceipt)> {
    let stored_relative: String = row.get(0)?;
    let receipt_key: Vec<u8> = row.get(1)?;
    let receipt = decode_receipt(
        encoder,
        &receipt_key,
        row.get_ref(2)?.as_blob()?,
        row.get_ref(3)?.as_blob()?,
    )?;
    ensure!(
        stored_relative == receipt.source_rel,
        "Fast IMG execution row does not match its source identity"
    );
    validate_receipt(encoder, &receipt, key, logical_key, marker, &marker.src_dir)?;
    Ok((receipt_key, receipt))
}

pub(super) fn validate_receipts(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
    schema_version: Option<i32>,
) -> Result<()> {
    let logical_key = logical_task_key(conn, key)?;
    for encoder in [Encoder::Jpeg, Encoder::Avif] {
        let (table, required_since) = encoder.storage();
        if schema_version.is_none_or(|version| version < required_since) {
            continue;
        }
        let mut statement = conn.prepare(&format!(
            "SELECT source_rel,receipt_key,payload,payload_blake3 FROM {table}
             WHERE task_key=?1 ORDER BY source_rel,receipt_key",
        ))?;
        let mut rows = statement.query([key])?;
        while let Some(row) = rows.next()? {
            read_receipt_row(row, encoder, key, &logical_key, marker)?;
        }
    }
    Ok(())
}

/// Commit a winning invocation together with its exact published JPEG-to-JXL outcome.
///
/// # Errors
/// Rejects incomplete bindings, conflicting receipt writes or unavailable state.
pub fn write_marker_with_jpeg_execution(
    marker: &WorkingCopyMarker,
    source_rel: &str,
    encoder: JpegEncoderInvocation,
    attempt: Option<JpegExecutionAttempt>,
) -> io::Result<()> {
    write_marker_with_execution(
        Encoder::Jpeg,
        marker,
        source_rel,
        None,
        None,
        encoder,
        attempt,
    )
}

/// Commit an existing-JXL replacement together with the invocation that produced it.
///
/// # Errors
/// Rejects incomplete bindings, conflicting receipt writes or unavailable state.
pub fn write_marker_with_reconstructed_jpeg_execution(
    marker: &WorkingCopyMarker,
    source_rel: &str,
    proof: ReconstructedJpegExecution,
    attempt: Option<JpegExecutionAttempt>,
) -> io::Result<()> {
    write_marker_with_execution(
        Encoder::Jpeg,
        marker,
        source_rel,
        Some(proof.candidate_blake3),
        Some(proof.reconstructed_jpeg_blake3),
        proof.encoder,
        attempt,
    )
}

/// Commit a winning avifenc invocation and the unchanged verified candidate with its output.
///
/// # Errors
/// Rejects missing or conflicting bindings, state, and candidate/output digest mismatches.
pub fn write_marker_with_avif_execution(
    marker: &WorkingCopyMarker,
    source_rel: &str,
    candidate_blake3: &str,
    encoder: AvifEncoderInvocation,
    attempt: Option<JpegExecutionAttempt>,
) -> io::Result<()> {
    write_marker_with_execution(
        Encoder::Avif,
        marker,
        source_rel,
        Some(candidate_blake3.to_owned()),
        None,
        encoder,
        attempt,
    )
}

fn write_marker_with_execution(
    kind: Encoder,
    marker: &WorkingCopyMarker,
    source_rel: &str,
    candidate_blake3: Option<String>,
    reconstructed_jpeg_blake3: Option<String>,
    encoder: EncoderInvocation,
    attempt: Option<JpegExecutionAttempt>,
) -> io::Result<()> {
    (|| -> Result<()> {
        super::migration::reject_unmigrated_for_working_copy(&marker.working_copy)?;
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let wc = canonical_text(&marker.working_copy)?;
        let source = canonical_text(&marker.src_dir)?;
        let task_key = key_for_canonical(&wc);
        let logical_key = logical_task_key(&tx, &task_key)?;
        let entry = marker.blake3_log.get(source_rel)
            .context("Fast IMG execution has no published output outcome")?;
        let receipt = EncoderExecutionReceipt {
            schema_version: if reconstructed_jpeg_blake3.is_some() { 2 } else { 1 },
            task_key,
            logical_task_key: logical_key,
            source_rel: source_rel.to_owned(),
            source_blake3: entry.src.clone(),
            output_rel: entry.out_rel.clone().context("Encoder execution output has no exact path")?,
            output_blake3: entry.out.clone(),
            attempt,
            processing_contract: marker.processing_contract.clone()
                .context("Encoder execution has no processing contract")?,
            encoder,
            candidate_blake3,
            reconstructed_jpeg_blake3,
        };
        validate_receipt(kind, &receipt, &receipt.task_key, &receipt.logical_task_key, marker, Path::new(&source))?;
        let observation = PrimaryResultObservation {
            attempt: receipt.attempt.clone(),
            phase: crate::infra::static_logs::FileFailurePhase::EncodeOrPublish,
            adopted_existing_jxl: false,
            reason_code: None,
        };
        // Snapshot write already verifies its complete SQL readback before return.
        write_observed_snapshot(&tx, marker, Some((source_rel, &observation)))?;
        let payload = canonical_bytes(&receipt)?;
        ensure!(payload.len() <= MAX_RECEIPT_BYTES, "Fast IMG execution payload exceeds limit");
        let key = receipt_key(kind, &receipt)?;
        let table = kind.table();
        tx.execute(
            &format!("INSERT INTO {table}(task_key,source_rel,receipt_key,payload,payload_blake3)
             VALUES(?1,?2,?3,?4,?5) ON CONFLICT(task_key,source_rel,receipt_key) DO NOTHING"),
            params![receipt.task_key, receipt.source_rel, key.as_bytes().as_slice(), payload, blake3::hash(&payload).as_bytes().as_slice()],
        )?;
        let actual = tx.query_row(
            &format!("SELECT payload,payload_blake3 FROM {table} WHERE task_key=?1 AND source_rel=?2 AND receipt_key=?3"),
            params![receipt.task_key, receipt.source_rel, key.as_bytes().as_slice()],
            |row| Ok(decode_receipt(kind, key.as_bytes(), row.get_ref(0)?.as_blob()?, row.get_ref(1)?.as_blob()?)),
        )??;
        ensure!(
            actual == receipt,
            "Fast IMG execution receipt changed; transaction must roll back"
        );
        tx.commit().context("Cannot commit Fast IMG execution and output together")
    })().map_err(as_io)
}

#[derive(Debug)]
pub struct EncoderExecutionReceiptPage {
    pub receipts: Vec<EncoderExecutionReceipt>,
    pub next_cursor: Option<String>,
}
pub type JpegExecutionReceiptPage = EncoderExecutionReceiptPage;
pub type AvifExecutionReceiptPage = EncoderExecutionReceiptPage;

/// Read all historical encoder evidence for an exact source, up to the diagnostic limit.
/// Matching bytes alone do not prove an invocation ran in the current attempt.
/// # Errors
/// Rejects unavailable/corrupt state, changed ownership, or more than 64 receipts.
pub fn read_jpeg_execution_receipts_for_source(
    working_copy: &Path,
    source_relative: &str,
) -> io::Result<Vec<JpegExecutionReceipt>> {
    let page = read_jpeg_execution_receipt_page(working_copy, source_relative, None)?;
    if page.next_cursor.is_some() {
        return Err(io::Error::other(
            "Fast IMG JPEG execution history exceeds the 64-receipt per-source diagnostic limit; use paged diagnostics",
        ));
    }
    Ok(page.receipts)
}

/// Read up to 64 historical receipts in deterministic key order for an exact source.
/// Pass a cursor returned for this task and source to continue after that receipt.
/// Each call sees a new store snapshot, so concurrent appends can change later pages.
/// # Errors
/// Rejects invalid or foreign cursors, unavailable/corrupt state, or changed ownership.
pub fn read_jpeg_execution_receipt_page(
    working_copy: &Path,
    source_relative: &str,
    after: Option<&str>,
) -> io::Result<JpegExecutionReceiptPage> {
    read_execution_receipt_page(Encoder::Jpeg, working_copy, source_relative, after)
}

/// Read up to 64 historical AVIF receipts for one exact working copy and source.
///
/// # Errors
/// Rejects invalid or foreign cursors and unavailable or corrupt state.
pub fn read_avif_execution_receipt_page(
    working_copy: &Path,
    source_relative: &str,
    after: Option<&str>,
) -> io::Result<AvifExecutionReceiptPage> {
    read_execution_receipt_page(Encoder::Avif, working_copy, source_relative, after)
}

fn read_execution_receipt_page(
    encoder: Encoder,
    working_copy: &Path,
    source_relative: &str,
    after: Option<&str>,
) -> io::Result<EncoderExecutionReceiptPage> {
    (|| -> Result<EncoderExecutionReceiptPage> {
        validate_marker_relative_path("encoder execution query", source_relative)
            .map_err(anyhow::Error::msg)?;
        let after = after
            .map(|value| -> Result<_> {
                validate_digest(value).context("Invalid Fast IMG execution cursor")?;
                Ok(blake3::Hash::from_hex(value)?)
            })
            .transpose()?;
        let empty = || -> Result<EncoderExecutionReceiptPage> {
            ensure!(
                after.is_none(),
                "Fast IMG execution cursor is not recorded for this task and source"
            );
            Ok(EncoderExecutionReceiptPage {
                receipts: Vec::new(),
                next_cursor: None,
            })
        };
        let (table, required_since) = encoder.storage();
        super::migration::reject_unmigrated_for_working_copy(working_copy)?;
        let Some(conn) = open_readonly()? else {
            return empty();
        };
        let wc = canonical_text(working_copy)?;
        in_snapshot(&conn, |conn| {
            if inspect_store_schema_version(conn)?.is_none_or(|version| version < required_since) {
                return empty();
            }
            let Some(marker) = checked_marker(conn, &wc)? else {
                return empty();
            };
            let key = key_for_canonical(&wc);
            let logical_key = logical_task_key(conn, &key)?;
            if let Some(cursor) = &after {
                conn.query_row(
                    &format!(
                        "SELECT source_rel,receipt_key,payload,payload_blake3 FROM {table}
                     WHERE task_key=?1 AND source_rel=?2 AND receipt_key=?3"
                    ),
                    params![key, source_relative, cursor.as_bytes().as_slice()],
                    |row| Ok(read_receipt_row(row, encoder, &key, &logical_key, &marker)),
                )
                .optional()?
                .context("Fast IMG execution cursor is not recorded for this task and source")??;
            }
            let after = after
                .as_ref()
                .map_or(&[][..], |key| key.as_bytes().as_slice());
            let mut statement = conn.prepare(&format!(
                "SELECT source_rel,receipt_key,payload,payload_blake3 FROM {table}
                 WHERE task_key=?1 AND source_rel=?2 AND receipt_key>?3
                 ORDER BY receipt_key LIMIT ?4"
            ))?;
            let mut rows = statement.query(params![
                key,
                source_relative,
                after,
                i64::try_from(MAX_SOURCE_RECEIPTS + 1)?
            ])?;
            let mut receipts = Vec::new();
            let mut last_key: Option<Vec<u8>> = None;
            while let Some(row) = rows.next()? {
                let (receipt_key, receipt) =
                    read_receipt_row(row, encoder, &key, &logical_key, &marker)?;
                if receipts.len() == MAX_SOURCE_RECEIPTS {
                    let cursor = last_key.context("Missing Fast IMG execution page cursor")?;
                    let cursor =
                        blake3::Hash::from_bytes(cursor.try_into().map_err(|_| {
                            anyhow::anyhow!("Invalid Fast IMG execution receipt key")
                        })?);
                    return Ok(EncoderExecutionReceiptPage {
                        receipts,
                        next_cursor: Some(cursor.to_hex().to_string()),
                    });
                }
                receipts.push(receipt);
                last_key = Some(receipt_key);
            }
            Ok(EncoderExecutionReceiptPage {
                receipts,
                next_cursor: None,
            })
        })
    })()
    .map_err(as_io)
}

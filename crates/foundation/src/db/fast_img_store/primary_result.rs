//! Observed primary outcomes share a transaction with the authoritative snapshot.

use super::*;
use crate::infra::static_logs::FileFailurePhase;
use crate::pipeline::verification::validate_marker_relative_path;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrimaryResultObservation {
    pub attempt: Option<JpegExecutionAttempt>,
    pub phase: FileFailurePhase,
    pub adopted_existing_jxl: bool,
    pub reason_code: Option<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrimaryResultEvidence {
    pub task_revision: i64,
    pub outcome_blake3: Option<String>,
    pub observation: Option<PrimaryResultObservation>,
}

pub(super) fn outcome_digest(row: &SourceOutcome<'_>) -> Result<String> {
    // A later Photos receipt can change the library hash without changing the source result.
    let outcome = (
        row.disposition,
        row.source,
        row.output_rel,
        row.output,
        row.reason,
    );
    Ok(blake3::hash(&canonical_bytes(&outcome)?)
        .to_hex()
        .to_string())
}

fn validate_ids(observation: &PrimaryResultObservation) -> Result<()> {
    if let Some(attempt) = &observation.attempt {
        for id in attempt
            .run_id
            .iter()
            .chain(std::iter::once(&attempt.invocation_id))
        {
            ensure!(
                !id.is_empty() && id.len() <= 256 && id.bytes().all(|byte| byte.is_ascii_graphic()),
                "Fast IMG primary result has invalid attempt identity"
            );
        }
    }
    if let Some(code) = &observation.reason_code {
        ensure!(
            !code.is_empty()
                && code.len() <= 256
                && code.bytes().all(|byte| byte.is_ascii_graphic()),
            "Fast IMG primary result has invalid reason code"
        );
    }
    Ok(())
}

fn bind_outcome(evidence: &PrimaryResultEvidence, row: Option<&SourceOutcome<'_>>) -> Result<()> {
    ensure!(
        evidence.outcome_blake3 == row.map(outcome_digest).transpose()?,
        "Fast IMG primary result evidence does not match the current source disposition"
    );
    if let Some(observation) = &evidence.observation {
        let row = row.context("Observed Fast IMG primary result has no outcome")?;
        ensure!(
            matches!(
                observation.phase,
                FileFailurePhase::EncodeOrPublish | FileFailurePhase::LocalVerification
            ) && (observation.phase == FileFailurePhase::EncodeOrPublish
                || row.disposition == "failed")
                && (observation.reason_code.is_none() || row.disposition == "failed")
                && (!observation.adopted_existing_jxl
                    || (row.disposition == "converted" && row.output == Some(row.source))),
            "Fast IMG primary result phase or adopted outcome does not match its observation"
        );
    }
    Ok(())
}

pub(super) fn has_rows(conn: &Connection, key: &str, source: Option<&str>) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM fast_img_primary_results
         WHERE task_key=?1 AND (?2 IS NULL OR source_rel=?2))",
        params![key, source],
        |row| row.get(0),
    )?)
}

fn evidence_digest(key: &str, source: &str, payload: &[u8]) -> Result<blake3::Hash> {
    Ok(blake3::hash(&canonical_bytes(&(
        key,
        source,
        blake3::hash(payload).to_hex().as_str(),
    ))?))
}

pub(super) fn write(
    conn: &Connection,
    key: &str,
    source: &str,
    revision: i64,
    row: Option<&SourceOutcome<'_>>,
    observation: Option<&PrimaryResultObservation>,
) -> Result<()> {
    validate_marker_relative_path("primary result", source).map_err(anyhow::Error::msg)?;
    let evidence = PrimaryResultEvidence {
        task_revision: revision,
        outcome_blake3: row.map(outcome_digest).transpose()?,
        observation: observation.cloned(),
    };
    if let Some(observation) = observation {
        validate_ids(observation)?;
    }
    bind_outcome(&evidence, row)?;
    let payload = canonical_bytes(&evidence)?;
    conn.execute(
        "INSERT INTO fast_img_primary_results(task_key,source_rel,revision,payload,payload_blake3)
         VALUES(?1,?2,?3,?4,?5)",
        params![
            key,
            source,
            revision,
            payload,
            evidence_digest(key, source, &payload)?
                .as_bytes()
                .as_slice()
        ],
    )?;
    let saved = conn.query_row(
        "SELECT source_rel,revision,payload,payload_blake3 FROM fast_img_primary_results
         WHERE task_key=?1 AND source_rel=?2 AND revision=?3",
        params![key, source, revision],
        |row| Ok(decode(key, row, revision)),
    )??;
    ensure!(
        saved.1 == evidence,
        "Fast IMG primary result readback does not match the committed outcome"
    );
    Ok(())
}

pub(super) fn forget_prior(
    conn: &Connection,
    key: &str,
    source: &str,
    revision: i64,
    row: Option<&SourceOutcome<'_>>,
) -> Result<()> {
    // A later unobserved change closes the attribution even if an older outcome recurs.
    if has_rows(conn, key, Some(source))? {
        write(conn, key, source, revision, row, None)?;
    }
    Ok(())
}

fn decode(
    key: &str,
    row: &rusqlite::Row<'_>,
    latest_revision: i64,
) -> Result<(String, PrimaryResultEvidence)> {
    let source: String = row.get(0)?;
    let revision: i64 = row.get(1)?;
    let payload: Vec<u8> = row.get(2)?;
    let digest: Vec<u8> = row.get(3)?;
    validate_marker_relative_path("primary result", &source).map_err(anyhow::Error::msg)?;
    ensure!(
        (0..=latest_revision).contains(&revision)
            && evidence_digest(key, &source, &payload)?
                .as_bytes()
                .as_slice()
                == digest,
        "Fast IMG primary result revision or BLAKE3 is invalid"
    );
    let evidence: PrimaryResultEvidence = serde_json::from_slice(&payload)?;
    ensure!(
        evidence.task_revision == revision
            && evidence
                .outcome_blake3
                .as_deref()
                .is_none_or(cleanup::hash_is_valid)
            && (evidence.observation.is_none() || evidence.outcome_blake3.is_some()),
        "Malformed Fast IMG primary result evidence"
    );
    if let Some(observation) = &evidence.observation {
        validate_ids(observation)?;
    }
    Ok((source, evidence))
}

pub(super) fn current(
    conn: &Connection,
    key: &str,
    source: &str,
    revision: i64,
    marker: &WorkingCopyMarker,
) -> Result<Option<PrimaryResultEvidence>> {
    if inspect_store_schema_version(conn)?
        .is_none_or(|schema| schema < FAST_IMG_PRIMARY_RESULT_SCHEMA_VERSION)
    {
        return Ok(None);
    }
    let mut statement = conn.prepare("SELECT source_rel,revision,payload,payload_blake3
        FROM fast_img_primary_results WHERE task_key=?1 AND source_rel=?2 ORDER BY revision DESC LIMIT 1")?;
    let mut rows = statement.query(params![key, source])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let (_, evidence) = decode(key, row, revision)?;
    bind_outcome(&evidence, source_outcome(marker, source).as_ref())?;
    Ok(evidence.observation.is_some().then_some(evidence))
}

pub(super) fn validate_history(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<()> {
    let revision = conn.query_row(
        "SELECT revision FROM fast_img_tasks WHERE task_key=?1",
        [key],
        |row| row.get(0),
    )?;
    let outcomes = source_outcomes(marker);
    let mut statement = conn.prepare(
        "SELECT source_rel,revision,payload,payload_blake3
        FROM fast_img_primary_results WHERE task_key=?1 ORDER BY source_rel,revision DESC",
    )?;
    let mut rows = statement.query([key])?;
    let mut previous = String::new();
    while let Some(row) = rows.next()? {
        let (source, evidence) = decode(key, row, revision)?;
        if previous != source {
            bind_outcome(&evidence, outcomes.get(source.as_str()))?;
            previous = source;
        }
    }
    Ok(())
}

/// Commit the precisely observed primary result with its working-copy checkpoint.
///
/// # Errors
/// Rejects incompatible observations or any transaction failure.
pub fn write_marker_with_primary_result(
    marker: &WorkingCopyMarker,
    source: &str,
    observation: PrimaryResultObservation,
) -> io::Result<()> {
    (|| -> Result<()> {
        migration::reject_unmigrated_for_working_copy(&marker.working_copy)?;
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_observed_snapshot(&tx, marker, Some((source, &observation)))?;
        tx.commit()
            .context("Cannot commit Fast IMG primary result; prior state retained")
    })()
    .map_err(as_io)
}

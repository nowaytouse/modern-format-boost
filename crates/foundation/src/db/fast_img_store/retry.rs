//! Immutable failed-source admission manifests; mutable outcomes remain in the task store.

use super::*;
use crate::batch::planned_source::PlannedSource;
use crate::pipeline::verification::FastImgStageName;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FailedRetrySidecar {
    Absent,
    Present {
        relative_path: String,
        blake3: String,
        claim: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedRetryAdmission {
    pub source_rel: String,
    pub parent_source_blake3: String,
    pub admitted_source_blake3: String,
    pub claim: String,
    pub xmp: FailedRetrySidecar,
    pub aae: FailedRetrySidecar,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FailedRetryDeliveryIntent {
    Local,
    Photos {
        library_path: Option<PathBuf>,
        naming_policy: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedRetryToolAuthority {
    pub manifest_path: PathBuf,
    pub identity: String,
}

/// A verified parent snapshot, not a caller-constructed claim of ownership.
#[derive(Clone, Debug)]
pub struct FailedRetryParent {
    marker: WorkingCopyMarker,
    key: String,
    revision: i64,
    hash: String,
    request: String,
    child: PathBuf,
}

impl FailedRetryParent {
    #[must_use]
    pub fn marker(&self) -> &WorkingCopyMarker {
        &self.marker
    }
    #[must_use]
    pub fn task_key(&self) -> &str {
        &self.key
    }
    #[must_use]
    pub fn snapshot_blake3(&self) -> &str {
        &self.hash
    }
    #[must_use]
    pub fn request_key(&self) -> &str {
        &self.request
    }
    #[must_use]
    pub fn child_working_copy(&self) -> &Path {
        &self.child
    }
}

#[derive(Clone, Debug)]
pub struct FailedRetryContext {
    ancestors: Vec<WorkingCopyMarker>,
    admissions: BTreeMap<String, FailedRetryAdmission>,
    parent_key: String,
    delivery: FailedRetryDeliveryIntent,
}

impl FailedRetryContext {
    #[must_use]
    pub fn parent(&self) -> &WorkingCopyMarker {
        &self.ancestors[0]
    }
    #[must_use]
    pub fn ancestors(&self) -> &[WorkingCopyMarker] {
        &self.ancestors
    }
    #[must_use]
    pub fn admissions(&self) -> &BTreeMap<String, FailedRetryAdmission> {
        &self.admissions
    }
    #[must_use]
    pub fn parent_task_key(&self) -> &str {
        &self.parent_key
    }
    #[must_use]
    pub fn requested_delivery(&self) -> &FailedRetryDeliveryIntent {
        &self.delivery
    }
}

#[derive(Clone, Debug, Serialize)]
struct Relation {
    parent: String,
    revision: i64,
    snapshot: Vec<u8>,
    started: String,
    logical: String,
    request: String,
    manifest: Vec<u8>,
    delivery: FailedRetryDeliveryIntent,
    count: usize,
}

fn has_schema(conn: &Connection) -> Result<bool> {
    Ok(inspect_store_schema_version(conn)? == Some(5))
}

fn hash_text(value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit()),
        "Failed retry requires an exact 64-hex BLAKE3 identity; no historical identity was invented"
    );
    Ok(())
}

fn validate_delivery(delivery: &FailedRetryDeliveryIntent) -> Result<()> {
    if let FailedRetryDeliveryIntent::Photos {
        library_path,
        naming_policy,
    } = delivery
    {
        if let Some(path) = library_path {
            validate_locator(text(path)?)?;
        }
        let (root, album, _): (Option<String>, Option<String>, bool) =
            serde_json::from_str(naming_policy)
                .context("Failed retry Photos naming intent is not a supported typed policy")?;
        ensure!(
            root.as_deref().is_none_or(|value| !value.contains('\0'))
                && album.as_deref().is_none_or(|value| !value.contains('\0')),
            "Failed retry Photos naming intent contains invalid text"
        );
    }
    Ok(())
}

fn metadata_policy_version(marker: &WorkingCopyMarker) -> Result<u32> {
    use crate::pipeline::verification::FastImgEncodingParameters;
    Ok(
        match &marker
            .processing_contract
            .as_ref()
            .context("Retry processing intent is missing")?
            .encoding
        {
            FastImgEncodingParameters::Jxl { .. } => 0,
            FastImgEncodingParameters::Avif {
                avif_metadata_policy,
                ..
            } => avif_metadata_policy.version(),
        },
    )
}

fn relative(value: &str) -> Result<&Path> {
    let path = Path::new(value);
    let normalized: PathBuf = path.components().collect();
    ensure!(
        !value.is_empty()
            && normalized.as_os_str() == path.as_os_str()
            && path
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            && !value.contains('\0'),
        "Failed retry contains an invalid relative path"
    );
    Ok(path)
}

fn validate_parent(marker: &WorkingCopyMarker) -> Result<()> {
    ensure!(
        marker.stage == FastImgStageName::CleanupComplete && !marker.failed_sources.is_empty(),
        "Only a completed task with retained failures can create a failed-only child"
    );
    ensure!(
        marker.recorded_source_count() == marker.src_jpeg_count,
        "Failed-only retry requires complete parent source dispositions"
    );
    ensure!(
        marker.processing_contract.is_some(),
        "Failed-only retry requires the parent's recorded processing intent; old defaults were not guessed"
    );
    for (rel, failure) in &marker.failed_sources {
        relative(rel)?;
        hash_text(&failure.src)?;
    }
    Ok(())
}

fn request_key(key: &str, hash: &str) -> Result<String> {
    Ok(
        blake3::hash(&canonical_bytes(&("fast-img-failed-retry-v1", key, hash))?)
            .to_hex()
            .to_string(),
    )
}

fn child_path(marker: &WorkingCopyMarker, request: &str) -> Result<PathBuf> {
    let name = marker
        .src_dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("Retry source has no supported name")?;
    let parent = marker
        .working_copy
        .parent()
        .context("Retry parent output has no parent")?;
    let child = parent.join(format!("{name}_optimized_retry_{}", &request[..16]));
    validate_locator(text(&child)?)?;
    ensure!(
        child != marker.working_copy
            && !child.starts_with(&marker.src_dir)
            && !marker.src_dir.starts_with(&child),
        "Retry output overlaps its parent or source"
    );
    Ok(child)
}

fn parent_ref(conn: &Connection, key: &str) -> Result<FailedRetryParent> {
    let marker = read_snapshot(conn, key)?.context("Failed retry parent task is missing")?;
    validate_parent(&marker)?;
    let (revision, hash): (i64, Vec<u8>) = conn.query_row(
        "SELECT revision,snapshot_blake3 FROM fast_img_tasks WHERE task_key=?1",
        [key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    ensure!(
        hash.len() == 32 && revision >= 0,
        "Retry parent has invalid snapshot evidence"
    );
    let hash = blake3::Hash::from_bytes(
        hash.try_into()
            .map_err(|_| anyhow::anyhow!("Invalid parent digest"))?,
    )
    .to_hex()
    .to_string();
    let request = request_key(key, &hash)?;
    let child = child_path(&marker, &request)?;
    Ok(FailedRetryParent {
        marker,
        key: key.to_owned(),
        revision,
        hash,
        request,
        child,
    })
}

fn recheck_parent(conn: &Connection, expected: &FailedRetryParent) -> Result<FailedRetryParent> {
    let current = parent_ref(conn, &expected.key)?;
    ensure!(
        current.revision == expected.revision
            && current.hash == expected.hash
            && current.marker == expected.marker
            && current.request == expected.request
            && current.child == expected.child,
        "Failed retry parent changed after inspection; neither task was reset"
    );
    Ok(current)
}

/// Inspect retained failures without creating a database, output, or admission facts.
///
/// # Errors
/// Rejects unavailable, incomplete, changed, or unsupported parent state.
pub fn inspect_failed_retry_parent(parent_wc: &Path) -> io::Result<FailedRetryParent> {
    (|| -> Result<_> {
        let conn = open_readonly()?.context("Failed retry requires an existing formal store")?;
        let wc = canonical_text(parent_wc)?;
        in_snapshot(&conn, |conn| parent_ref(conn, &key_for_canonical(&wc)))
    })()
    .map_err(as_io)
}

fn relation(conn: &Connection, key: &str) -> Result<Option<Relation>> {
    if !has_schema(conn)? {
        return Ok(None);
    }
    conn.query_row("SELECT parent_task_key,parent_revision,parent_snapshot_blake3,parent_started_at,
        logical_task_key,request_key,manifest_blake3,source_count,requested_delivery FROM fast_img_retry_tasks WHERE task_key=?1",
        [key], |row| Ok(Relation { parent:row.get(0)?,revision:row.get(1)?,snapshot:row.get(2)?,started:row.get(3)?,
            logical:row.get(4)?,request:row.get(5)?,manifest:row.get(6)?,count:usize::try_from(row.get::<_,i64>(7)?).map_err(|error|
                rusqlite::Error::FromSqlConversionFailure(7,rusqlite::types::Type::Integer,Box::new(error)))?,
            delivery:serde_json::from_str(&row.get::<_,String>(8)?).map_err(|error|
                rusqlite::Error::FromSqlConversionFailure(8,rusqlite::types::Type::Text,Box::new(error)))? })).optional().map_err(Into::into)
}

fn sidecar_from_row(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<FailedRetrySidecar> {
    match (
        row.get::<_, Option<String>>(offset)?,
        row.get::<_, Option<String>>(offset + 1)?,
        row.get::<_, Option<String>>(offset + 2)?,
    ) {
        (None, None, None) => Ok(FailedRetrySidecar::Absent),
        (Some(relative_path), Some(blake3), Some(claim)) => Ok(FailedRetrySidecar::Present {
            relative_path,
            blake3,
            claim,
        }),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn admissions(conn: &Connection, key: &str) -> Result<BTreeMap<String, FailedRetryAdmission>> {
    let values = conn.prepare("SELECT source_rel,parent_source_blake3,admitted_source_blake3,source_claim,
        xmp_rel,xmp_blake3,xmp_claim,aae_rel,aae_blake3,aae_claim FROM fast_img_retry_sources WHERE task_key=?1 ORDER BY source_rel")?
        .query_map([key],|row| Ok(FailedRetryAdmission { source_rel:row.get(0)?,parent_source_blake3:row.get(1)?,
            admitted_source_blake3:row.get(2)?,claim:row.get(3)?,xmp:sidecar_from_row(row,4)?,aae:sidecar_from_row(row,7)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut result = BTreeMap::new();
    for value in values {
        ensure!(
            result.insert(value.source_rel.clone(), value).is_none(),
            "Duplicate retry admission"
        );
    }
    Ok(result)
}

fn validate_admissions(
    parent: &WorkingCopyMarker,
    values: &BTreeMap<String, FailedRetryAdmission>,
) -> Result<()> {
    ensure!(
        values.len() == parent.failed_sources.len() && !values.is_empty(),
        "Failed retry admission must cover exactly the parent's failed sources"
    );
    for (rel, value) in values {
        ensure!(rel == &value.source_rel, "Retry admission path key changed");
        let path = parent.src_dir.join(relative(rel)?);
        hash_text(&value.parent_source_blake3)?;
        hash_text(&value.admitted_source_blake3)?;
        ensure!(
            parent
                .failed_sources
                .get(rel)
                .is_some_and(|entry| entry.src == value.parent_source_blake3),
            "Retry admission contains a successful, skipped, unknown, or changed parent identity"
        );
        PlannedSource::validate_identity(&value.claim, &path, &value.admitted_source_blake3)?;
        for (kind, sidecar) in [("xmp", &value.xmp), ("aae", &value.aae)] {
            if let FailedRetrySidecar::Present {
                relative_path,
                blake3,
                claim,
            } = sidecar
            {
                hash_text(blake3)?;
                let associated = parent.src_dir.join(relative(relative_path)?);
                ensure!(
                    associated != path && associated.parent() == path.parent(),
                    "Retry sidecar escapes its source directory"
                );
                ensure!(
                    associated
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .is_some_and(|extension| extension.eq_ignore_ascii_case(kind)),
                    "Retry sidecar kind disagrees with its recorded file"
                );
                PlannedSource::validate_identity(claim, &associated, blake3)?;
            }
        }
        if let (
            FailedRetrySidecar::Present {
                relative_path: xmp, ..
            },
            FailedRetrySidecar::Present {
                relative_path: aae, ..
            },
        ) = (&value.xmp, &value.aae)
        {
            ensure!(
                xmp != aae,
                "Retry associates one file with two sidecar roles"
            );
        }
    }
    Ok(())
}

fn manifest_hash(
    key: &str,
    row: &Relation,
    child: &WorkingCopyMarker,
    values: &BTreeMap<String, FailedRetryAdmission>,
) -> Result<blake3::Hash> {
    let intent = (
        &child.src_dir,
        &child.working_copy,
        &child.started_at,
        &child.strategy,
        child.metadata_policy_version,
        &child.processing_contract,
        &child.tool_snapshot_manifest,
        &child.tool_snapshot_identity,
    );
    Ok(blake3::hash(&canonical_bytes(&(
        1u8,
        key,
        &row.parent,
        row.revision,
        &row.snapshot,
        &row.started,
        &row.logical,
        &row.request,
        row.count,
        &row.delivery,
        intent,
        values,
    ))?))
}

fn validate_child_results(
    child: &WorkingCopyMarker,
    values: &BTreeMap<String, FailedRetryAdmission>,
) -> Result<()> {
    ensure!(
        child.src_jpeg_count == values.len(),
        "Retry child denominator differs from its immutable work set"
    );
    for (rel, outcome) in source_outcomes(child) {
        let admitted = values
            .get(rel)
            .context("Retry outcome escapes its admitted failure set")?;
        ensure!(
            outcome.source == admitted.admitted_source_blake3,
            "Retry outcome used a changed source identity"
        );
    }
    if let Some(history) = child.photos_submission_history() {
        for entry in &history.intents {
            let admitted = values
                .get(&entry.source_rel)
                .context("Retry submission escapes its admitted failure set")?;
            ensure!(
                entry.source_blake3 == admitted.admitted_source_blake3,
                "Retry submission source changed"
            );
        }
    }
    for receipt in &child.photos_imported_assets {
        ensure!(
            child
                .blake3_log
                .iter()
                .any(|(rel, entry)| values.contains_key(rel)
                    && entry.out_rel.as_deref() == Some(receipt.rel_path.as_str())),
            "Retry receipt has no admitted converted source"
        );
    }
    for receipt in &child.tier2_imported_assets {
        ensure!(
            values.contains_key(&receipt.rel_path),
            "Retry original receipt escapes its admitted source set"
        );
    }
    Ok(())
}

fn validate_child_delivery(
    child: &WorkingCopyMarker,
    delivery: &FailedRetryDeliveryIntent,
) -> Result<()> {
    validate_delivery(delivery)?;
    match delivery {
        FailedRetryDeliveryIntent::Local => ensure!(
            child.photos_library_path.is_none()
                && child.photos_import_naming_policy.is_none()
                && child.photos_imported_assets.is_empty()
                && child.tier2_imported_assets.is_empty()
                && child
                    .photos_submission_history()
                    .is_none_or(|history| history.intents.is_empty()),
            "A local-output retry cannot gain Photos bindings or delivery evidence"
        ),
        FailedRetryDeliveryIntent::Photos {
            library_path,
            naming_policy,
        } => {
            ensure!(
                child.photos_import_naming_policy.as_ref() == Some(naming_policy),
                "Retry Photos naming intent changed"
            );
            if let Some(path) = library_path {
                ensure!(
                    child.photos_library_path.as_ref() == Some(path),
                    "Retry explicit Photos library changed"
                );
            }
            if !child.photos_imported_assets.is_empty()
                || !child.tier2_imported_assets.is_empty()
                || child
                    .photos_submission_history()
                    .is_some_and(|history| !history.intents.is_empty())
            {
                ensure!(
                    child.photos_library_path.is_some(),
                    "Retry Photos submissions have no bound library identity"
                );
            }
        }
    }
    Ok(())
}

fn context_for(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<Option<FailedRetryContext>> {
    let mut seen = BTreeSet::from([key.to_owned()]);
    let mut current_key = key.to_owned();
    let mut current = marker.clone();
    let mut ancestors = Vec::new();
    let mut first_admissions = BTreeMap::new();
    let mut parent_key = String::new();
    let mut delivery = None;
    let mut logical_key: Option<String> = None;
    loop {
        let row = relation(conn, &current_key)?;
        let Some(row) = row else {
            ensure!(
                current.failed_retry_manifest_blake3.is_none(),
                "Retry manifest relationship is missing; task is not ordinary or absent"
            );
            if has_schema(conn)? {
                let orphan: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM fast_img_retry_sources WHERE task_key=?1)",
                    [&current_key],
                    |r| r.get(0),
                )?;
                ensure!(!orphan, "Retry source rows have no manifest authority");
            }
            if let Some(logical) = logical_key {
                ensure!(
                    logical == current_key,
                    "Retry ancestry has a different logical root"
                );
            }
            break;
        };
        ensure!(
            row.snapshot.len() == 32
                && row.manifest.len() == 32
                && row.revision >= 0
                && row.count > 0,
            "Invalid retry relation evidence"
        );
        hash_text(&row.parent)?;
        hash_text(&row.logical)?;
        hash_text(&row.request)?;
        ensure!(
            seen.insert(row.parent.clone()),
            "Retry ancestry contains a cycle"
        );
        let parent = read_snapshot_base(conn, &row.parent)?
            .context("Retry parent disappeared; child is not an independent ordinary task")?;
        validate_parent(&parent)?;
        let (revision, snapshot): (i64, Vec<u8>) = conn.query_row(
            "SELECT revision,snapshot_blake3 FROM fast_img_tasks WHERE task_key=?1",
            [&row.parent],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ensure!(
            revision == row.revision
                && snapshot == row.snapshot
                && parent.started_at == row.started,
            "Retry parent snapshot changed"
        );
        let hash = blake3::Hash::from_bytes(
            row.snapshot
                .clone()
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid parent hash"))?,
        )
        .to_hex()
        .to_string();
        ensure!(
            row.request == request_key(&row.parent, &hash)?
                && current.working_copy == child_path(&parent, &row.request)?
                && current.src_dir == parent.src_dir
                && current.strategy == parent.strategy
                && current.processing_contract == parent.processing_contract
                && current.metadata_policy_version == metadata_policy_version(&current)?,
            "Retry task identity, output or processing intent differs from its recorded admission"
        );
        let values = admissions(conn, &current_key)?;
        ensure!(
            values.len() == row.count,
            "Retry admission rows are incomplete"
        );
        validate_admissions(&parent, &values)?;
        let digest = manifest_hash(&current_key, &row, &current, &values)?;
        ensure!(
            digest.as_bytes().as_slice() == row.manifest
                && current.failed_retry_manifest_blake3.as_deref()
                    == Some(digest.to_hex().as_str()),
            "Retry manifest failed complete identity/hash validation"
        );
        validate_child_results(&current, &values)?;
        validate_child_delivery(&current, &row.delivery)?;
        if ancestors.is_empty() {
            first_admissions = values;
            parent_key = row.parent.clone();
            delivery = Some(row.delivery.clone());
        }
        if let Some(logical) = &logical_key {
            ensure!(logical == &row.logical, "Retry logical ancestry changed");
        }
        logical_key = Some(row.logical);
        ancestors.push(parent.clone());
        current_key = row.parent;
        current = parent;
    }
    if ancestors.is_empty() {
        return Ok(None);
    }
    Ok(Some(FailedRetryContext {
        ancestors,
        admissions: first_admissions,
        parent_key,
        delivery: delivery.context("Retry lineage has no requested delivery intent")?,
    }))
}

pub(super) fn validate_task(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<()> {
    context_for(conn, key, marker)?;
    Ok(())
}

pub(super) fn reject_missing_task_evidence(conn: &Connection, key: &str) -> Result<()> {
    if has_schema(conn)? {
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM fast_img_retry_tasks WHERE task_key=?1 OR parent_task_key=?1 OR logical_task_key=?1)
            OR EXISTS(SELECT 1 FROM fast_img_retry_sources WHERE task_key=?1)",[key],|r|r.get(0))?;
        ensure!(
            !exists,
            "Missing Fast IMG task still has protected retry references; state is not absent"
        );
    }
    Ok(())
}

pub(super) fn require_parent_unmodified(conn: &Connection, key: &str) -> Result<()> {
    if has_schema(conn)? {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM fast_img_retry_tasks WHERE parent_task_key=?1)",
            [key],
            |r| r.get(0),
        )?;
        ensure!(
            !exists,
            "Completed parent snapshot is immutable after a failed-retry child is created"
        );
    }
    Ok(())
}

pub(super) fn validate_update(old: &WorkingCopyMarker, next: &WorkingCopyMarker) -> Result<()> {
    if old.failed_retry_manifest_blake3.is_some() {
        ensure!(
            old.photos_library_path
                .as_ref()
                .is_none_or(|value| next.photos_library_path.as_ref() == Some(value))
                && old
                    .photos_import_naming_policy
                    .as_ref()
                    .is_none_or(|value| next.photos_import_naming_policy.as_ref() == Some(value)),
            "Retry's established Photos target cannot be changed or cleared"
        );
    }
    Ok(())
}

pub(super) fn row_count(conn: &Connection) -> Result<usize> {
    if !has_schema(conn)? {
        return Ok(0);
    }
    let rows:i64=conn.query_row("SELECT (SELECT count(*) FROM fast_img_retry_tasks)+(SELECT count(*) FROM fast_img_retry_sources)",[],|r|r.get(0))?;
    Ok(usize::try_from(rows)?)
}

/// Find the existing child before requiring a new filesystem admission scan.
///
/// # Errors
/// Rejects changed parent references, unavailable storage, and invalid child evidence.
pub fn find_failed_retry_child(
    parent: &FailedRetryParent,
    delivery: &FailedRetryDeliveryIntent,
) -> io::Result<Option<WorkingCopyMarker>> {
    (|| -> Result<_> {
        let conn = open_readonly()?.context("Retry parent store disappeared")?;
        in_snapshot(&conn, |conn| {
            recheck_parent(conn, parent)?;
            validate_delivery(delivery)?;
            if !has_schema(conn)? {
                return Ok(None);
            }
            let key: Option<String> = conn
                .query_row(
                    "SELECT task_key FROM fast_img_retry_tasks WHERE request_key=?1",
                    [&parent.request],
                    |r| r.get(0),
                )
                .optional()?;
            key.map(|key| {
                let marker=read_snapshot(conn,&key)?.context("Existing retry child disappeared")?;
                let context=context_for(conn,&key,&marker)?.context("Existing retry has no lineage")?;
                ensure!(context.requested_delivery()==delivery,"Existing failed retry has a different requested delivery target; no task was reused or reset");
                Ok(marker)
            })
                .transpose()
        })
    })()
    .map_err(as_io)
}

/// Read the immutable lineage and admissions. This performs no media filesystem access.
///
/// # Errors
/// Rejects malformed locators, broken ancestry, incomplete admissions, or hash drift.
pub fn failed_retry_context(child_wc: &Path) -> io::Result<Option<FailedRetryContext>> {
    (|| -> Result<_> {
        validate_locator(text(child_wc)?)?;
        let Some(conn) = open_readonly()? else {
            return Ok(None);
        };
        in_snapshot(&conn, |conn| {
            let key = key_for_canonical(text(child_wc)?);
            let Some(marker) = read_snapshot_base(conn, &key)? else {
                return Ok(None);
            };
            context_for(conn, &key, &marker)
        })
    })()
    .map_err(as_io)
}

fn sidecar_columns(sidecar: &FailedRetrySidecar) -> (Option<&str>, Option<&str>, Option<&str>) {
    match sidecar {
        FailedRetrySidecar::Absent => (None, None, None),
        FailedRetrySidecar::Present {
            relative_path,
            blake3,
            claim,
        } => (Some(relative_path), Some(blake3), Some(claim)),
    }
}

/// Atomically create one independent child, or return the exact existing request.
/// The supplied claims are structurally checked here; execution must reverify them against files.
///
/// # Errors
/// Rejects changed parent references, conflicting admissions/output ownership, and storage failures.
pub fn create_or_resume_failed_retry(
    parent: &FailedRetryParent,
    supplied: &[FailedRetryAdmission],
    delivery: &FailedRetryDeliveryIntent,
    tools: Option<&FailedRetryToolAuthority>,
) -> io::Result<WorkingCopyMarker> {
    (||->Result<_>{
        let mut conn=open_store_connection()?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        recheck_parent(&tx,parent)?;
        validate_delivery(delivery)?;
        if let Some(tools)=tools { validate_locator(text(&tools.manifest_path)?)?; hash_text(&tools.identity)?; }
        let mut values=BTreeMap::new();
        for value in supplied {ensure!(values.insert(value.source_rel.clone(),value.clone()).is_none(),"Duplicate failed retry source");}
        validate_admissions(&parent.marker,&values)?;
        if let Some(key)=tx.query_row("SELECT task_key FROM fast_img_retry_tasks WHERE request_key=?1",[&parent.request],|r|r.get::<_,String>(0)).optional()? {
            let marker=read_snapshot(&tx,&key)?.context("Existing failed child is missing")?;
            let context=context_for(&tx,&key,&marker)?.context("Existing child lost its retry manifest")?;
            ensure!(context.admissions==values,"Existing retry request has different admissions; no second child was created");
            ensure!(context.requested_delivery()==delivery,"Existing failed retry has a different requested delivery target; no task was reused or reset");
            ensure!(marker.tool_snapshot_manifest.as_ref()==tools.map(|tools|&tools.manifest_path)
                && marker.tool_snapshot_identity.as_ref()==tools.map(|tools|&tools.identity),"Existing retry's fixed tool authority differs from this request");
            tx.commit()?;
            return Ok(marker);
        }
        let key=key_for_canonical(text(&parent.child)?);
        ensure!(read_snapshot(&tx,&key)?.is_none(),"Stable retry output is already reserved by another task");
        let mut child=WorkingCopyMarker::new(parent.marker.src_dir.clone(),parent.child.clone(),values.len()).with_strategy(parent.marker.strategy.clone());
        child.processing_contract=parent.marker.processing_contract.clone();
        child.metadata_policy_version=metadata_policy_version(&child)?;
        if let FailedRetryDeliveryIntent::Photos{library_path,naming_policy}=delivery {
            child.photos_library_path=library_path.clone();
            child.photos_import_naming_policy=Some(naming_policy.clone());
        }
        child.tool_snapshot_manifest=tools.map(|tools|tools.manifest_path.clone());
        child.tool_snapshot_identity=tools.map(|tools|tools.identity.clone());
        let parent_context=context_for(&tx,&parent.key,&parent.marker)?;
        let logical=parent_context.as_ref().and_then(|context|context.ancestors.last()).map_or_else(||Ok(parent.key.clone()),|root|Ok::<_,anyhow::Error>(key_for_canonical(text(&root.working_copy)?)))?;
        let mut row=Relation{parent:parent.key.clone(),revision:parent.revision,snapshot:blake3::Hash::from_hex(&parent.hash)?.as_bytes().to_vec(),
            started:parent.marker.started_at.clone(),logical,request:parent.request.clone(),manifest:Vec::new(),delivery:delivery.clone(),count:values.len()};
        let digest=manifest_hash(&key,&row,&child,&values)?;
        row.manifest=digest.as_bytes().to_vec();
        child.failed_retry_manifest_blake3=Some(digest.to_hex().to_string());
        write_snapshot_at(&tx,&child,text(&child.working_copy)?,text(&child.src_dir)?,false)?;
        tx.execute("INSERT INTO fast_img_retry_tasks(task_key,parent_task_key,parent_revision,parent_snapshot_blake3,parent_started_at,
            logical_task_key,request_key,manifest_blake3,source_count,requested_delivery) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![key,row.parent,row.revision,row.snapshot,row.started,row.logical,row.request,row.manifest,i64::try_from(row.count)?,String::from_utf8(canonical_bytes(delivery)?)?])?;
        for value in values.values(){
            let (xr,xh,xc)=sidecar_columns(&value.xmp); let (ar,ah,ac)=sidecar_columns(&value.aae);
            tx.execute("INSERT INTO fast_img_retry_sources(task_key,source_rel,parent_source_blake3,admitted_source_blake3,source_claim,
                xmp_rel,xmp_blake3,xmp_claim,aae_rel,aae_blake3,aae_claim) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![key,value.source_rel,value.parent_source_blake3,value.admitted_source_blake3,value.claim,xr,xh,xc,ar,ah,ac])?;
        }
        ensure!(read_snapshot(&tx,&key)?.as_ref()==Some(&child),"Failed retry creation readback differs; transaction must roll back");
        recheck_parent(&tx,parent)?;
        tx.commit().context("Failed retry creation did not commit; parent retained")?;
        Ok(child)
    })().map_err(as_io)
}

#[cfg(test)]
mod tests;

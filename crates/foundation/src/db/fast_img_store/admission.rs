//! Immutable admission evidence for new, full-discovery Fast IMG tasks.

use super::*;
use crate::mfb_sqlite_store::{
    FAST_IMG_ADMISSION_AAE_SCHEMA_VERSION, FAST_IMG_ADMISSION_SCHEMA_VERSION,
    FAST_IMG_ADMISSION_XMP_SCHEMA_VERSION,
};
use crate::pipeline::verification::{FastImgStageName, validate_marker_relative_path};

/// The route selected by the worker's media inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FastImgAdmissionRoute {
    Primary,
    /// The file was scanned, but its media probe failed and it must be retained.
    PrimaryRejected,
    Original,
}

impl FastImgAdmissionRoute {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::PrimaryRejected => "primary_rejected",
            Self::Original => "original",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "primary" => Ok(Self::Primary),
            "primary_rejected" => Ok(Self::PrimaryRejected),
            "original" => Ok(Self::Original),
            _ => bail!("Invalid Fast IMG admission route"),
        }
    }
}

/// Observed adjacent sidecar candidate, separate from content validity or ownership.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FastImgAdmissionSidecar {
    Absent,
    Present {
        relative_path: String,
        blake3: String,
    },
    Unresolved,
}

impl FastImgAdmissionSidecar {
    fn from_storage(
        state: Option<&str>,
        relative_path: Option<String>,
        blake3: Option<String>,
    ) -> Result<Option<Self>> {
        Ok(match (state, relative_path, blake3) {
            (None, None, None) => None,
            (Some("absent"), None, None) => Some(Self::Absent),
            (Some("present"), Some(relative_path), Some(blake3)) => Some(Self::Present {
                relative_path,
                blake3,
            }),
            (Some("unresolved"), None, None) => Some(Self::Unresolved),
            _ => bail!("Invalid Fast IMG admission sidecar evidence"),
        })
    }

    fn storage_fields(&self) -> (&'static str, Option<&str>, Option<&str>) {
        match self {
            Self::Absent => ("absent", None, None),
            Self::Present {
                relative_path,
                blake3,
            } => ("present", Some(relative_path), Some(blake3)),
            Self::Unresolved => ("unresolved", None, None),
        }
    }

    pub(crate) fn matches_observation(&self, observed: Option<(&str, &str)>) -> bool {
        match (self, observed) {
            (Self::Absent, None) => true,
            (
                Self::Present {
                    relative_path,
                    blake3,
                },
                Some((path, hash)),
            ) => relative_path == path && blake3 == hash,
            _ => false,
        }
    }
}

/// Identity of a source selected for primary encoding or original delivery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FastImgSourceAdmission {
    pub source_blake3: String,
    pub route: FastImgAdmissionRoute,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xmp: Option<FastImgAdmissionSidecar>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aae: Option<FastImgAdmissionSidecar>,
}

/// Admissions are keyed by exact, normalized relative path.
pub type FastImgAdmissionPlan = BTreeMap<String, FastImgSourceAdmission>;

fn validate_plan(marker: &WorkingCopyMarker, plan: &FastImgAdmissionPlan) -> Result<String> {
    let mut primary_count = 0;
    let mut xmp_count = 0;
    let mut aae_count = 0;
    ensure!(
        !plan.is_empty(),
        "Fast IMG admission plan must not be empty"
    );
    for (rel, entry) in plan {
        validate_marker_relative_path("Fast IMG admission source", rel)
            .map_err(anyhow::Error::msg)?;
        ensure!(
            cleanup::hash_is_valid(&entry.source_blake3),
            "Invalid Fast IMG admission source BLAKE3: {rel}"
        );
        for (sidecar, extension, count) in [
            (&entry.xmp, "xmp", &mut xmp_count),
            (&entry.aae, "aae", &mut aae_count),
        ] {
            if sidecar.is_some() {
                *count += 1;
            }
            if let Some(FastImgAdmissionSidecar::Present {
                relative_path,
                blake3,
            }) = sidecar
            {
                validate_marker_relative_path("Fast IMG admission sidecar", relative_path)
                    .map_err(anyhow::Error::msg)?;
                let path = Path::new(relative_path);
                ensure!(
                    relative_path != rel
                        && path.parent() == Path::new(rel).parent()
                        && path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
                        && cleanup::hash_is_valid(blake3),
                    "Fast IMG admission {extension} identity is invalid: {rel}"
                );
            }
        }
        if entry.route != FastImgAdmissionRoute::Original {
            primary_count += 1;
        }
    }
    ensure!(
        primary_count == marker.primary_source_count(),
        "Fast IMG admission primary count differs from its task"
    );
    ensure!(
        xmp_count == 0 || xmp_count == plan.len(),
        "Fast IMG admission XMP coverage is incomplete"
    );
    ensure!(
        aae_count == 0 || (aae_count == plan.len() && xmp_count == plan.len()),
        "Fast IMG admission AAE coverage is incomplete"
    );
    let context = if aae_count != 0 {
        "Modern Format Boost Fast IMG admission plan v3"
    } else if xmp_count != 0 {
        "Modern Format Boost Fast IMG admission plan v2"
    } else {
        "Modern Format Boost Fast IMG admission plan v1"
    };
    let mut digest = blake3::Hasher::new_derive_key(context);
    digest.update(&canonical_bytes(plan)?);
    Ok(digest.finalize().to_hex().to_string())
}

fn contains_rows(conn: &Connection, key: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM fast_img_admission WHERE task_key=?1)",
        [key],
        |row| row.get(0),
    )?)
}

pub(super) fn reject_orphans(conn: &Connection, key: &str) -> Result<()> {
    let version = inspect_store_schema_version(conn)?;
    ensure!(
        !contains_rows(conn, key)?,
        "Missing Fast IMG task still has protected admission evidence"
    );
    for (table, since) in [
        (
            "fast_img_admission_xmp",
            FAST_IMG_ADMISSION_XMP_SCHEMA_VERSION,
        ),
        (
            "fast_img_admission_aae",
            FAST_IMG_ADMISSION_AAE_SCHEMA_VERSION,
        ),
    ] {
        if version.is_some_and(|version| version >= since) {
            ensure!(
                !conn.query_row(
                    &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE task_key=?1)"),
                    [key],
                    |row| row.get::<_, bool>(0),
                )?,
                "Missing Fast IMG task still has protected admission evidence"
            );
        }
    }
    Ok(())
}

pub(super) fn validate_row_presence(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
    schema_version: Option<i32>,
) -> Result<()> {
    let present = schema_version
        .is_some_and(|version| version >= FAST_IMG_ADMISSION_SCHEMA_VERSION)
        && contains_rows(conn, key)?;
    ensure!(
        present == marker.admission_plan_blake3.is_some(),
        "Fast IMG task header and durable admission presence disagree"
    );
    Ok(())
}

pub(super) fn read_validated(
    conn: &Connection,
    key: &str,
    marker: &WorkingCopyMarker,
) -> Result<Option<FastImgAdmissionPlan>> {
    let Some(digest) = &marker.admission_plan_blake3 else {
        return Ok(None);
    };
    let version = inspect_store_schema_version(conn)?;
    let mut statement = conn.prepare(match version {
        Some(version) if version >= FAST_IMG_ADMISSION_AAE_SCHEMA_VERSION =>
            "SELECT a.source_rel,a.source_blake3,a.route,x.state,x.xmp_rel,x.xmp_blake3,
                    e.state,e.aae_rel,e.aae_blake3
             FROM fast_img_admission a LEFT JOIN fast_img_admission_xmp x USING(task_key,source_rel)
             LEFT JOIN fast_img_admission_aae e USING(task_key,source_rel)
             WHERE a.task_key=?1 ORDER BY a.source_rel",
        Some(version) if version >= FAST_IMG_ADMISSION_XMP_SCHEMA_VERSION =>
            "SELECT a.source_rel,a.source_blake3,a.route,x.state,x.xmp_rel,x.xmp_blake3,NULL,NULL,NULL
             FROM fast_img_admission a LEFT JOIN fast_img_admission_xmp x USING(task_key,source_rel)
             WHERE a.task_key=?1 ORDER BY a.source_rel",
        _ => "SELECT source_rel,source_blake3,route,NULL,NULL,NULL,NULL,NULL,NULL
              FROM fast_img_admission WHERE task_key=?1 ORDER BY source_rel",
    })?;
    let mut rows = statement.query([key])?;
    let mut plan = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let rel: String = row.get(0)?;
        let source_blake3 = row.get(1)?;
        let route = FastImgAdmissionRoute::from_str(row.get_ref(2)?.as_str()?)?;
        let xmp = FastImgAdmissionSidecar::from_storage(
            row.get::<_, Option<String>>(3)?.as_deref(),
            row.get(4)?,
            row.get(5)?,
        )?;
        let aae = FastImgAdmissionSidecar::from_storage(
            row.get::<_, Option<String>>(6)?.as_deref(),
            row.get(7)?,
            row.get(8)?,
        )?;
        ensure!(
            plan.insert(
                rel,
                FastImgSourceAdmission {
                    source_blake3,
                    route,
                    xmp,
                    aae
                }
            )
            .is_none(),
            "Duplicate durable Fast IMG admission"
        );
    }
    ensure!(
        validate_plan(marker, &plan)? == *digest,
        "Fast IMG admission plan failed BLAKE3 verification; admitted media retained"
    );
    Ok(Some(plan))
}

pub(super) fn insert_rows(conn: &Connection, key: &str, plan: &FastImgAdmissionPlan) -> Result<()> {
    let mut insert = conn.prepare(
        "INSERT INTO fast_img_admission(task_key,source_rel,source_blake3,route)
         VALUES(?1,?2,?3,?4)",
    )?;
    for (rel, entry) in plan {
        insert.execute(params![key, rel, entry.source_blake3, entry.route.as_str()])?;
    }
    let mut insert_xmp = conn.prepare(
        "INSERT INTO fast_img_admission_xmp(task_key,source_rel,state,xmp_rel,xmp_blake3)
         VALUES(?1,?2,?3,?4,?5)",
    )?;
    let mut insert_aae = conn.prepare(
        "INSERT INTO fast_img_admission_aae(task_key,source_rel,state,aae_rel,aae_blake3)
         VALUES(?1,?2,?3,?4,?5)",
    )?;
    for (rel, entry) in plan {
        for (sidecar, statement) in [(&entry.xmp, &mut insert_xmp), (&entry.aae, &mut insert_aae)] {
            if let Some(sidecar) = sidecar {
                let (state, path, hash) = sidecar.storage_fields();
                statement.execute(params![key, rel, state, path, hash])?;
            }
        }
    }
    Ok(())
}

/// Atomically reserve a new task with its complete admission before media work.
///
/// # Errors
/// Rejects an existing checkpoint, invalid admission, or failed storage/readback.
pub fn initialize_with_admission(
    marker: &mut WorkingCopyMarker,
    plan: &FastImgAdmissionPlan,
) -> io::Result<()> {
    (|| -> Result<()> {
        ensure!(
            marker.stage == FastImgStageName::ScanComplete
                && marker.admission_plan_blake3.is_none()
                && marker.failed_retry_manifest_blake3.is_none()
                && marker.blake3_log.is_empty()
                && marker.skipped_sources.is_empty()
                && marker.failed_sources.is_empty()
                && marker.tier2_failed_sources.is_empty()
                && marker.photos_imported_assets.is_empty()
                && marker.tier2_imported_assets.is_empty()
                && marker
                    .photos_submission_history()
                    .is_some_and(|history| history.intents.is_empty()),
            "Fast IMG admission can only initialize a new full-discovery task"
        );
        migration::reject_unmigrated_for_working_copy(&marker.working_copy)?;
        let wc = canonical_text(&marker.working_copy)?;
        let source = canonical_text(&marker.src_dir)?;
        let digest = validate_plan(marker, plan)?;
        let mut next = marker.clone();
        next.working_copy = PathBuf::from(&wc);
        next.src_dir = PathBuf::from(&source);
        next.admission_plan_blake3 = Some(digest);
        let mut conn = open_store_connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_snapshot_at_with_finish(&tx, &next, &wc, &source, true, None, Some(plan))?;
        tx.commit()
            .context("Fast IMG admission commit failed; media work was not started")?;
        *marker = next;
        Ok(())
    })()
    .map_err(as_io)
}

impl WorkingCopyMarker {
    /// Whether this task is bound to a durable full-discovery admission plan.
    #[must_use]
    pub const fn has_admission_plan(&self) -> bool {
        self.admission_plan_blake3.is_some()
    }
}

/// Load and verify all immutable admissions in one read-only SQL snapshot.
///
/// # Errors
/// Rejects changed checkpoints, incomplete/corrupt admissions and unavailable state.
pub fn read_admission_plan(marker: &WorkingCopyMarker) -> io::Result<Option<FastImgAdmissionPlan>> {
    (|| -> Result<Option<FastImgAdmissionPlan>> {
        if marker.admission_plan_blake3.is_none() {
            return Ok(None);
        }
        let conn = open_readonly()?.context("Fast IMG admission store is unavailable")?;
        let wc = canonical_text(&marker.working_copy)?;
        let key = key_for_canonical(&wc);
        in_snapshot(&conn, |conn| {
            let saved = checked_marker(conn, &wc)?.context("Fast IMG admission task is missing")?;
            ensure!(
                saved == *marker,
                "Fast IMG task changed before reading its admissions"
            );
            read_validated(conn, &key, marker)
        })
    })()
    .map_err(as_io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn schema13_migrates_v12_without_inventing_or_rewriting_xmp() -> Result<()> {
        let root = tempfile::tempdir()?;
        let root = root.path().canonicalize()?;
        let db = root.join("state.sqlite");
        let _store = crate::mfb_sqlite_store::set_test_store_path_for_tests(db.clone());
        let src = root.join("source");
        fs::create_dir(&src)?;
        let mut marker = WorkingCopyMarker::new(src, root.join("source_optimized"), 1);
        let plan = BTreeMap::from([(
            "one.jpg".into(),
            FastImgSourceAdmission {
                source_blake3: blake3::hash(b"old synthetic JPEG").to_hex().to_string(),
                route: FastImgAdmissionRoute::Primary,
                xmp: None,
                aae: None,
            },
        )]);
        initialize_with_admission(&mut marker, &plan)?;
        let saved_digest = marker.admission_plan_blake3.clone();
        let conn = open_store_connection()?;
        conn.execute_batch(
            "DROP TABLE fast_img_primary_results;
             DROP TABLE fast_img_admission_aae;
             DROP TABLE fast_img_admission_xmp;
             UPDATE store_metadata SET value=12 WHERE key='schema_version';
             CREATE TRIGGER reject_xmp_upgrade BEFORE UPDATE OF value ON store_metadata
             WHEN NEW.value=13 BEGIN SELECT RAISE(ABORT,'synthetic XMP migration failure'); END",
        )?;
        drop(conn);
        assert_eq!(read_admission_plan(&marker)?, Some(plan.clone()));
        let err = open_store_connection().unwrap_err();
        assert!(format!("{err:#}").contains("synthetic XMP migration failure"));
        let conn = Connection::open(&db)?;
        assert_eq!(inspect_store_schema_version(&conn)?, Some(12));
        assert!(
            conn.prepare("SELECT * FROM fast_img_admission_xmp")
                .is_err()
        );
        conn.execute_batch("DROP TRIGGER reject_xmp_upgrade")?;
        drop(conn);

        let conn = open_store_connection()?;
        assert_eq!(
            inspect_store_schema_version(&conn)?,
            Some(crate::mfb_sqlite_store::STORE_SCHEMA_VERSION),
        );
        assert_eq!(inspect_integrity(&conn)?, 2);
        assert_eq!(read_admission_plan(&marker)?, Some(plan));
        assert_eq!(
            read_marker(&marker.working_copy)?.admission_plan_blake3,
            saved_digest
        );
        Ok(())
    }

    #[test]
    fn schema14_migrates_v13_without_inventing_or_rewriting_aae() -> Result<()> {
        let root = tempfile::tempdir()?;
        let root = root.path().canonicalize()?;
        let db = root.join("state.sqlite");
        let _store = crate::mfb_sqlite_store::set_test_store_path_for_tests(db.clone());
        let src = root.join("source");
        fs::create_dir(&src)?;
        let mut marker = WorkingCopyMarker::new(src, root.join("source_optimized"), 1);
        let plan = BTreeMap::from([(
            "one.jpg".into(),
            FastImgSourceAdmission {
                source_blake3: blake3::hash(b"old synthetic JPEG").to_hex().to_string(),
                route: FastImgAdmissionRoute::Primary,
                xmp: Some(FastImgAdmissionSidecar::Present {
                    relative_path: "one.XMP".into(),
                    blake3: blake3::hash(b"old synthetic XMP").to_hex().to_string(),
                }),
                aae: None,
            },
        )]);
        initialize_with_admission(&mut marker, &plan)?;
        let digest = marker.admission_plan_blake3.clone();
        let conn = open_store_connection()?;
        conn.execute_batch(
            "DROP TABLE fast_img_primary_results;
             DROP TABLE fast_img_admission_aae;
             UPDATE store_metadata SET value=13 WHERE key='schema_version';
             CREATE TRIGGER reject_aae_upgrade BEFORE UPDATE OF value ON store_metadata
             WHEN NEW.value=14 BEGIN SELECT RAISE(ABORT,'synthetic AAE migration failure'); END",
        )?;
        drop(conn);
        assert_eq!(read_admission_plan(&marker)?, Some(plan.clone()));
        assert!(
            format!("{:#}", open_store_connection().unwrap_err())
                .contains("synthetic AAE migration failure")
        );
        let conn = Connection::open(&db)?;
        assert_eq!(inspect_store_schema_version(&conn)?, Some(13));
        assert!(
            conn.prepare("SELECT * FROM fast_img_admission_aae")
                .is_err()
        );
        conn.execute_batch("DROP TRIGGER reject_aae_upgrade")?;
        drop(conn);

        let conn = open_store_connection()?;
        assert_eq!(
            inspect_store_schema_version(&conn)?,
            Some(crate::mfb_sqlite_store::STORE_SCHEMA_VERSION),
        );
        assert_eq!(inspect_integrity(&conn)?, 3);
        assert_eq!(read_admission_plan(&marker)?, Some(plan));
        assert_eq!(
            read_marker(&marker.working_copy)?.admission_plan_blake3,
            digest
        );
        Ok(())
    }

    #[test]
    fn durable_admission_is_atomic_immutable_and_detects_corruption() -> Result<()> {
        let root = tempfile::tempdir()?;
        let root = root.path().canonicalize()?;
        let state = root.join("state.sqlite");
        let _store = crate::mfb_sqlite_store::set_test_store_path_for_tests(state);
        let src = root.join("source");
        fs::create_dir(&src)?;
        let mut marker = WorkingCopyMarker::new(src, root.join("source_optimized"), 1);
        let plan = BTreeMap::from([
            (
                "nested/one.jpg".into(),
                FastImgSourceAdmission {
                    source_blake3: blake3::hash(b"synthetic JPEG").to_hex().to_string(),
                    route: FastImgAdmissionRoute::Primary,
                    xmp: Some(FastImgAdmissionSidecar::Present {
                        relative_path: "nested/one.JPG.XMP".into(),
                        blake3: blake3::hash(b"synthetic XMP").to_hex().to_string(),
                    }),
                    aae: Some(FastImgAdmissionSidecar::Present {
                        relative_path: "nested/ONE.aAe".into(),
                        blake3: blake3::hash(b"synthetic AAE").to_hex().to_string(),
                    }),
                },
            ),
            (
                "other.webp".into(),
                FastImgSourceAdmission {
                    source_blake3: blake3::hash(b"synthetic WebP").to_hex().to_string(),
                    route: FastImgAdmissionRoute::Original,
                    xmp: Some(FastImgAdmissionSidecar::Absent),
                    aae: Some(FastImgAdmissionSidecar::Absent),
                },
            ),
        ]);
        let conn = open_store_connection()?;
        conn.execute_batch(
            "CREATE TRIGGER reject_original BEFORE INSERT ON fast_img_admission
             WHEN NEW.route='original' BEGIN SELECT RAISE(ABORT,'synthetic write failure'); END",
        )?;
        let original = marker.clone();
        let error = initialize_with_admission(&mut marker, &plan).unwrap_err();
        assert!(error.to_string().contains("synthetic write failure"));
        assert_eq!(marker, original);
        assert_eq!(
            read_marker(&marker.working_copy).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(row_count(&conn)?, 0);

        conn.execute_batch(
            "DROP TRIGGER reject_original;
             CREATE TRIGGER reject_xmp BEFORE INSERT ON fast_img_admission_xmp
             WHEN NEW.state='present' BEGIN SELECT RAISE(ABORT,'synthetic XMP write failure'); END",
        )?;
        assert!(
            initialize_with_admission(&mut marker, &plan)
                .unwrap_err()
                .to_string()
                .contains("synthetic XMP write failure")
        );
        assert_eq!(marker, original);
        assert_eq!(row_count(&conn)?, 0);
        conn.execute_batch(
            "DROP TRIGGER reject_xmp;
             CREATE TRIGGER reject_aae BEFORE INSERT ON fast_img_admission_aae
             WHEN NEW.state='present' BEGIN SELECT RAISE(ABORT,'synthetic AAE write failure'); END",
        )?;
        assert!(
            initialize_with_admission(&mut marker, &plan)
                .unwrap_err()
                .to_string()
                .contains("synthetic AAE write failure")
        );
        assert_eq!(marker, original);
        assert_eq!(row_count(&conn)?, 0);
        conn.execute_batch("DROP TRIGGER reject_aae")?;
        initialize_with_admission(&mut marker, &plan)?;
        assert_eq!(read_marker(&marker.working_copy)?, marker);
        assert_eq!(read_admission_plan(&marker)?, Some(plan.clone()));
        assert_eq!(inspect_integrity(&conn)?, 7);
        assert!(initialize_with_admission(&mut original.clone(), &plan).is_err());
        assert!(write_marker(&original).is_err());
        marker.stage = FastImgStageName::OutputPrepared;
        write_marker(&marker)?;
        assert_eq!(read_admission_plan(&marker)?, Some(plan));

        conn.execute(
            "UPDATE fast_img_admission_aae SET aae_blake3=?1 WHERE source_rel='nested/one.jpg'",
            [blake3::hash(b"tampered").to_hex().to_string()],
        )?;
        assert!(read_admission_plan(&marker).is_err());
        assert!(inspect_integrity(&conn).is_err());
        // Occupancy remains established without loading every admission row.
        assert_eq!(read_marker(&marker.working_copy)?, marker);
        Ok(())
    }
}

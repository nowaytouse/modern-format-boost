//! Narrow read-only original-resource custody lookup for the recognized Photos schema.
//! Album tables are deliberately outside this query. Unsupported resource layouts
//! retain the existing verifier; incomplete or contradictory evidence never does.

use super::FastImgLibraryAssetProbe;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::BTreeSet;
use std::path::{Component, Path};
use std::time::Duration;

pub(super) enum Lookup {
    Probes(Vec<FastImgLibraryAssetProbe>),
    UnsupportedLayout(&'static str),
}

const ASSET_SQL: &str = "SELECT Z_PK,ZUUID,ZDIRECTORY,ZFILENAME,ZCLOUDASSETGUID,ZMASTER,
    ZCLOUDBATCHPUBLISHDATE,ZSYNDICATIONSTATE,ZTRASHEDSTATE,ZMOMENTSHARE
    FROM ZASSET WHERE ZUUID=?1 LIMIT 2";
const RESOURCE_SQL: &str = "SELECT ZLOCALAVAILABILITY FROM ZINTERNALRESOURCE
    WHERE ZASSET=?1 AND ZDATASTORESUBTYPE=1 LIMIT 2";
const CLOUD_SQL: &str = "SELECT ZCLOUDLOCALSTATE FROM ZCLOUDMASTER WHERE Z_PK=?1";

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name=?1)",
        [name], |row| row.get(0),
    )?)
}

fn require_lookup_index(conn: &Connection, table: &str, column: &str) -> Result<()> {
    let indexed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_index_list(?1) AS idx
         JOIN pragma_index_info(idx.name) AS col
         WHERE idx.partial=0 AND col.seqno=0 AND col.name=?2)",
        params![table, column], |row| row.get(0),
    )?;
    ensure!(indexed, "Photos original-resource query requires an index on {table}.{column}");
    Ok(())
}

fn normal_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|part| matches!(part, Component::Normal(_)))
}

pub(super) fn query(uuids: &[String], library: &Path) -> Result<Lookup> {
    ensure!(uuids.iter().all(|uuid| !uuid.is_empty()), "Photos custody query contains an empty UUID");
    ensure!(uuids.iter().collect::<BTreeSet<_>>().len() == uuids.len(),
        "Photos custody query contains duplicate UUIDs");
    let selected = crate::common_utils::validate_photos_library_path(library)?;
    let database = selected.join("database/Photos.sqlite").canonicalize()?;
    ensure!(database.starts_with(&selected), "Photos database escaped the selected library");
    #[cfg(unix)]
    let before = std::fs::metadata(&database)?;
    let conn = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .context("open selected Photos database read-only")?;
    conn.busy_timeout(Duration::from_millis(250))?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF;")?;
    let tx = conn.unchecked_transaction()?;
    if !has_table(&tx, "ZASSET")? {
        ensure!(has_table(&tx, "ZGENERICASSET")? || has_table(&tx, "RKMaster")?,
            "unsupported database schema for Photos original-resource lookup");
        return Ok(Lookup::UnsupportedLayout("older Photos asset schema"));
    }
    for table in ["ZINTERNALRESOURCE", "ZCLOUDMASTER"] {
        ensure!(has_table(&tx, table)?, "Photos original-resource schema has no such table: {table}");
    }
    require_lookup_index(&tx, "ZASSET", "ZUUID")?;
    require_lookup_index(&tx, "ZINTERNALRESOURCE", "ZASSET")?;
    tx.prepare("SELECT Z_PK,ZUUID,ZDIRECTORY,ZFILENAME,ZCLOUDASSETGUID,ZMASTER,ZCLOUDBATCHPUBLISHDATE,ZTRASHEDSTATE FROM ZASSET LIMIT 0")
        .context("validate Photos core original identity fields")?;
    tx.prepare(RESOURCE_SQL).context("validate Photos original-resource fields")?;
    tx.prepare(CLOUD_SQL).context("validate Photos cloud state fields")?;
    let layout_fields: i64 = tx.query_row(
        "SELECT count(*) FROM pragma_table_info('ZASSET') WHERE name IN ('ZSYNDICATIONSTATE','ZMOMENTSHARE')",
        [], |row| row.get(0),
    )?;
    if layout_fields != 2 {
        return Ok(Lookup::UnsupportedLayout("older Photos original location discriminators"));
    }
    let originals = selected.join("originals").canonicalize()
        .context("resolve the selected library originals directory")?;
    ensure!(originals.starts_with(&selected) && originals.is_dir(),
        "Photos originals directory escaped the selected library");
    let mut assets = tx.prepare(ASSET_SQL).context("validate Photos asset fields")?;
    let mut resources = tx.prepare(RESOURCE_SQL).context("validate Photos original-resource fields")?;
    let mut cloud = tx.prepare(CLOUD_SQL).context("validate Photos cloud state fields")?;
    let mut probes = Vec::with_capacity(uuids.len());
    for uuid in uuids {
        let mut rows = assets.query([uuid])?;
        let row = rows.next()?.context("Photos custody UUID is not yet present in the selected library")?;
        let asset: i64 = row.get(0)?;
        let actual_uuid: String = row.get(1)?;
        let directory: String = row.get(2)?;
        let filename: String = row.get(3)?;
        let cloud_guid: Option<String> = row.get(4)?;
        let master: Option<i64> = row.get(5)?;
        let shared: Option<rusqlite::types::Value> = row.get(6)?;
        let syndicated: i64 = row.get(7)?;
        let trashed: i64 = row.get(8)?;
        let moment: Option<i64> = row.get(9)?;
        ensure!(asset > 0 && actual_uuid == *uuid, "Photos custody query returned an unexpected UUID or asset identity");
        ensure!(rows.next()?.is_none(), "Photos custody query returned duplicate UUID rows");
        ensure!(trashed == 0, "Photos custody asset is in the trash");
        if shared.is_some() || syndicated != 0 || moment.is_some_and(|value| value != 0) {
            return Ok(Lookup::UnsupportedLayout("shared or syndicated original-resource layout"));
        }
        let directory = Path::new(&directory);
        let filename = Path::new(&filename);
        if directory.is_absolute() {
            return Ok(Lookup::UnsupportedLayout("referenced original outside managed originals"));
        }
        ensure!(normal_relative(directory) && normal_relative(filename) && filename.components().count() == 1,
            "Photos original-resource path contains an unsafe component");
        let mut local_rows = resources.query([asset])?;
        let local: i64 = local_rows.next()?.context("Photos original-resource availability is absent")?.get(0)?;
        ensure!(local_rows.next()?.is_none(), "Photos original-resource availability is ambiguous");
        let iscloudasset = cloud_guid.is_some();
        let incloud = if let Some(guid) = cloud_guid {
            ensure!(!guid.is_empty(), "Photos cloud asset identity is empty");
            let master = master.context("Photos cloud asset has no master identity")?;
            let state: Option<i64> = cloud.query_row([master], |row| row.get(0)).optional()?.flatten();
            Some(state.context("Photos cloud synchronization state is absent")? == 3)
        } else {
            None
        };
        let candidate = originals.join(directory).join(filename);
        let (path, absent) = match std::fs::symlink_metadata(&candidate) {
            Ok(metadata) => {
                ensure!(metadata.file_type().is_file(), "Photos original resource is not a regular file");
                let path = candidate.canonicalize()?;
                ensure!(path.starts_with(&originals), "Photos original resource escaped managed originals");
                (path, false)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (candidate, true),
            Err(error) => return Err(error.into()),
        };
        probes.push(FastImgLibraryAssetProbe { uuid: actual_uuid, path, iscloudasset, incloud, ismissing: local != 1 || absent });
    }
    drop((assets, resources, cloud));
    tx.commit()?;
    ensure!(selected.join("database/Photos.sqlite").canonicalize()? == database,
        "Selected Photos database path changed during the custody query");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let after = std::fs::metadata(&database)?;
        ensure!((before.dev(), before.ino()) == (after.dev(), after.ino()),
            "Selected Photos database was replaced during the custody query");
    }
    Ok(Lookup::Probes(probes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<(tempfile::TempDir, std::path::PathBuf)> {
        let root = tempfile::tempdir()?;
        let library = root.path().join("Fixture.photoslibrary");
        std::fs::create_dir_all(library.join("database"))?;
        std::fs::create_dir_all(library.join("originals/A"))?;
        std::fs::write(library.join("originals/A/asset.jxl"), b"exact-original")?;
        let conn = Connection::open(library.join("database/Photos.sqlite"))?;
        conn.execute_batch("CREATE TABLE ZASSET(Z_PK INTEGER PRIMARY KEY,ZUUID TEXT,ZDIRECTORY TEXT,ZFILENAME TEXT,ZCLOUDASSETGUID TEXT,ZMASTER INTEGER,ZCLOUDBATCHPUBLISHDATE REAL,ZSYNDICATIONSTATE INTEGER,ZTRASHEDSTATE INTEGER,ZMOMENTSHARE INTEGER);
            CREATE INDEX asset_uuid ON ZASSET(ZUUID);
            CREATE TABLE ZINTERNALRESOURCE(ZASSET INTEGER,ZDATASTORESUBTYPE INTEGER,ZLOCALAVAILABILITY INTEGER);
            CREATE INDEX resource_asset ON ZINTERNALRESOURCE(ZASSET);
            CREATE TABLE ZCLOUDMASTER(Z_PK INTEGER PRIMARY KEY,ZCLOUDLOCALSTATE INTEGER);
            INSERT INTO ZASSET VALUES(1,'UUID-A','A','asset.jxl','cloud-id',1,NULL,0,0,NULL);
            INSERT INTO ZINTERNALRESOURCE VALUES(1,1,1);
            INSERT INTO ZCLOUDMASTER VALUES(1,3);")?;
        Ok((root, library))
    }

    #[test]
    fn indexed_original_custody_needs_no_album_tables_and_keeps_bytes() -> Result<()> {
        let (_root, library) = fixture()?;
        let database = library.join("database/Photos.sqlite");
        let before = std::fs::read(&database)?;
        let Lookup::Probes(probes) = query(&["UUID-A".into()], &library)? else { anyhow::bail!("recognized schema fell back") };
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].path, library.join("originals/A/asset.jxl").canonicalize()?);
        assert!(probes[0].iscloudasset && probes[0].incloud == Some(true) && !probes[0].ismissing);
        assert_eq!(std::fs::read(&database)?, before);
        assert_eq!(std::fs::read(&probes[0].path)?, b"exact-original");
        Ok(())
    }

    #[test]
    fn unsupported_and_incomplete_resource_evidence_never_becomes_custody() -> Result<()> {
        let (_root, library) = fixture()?;
        assert!(query(&["UUID-A".into(), "UUID-A".into()], &library).is_err());
        assert!(query(&["missing".into()], &library).is_err());
        for sql in [
            "UPDATE ZASSET SET ZDIRECTORY='../outside'",
            "UPDATE ZASSET SET ZFILENAME='../asset.jxl'",
            "UPDATE ZASSET SET ZTRASHEDSTATE=1",
            "DELETE FROM ZCLOUDMASTER",
            "DELETE FROM ZINTERNALRESOURCE",
            "INSERT INTO ZINTERNALRESOURCE VALUES(1,1,1)",
            "INSERT INTO ZASSET SELECT 2,ZUUID,ZDIRECTORY,ZFILENAME,ZCLOUDASSETGUID,ZMASTER,ZCLOUDBATCHPUBLISHDATE,ZSYNDICATIONSTATE,ZTRASHEDSTATE,ZMOMENTSHARE FROM ZASSET",
            "ALTER TABLE ZASSET RENAME COLUMN ZFILENAME TO renamed",
        ] {
            let (_case, library) = fixture()?;
            Connection::open(library.join("database/Photos.sqlite"))?.execute_batch(sql)?;
            assert!(query(&["UUID-A".into()], &library).is_err(), "{sql}");
        }
        let conn = Connection::open(library.join("database/Photos.sqlite"))?;
        conn.execute("UPDATE ZASSET SET ZDIRECTORY='/referenced'", [])?;
        assert!(matches!(query(&["UUID-A".into()], &library)?, Lookup::UnsupportedLayout(_)));
        conn.execute_batch("BEGIN EXCLUSIVE")?;
        let busy = match query(&["UUID-A".into()], &library) {
            Err(error) => error,
            Ok(_) => anyhow::bail!("locked database unexpectedly returned custody"),
        };
        assert!(busy.to_string().contains("locked") || busy.to_string().contains("busy"), "{busy}");
        assert!(super::super::photos_import_retry_reason(&busy.to_string()).is_some());
        conn.execute_batch("ROLLBACK")?;
        let (_old, old_library) = fixture()?;
        Connection::open(old_library.join("database/Photos.sqlite"))?
            .execute_batch("ALTER TABLE ZASSET RENAME COLUMN ZSYNDICATIONSTATE TO old_layout")?;
        assert!(matches!(query(&["UUID-A".into()], &old_library)?, Lookup::UnsupportedLayout(_)));
        Ok(())
    }
}

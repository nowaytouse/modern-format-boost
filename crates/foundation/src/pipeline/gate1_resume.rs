//! Reconstructable, per-file Gate 1 proofs. They never authorize Photos custody or deletion.
//! This cache has its own payload budget/TTL, using the shared per-cache numeric limits.

use super::verification::{PipelineCtx, WorkingCopyMarker};
use crate::db::mfb_sqlite_store::{self, NS_GATE1_PROOF};
use crate::metadata::XmpSidecarIndex;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

const RECEIPT_SCHEMA: i32 = 1;
const CHECK_SCHEMA: u32 = 1;
const MAINTENANCE_WRITES: usize = 256;

#[cfg(target_os = "macos")]
fn executing_worker_identity() -> io::Result<Option<String>> {
    use std::os::unix::fs::OpenOptionsExt;
    unsafe extern "C" {
        fn csops(
            pid: libc::pid_t,
            operation: libc::c_uint,
            data: *mut libc::c_void,
            size: usize,
        ) -> libc::c_int;
    }
    const CS_OPS_CDHASH: libc::c_uint = 5;
    // CS_OPS_CDHASH returns the kernel identity of this loaded image, not its current pathname.
    let mut loaded_cdhash = [0u8; 20];
    let pid = std::process::id().try_into().map_err(error)?;
    if unsafe {
        csops(
            pid,
            CS_OPS_CDHASH,
            loaded_cdhash.as_mut_ptr().cast(),
            loaded_cdhash.len(),
        )
    } != 0
    {
        return Ok(None);
    }
    let path = std::env::current_exe()?;
    let mut image = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let before = image.metadata()?;
    if !before.is_file() {
        return Ok(None);
    }
    let verified = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(&path)
        .output()?;
    if !verified.status.success() {
        return Ok(None);
    }
    let display = std::process::Command::new("/usr/bin/codesign")
        .args(["-d", "--verbose=4"])
        .arg(&path)
        .output()?;
    if !display.status.success() {
        return Ok(None);
    }
    let diagnostic = std::str::from_utf8(&display.stderr).map_err(error)?;
    let hashes = diagnostic
        .lines()
        .filter_map(|line| line.strip_prefix("CDHash="))
        .collect::<Vec<_>>();
    let loaded: String = loaded_cdhash
        .iter()
        .flat_map(|byte| {
            let digits = b"0123456789abcdef";
            [
                char::from(digits[usize::from(byte >> 4)]),
                char::from(digits[usize::from(byte & 15)]),
            ]
        })
        .collect();
    let at_path = std::fs::symlink_metadata(&path)?;
    if hashes.as_slice() != [loaded.as_str()]
        || at_path.file_type().is_symlink()
        || !same_open_file(&before, &at_path)?
    {
        return Ok(None);
    }
    let mut hash = blake3::Hasher::new();
    io::copy(&mut image, &mut hash)?;
    let after = image.metadata()?;
    let at_path = std::fs::symlink_metadata(&path)?;
    let mut still_loaded = [0u8; 20];
    if at_path.file_type().is_symlink()
        || !same_open_file(&before, &after)?
        || !same_open_file(&after, &at_path)?
        || unsafe {
            csops(
                pid,
                CS_OPS_CDHASH,
                still_loaded.as_mut_ptr().cast(),
                still_loaded.len(),
            )
        } != 0
        || still_loaded != loaded_cdhash
    {
        return Ok(None);
    }
    Ok(Some(hash.finalize().to_hex().to_string()))
}

#[cfg(target_os = "linux")]
fn executing_worker_identity() -> io::Result<Option<String>> {
    // This kernel-owned handle continues to identify the loaded executable after pathname replacement.
    let mut image = std::fs::File::open("/proc/self/exe")?;
    let before = image.metadata()?;
    let mut hash = blake3::Hasher::new();
    io::copy(&mut image, &mut hash)?;
    if !same_open_file(&before, &image.metadata()?)? {
        return Ok(None);
    }
    Ok(Some(hash.finalize().to_hex().to_string()))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn executing_worker_identity() -> io::Result<Option<String>> {
    Ok(None)
}

fn authority_digest(
    ctx: &PipelineCtx,
    marker: &WorkingCopyMarker,
    contract: &super::verification::FastImgProcessingContract,
    snapshot: &str,
    worker_blake3: &str,
) -> io::Result<String> {
    let bytes = serde_json::to_vec(&(
        CHECK_SCHEMA,
        &marker.started_at,
        &marker.src_dir,
        &marker.working_copy,
        &marker.strategy,
        marker.metadata_policy_version,
        contract,
        format!("{:?}", ctx.metadata_policy),
        format!("{:?}", ctx.output_format),
        snapshot,
        worker_blake3,
    ))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::other(error)
}

fn same_open_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> io::Result<bool> {
    let same = before.is_file()
        && after.is_file()
        && before.len() == after.len()
        && before.modified()? == after.modified()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(same
            && before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec())
    }
    #[cfg(not(unix))]
    {
        Ok(same)
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FileProofInput {
    path: PathBuf,
    bytes: u64,
    blake3: String,
}

impl FileProofInput {
    fn capture(path: &Path) -> io::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path)?;
        let before = file.metadata()?;
        if !before.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Gate proof input is not a regular file",
            ));
        }
        let mut hash = blake3::Hasher::new();
        io::copy(&mut file, &mut hash)?;
        let after = file.metadata()?;
        let at_path = std::fs::symlink_metadata(path)?;
        if at_path.file_type().is_symlink()
            || !same_open_file(&before, &after)?
            || !same_open_file(&after, &at_path)?
        {
            return Err(error(
                "Gate proof input changed while its complete content was hashed",
            ));
        }
        Ok(Self {
            path: path.to_owned(),
            bytes: after.len(),
            blake3: hash.finalize().to_hex().to_string(),
        })
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProofInputs {
    source: FileProofInput,
    output: FileProofInput,
    source_xmp: Option<FileProofInput>,
    output_xmp: Option<FileProofInput>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CachedPass {
    schema: i32,
    check_schema: u32,
    authority: String,
    check: String,
    inputs: ProofInputs,
}

pub(crate) struct Gate1ProofCache<'a> {
    context: &'a PipelineCtx,
    connection: Option<Connection>,
    authority: Option<String>,
    sidecars: XmpSidecarIndex,
    sources_by_output: BTreeMap<PathBuf, &'a str>,
    reused: Cell<usize>,
    fresh: Cell<usize>,
    invalidated: Cell<usize>,
    writes: Cell<usize>,
}

impl<'a> Gate1ProofCache<'a> {
    pub(crate) fn for_marker(
        ctx: &'a PipelineCtx,
        marker: &WorkingCopyMarker,
    ) -> io::Result<Option<Self>> {
        if ctx.src_dir != marker.src_dir
            || ctx.working_copy != marker.working_copy
            || ctx.blake3_log != marker.blake3_log
        {
            return Err(error("Gate 1 resume context differs from its task marker"));
        }
        if ctx.blake3_log.values().any(|entry| entry.out_rel.is_none()) {
            tracing::info!(target: "verification_gate_resume", "Gate 1 has no explicit complete output mapping; using the existing full verification path");
            return Ok(None);
        }
        let authority = match (
            &marker.processing_contract,
            &marker.tool_snapshot_identity,
            crate::common_utils::active_tool_snapshot(),
        ) {
            (Some(contract), Some(saved), Some(active)) if saved == &active.identity => {
                contract.validate(&marker.strategy).map_err(error)?;
                crate::common_utils::verify_tool_runtime_snapshot().map_err(error)?;
                let worker = match executing_worker_identity() {
                    Ok(Some(worker)) => worker,
                    Ok(None) => {
                        tracing::warn!(target: "verification_gate_resume", "Loaded worker identity could not be proven against its current executable; Gate 1 will run fully without reusable proofs");
                        return Self::new(ctx, None).map(Some);
                    }
                    Err(error) => {
                        tracing::warn!(target: "verification_gate_resume", reason = %error, "Loaded worker identity is unavailable; Gate 1 will run fully without reusable proofs");
                        return Self::new(ctx, None).map(Some);
                    }
                };
                Some(authority_digest(ctx, marker, contract, saved, &worker)?)
            }
            (Some(_), Some(saved), Some(active)) if saved != &active.identity => {
                return Err(error(
                    "Gate 1 resume tool authority differs from the saved task",
                ));
            }
            _ => {
                tracing::info!(target: "verification_gate_resume", "Gate 1 has no complete bound resume authority; every check will run fresh");
                None
            }
        };
        Self::new(ctx, authority).map(Some)
    }

    fn new(ctx: &'a PipelineCtx, authority: Option<String>) -> io::Result<Self> {
        let mut paths = Vec::with_capacity(ctx.blake3_log.len().saturating_mul(2));
        let mut sources_by_output = BTreeMap::new();
        for (source, entry) in &ctx.blake3_log {
            let output = ctx
                .working_copy
                .join(entry.out_rel.as_deref().ok_or_else(|| {
                    error("Gate 1 reusable proof requires an explicit output mapping")
                })?);
            paths.push(ctx.src_dir.join(source));
            paths.push(output.clone());
            if sources_by_output.insert(output, source.as_str()).is_some() {
                return Err(error("Gate 1 resume output mapping is ambiguous"));
            }
        }
        let sidecars = XmpSidecarIndex::capture(&paths)?;
        let connection = if authority.is_some() {
            let conn = mfb_sqlite_store::open_store_connection().map_err(error)?;
            maintain(&conn)?;
            Some(conn)
        } else {
            None
        };
        Ok(Self {
            context: ctx,
            connection,
            authority,
            sidecars,
            sources_by_output,
            reused: Cell::new(0),
            fresh: Cell::new(0),
            invalidated: Cell::new(0),
            writes: Cell::new(0),
        })
    }

    pub(crate) const fn sidecars(&self) -> &XmpSidecarIndex {
        &self.sidecars
    }

    #[cfg(test)]
    pub(super) fn for_tests(ctx: &'a PipelineCtx, authority: &str) -> io::Result<Self> {
        Self::new(ctx, Some(authority.to_owned()))
    }

    #[cfg(test)]
    pub(super) const fn counts(&self) -> (usize, usize, usize) {
        (self.reused.get(), self.fresh.get(), self.invalidated.get())
    }

    pub(crate) fn source_for_output(&self, output: &Path) -> io::Result<PathBuf> {
        self.sources_by_output
            .get(output)
            .map(|source| self.context.src_dir.join(source))
            .ok_or_else(|| error("Gate 1 resume output has no unique source mapping"))
    }

    fn inputs(&self, source: &Path, output: &Path) -> io::Result<ProofInputs> {
        let rel = self
            .sources_by_output
            .get(output)
            .ok_or_else(|| error("Gate 1 proof output mapping is absent"))?;
        if self.context.src_dir.join(rel) != source {
            return Err(error("Gate 1 proof source mapping changed"));
        }
        let expected = &self.context.blake3_log[*rel];
        let inputs = ProofInputs {
            source: FileProofInput::capture(source)?,
            output: FileProofInput::capture(output)?,
            source_xmp: self
                .sidecars
                .find(source)?
                .as_deref()
                .map(FileProofInput::capture)
                .transpose()?,
            output_xmp: self
                .sidecars
                .find(output)?
                .as_deref()
                .map(FileProofInput::capture)
                .transpose()?,
        };
        if inputs.source.blake3 != expected.src || inputs.output.blake3 != expected.out {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Gate 1 proof inputs differ from the current source/output BLAKE3 manifest",
            ));
        }
        Ok(inputs)
    }

    pub(crate) fn verify(
        &self,
        check: &str,
        source: &Path,
        output: &Path,
        probe: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        crate::batch_control::checkpoint().map_err(error)?;
        let (Some(authority), Some(conn)) = (&self.authority, &self.connection) else {
            probe()?;
            self.fresh.set(self.fresh.get() + 1);
            return Ok(());
        };
        crate::common_utils::verify_tool_runtime().map_err(error)?;
        let inputs = self.inputs(source, output)?;
        let key =
            blake3::hash(&serde_json::to_vec(&(authority, check, source, output)).map_err(error)?)
                .to_hex()
                .to_string();
        if let Some(bytes) = mfb_sqlite_store::read_blob(conn, NS_GATE1_PROOF, &key, RECEIPT_SCHEMA)
            .map_err(error)?
        {
            let saved: CachedPass = serde_json::from_slice(&bytes).map_err(error)?;
            if saved.schema == RECEIPT_SCHEMA
                && saved.check_schema == CHECK_SCHEMA
                && saved.authority == *authority
                && saved.check == check
                && saved.inputs == inputs
            {
                self.reused.set(self.reused.get() + 1);
                tracing::debug!(target: "verification_gate_resume", check, "reused Gate 1 proof after fresh complete input hashes");
                return Ok(());
            }
            self.invalidated.set(self.invalidated.get() + 1);
        }
        probe()?;
        crate::batch_control::checkpoint().map_err(error)?;
        crate::common_utils::verify_tool_runtime().map_err(error)?;
        if inputs != self.inputs(source, output)? {
            return Err(error(
                "Gate 1 proof inputs changed during verification; no pass was saved",
            ));
        }
        let receipt = CachedPass {
            schema: RECEIPT_SCHEMA,
            check_schema: CHECK_SCHEMA,
            authority: authority.clone(),
            check: check.to_owned(),
            inputs,
        };
        let payload = serde_json::to_vec(&receipt).map_err(error)?;
        let now = chrono::Utc::now().timestamp();
        mfb_sqlite_store::upsert_blob(
            conn,
            NS_GATE1_PROOF,
            &key,
            RECEIPT_SCHEMA,
            self.context.src_dir.to_str(),
            &payload,
            now,
        )
        .map_err(error)?;
        self.fresh.set(self.fresh.get() + 1);
        self.writes.set(self.writes.get() + 1);
        if self.writes.get().is_multiple_of(MAINTENANCE_WRITES) {
            maintain(conn)?;
        }
        Ok(())
    }

    pub(crate) fn finish(&self) -> io::Result<()> {
        self.sidecars.verify_unchanged()?;
        if self.authority.is_some() {
            crate::common_utils::verify_tool_runtime_snapshot().map_err(error)?;
        }
        if let Some(conn) = &self.connection {
            maintain(conn)?;
        }
        tracing::info!(target: "verification_gate_resume", reused = self.reused.get(), fresh = self.fresh.get(), invalidated = self.invalidated.get(),
            "Gate 1 proof cache: reused results are not fresh checks; count, size, BLAKE3 and applicable filesystem metadata checks are never reused");
        Ok(())
    }
}

fn maintain(conn: &Connection) -> io::Result<()> {
    let policy = crate::runtime_config::active()
        .map_or_else(crate::runtime_config::CachePolicy::default, |config| {
            config.cache
        });
    maintain_at(conn, policy, chrono::Utc::now().timestamp())
}

fn maintain_at(
    conn: &Connection,
    policy: crate::runtime_config::CachePolicy,
    now: i64,
) -> io::Result<()> {
    policy.validate().map_err(error)?;
    let ttl = i64::try_from(policy.path_tree_ttl_seconds).map_err(error)?;
    let max_bytes = i64::try_from(policy.path_tree_max_bytes).map_err(error)?;
    conn.execute(
        "DELETE FROM blob_store WHERE namespace=?1 AND (updated_at<=?2 OR updated_at>?3)",
        params![NS_GATE1_PROOF, now.saturating_sub(ttl), now],
    )
    .map_err(error)?;
    conn.execute("DELETE FROM blob_store WHERE namespace=?1 AND cache_key IN (
        SELECT cache_key FROM (SELECT cache_key,SUM(length(payload)) OVER (
        ORDER BY updated_at DESC,cache_key DESC) AS retained_bytes FROM blob_store WHERE namespace=?1)
        WHERE retained_bytes>?2)", params![NS_GATE1_PROOF, max_bytes]).map_err(error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::verification::{
        Blake3Entry, FastImgEncodingParameters, FastImgProcessingContract,
    };
    use super::*;

    fn fixture(root: &Path) -> io::Result<(PipelineCtx, PathBuf, PathBuf)> {
        let source_root = root.join("source");
        let output_root = root.join("output");
        std::fs::create_dir_all(&source_root)?;
        std::fs::create_dir_all(&output_root)?;
        let source = source_root.join("a.jpeg");
        let output = output_root.join("a.JXL");
        std::fs::write(&source, b"synthetic source")?;
        std::fs::write(&output, b"synthetic output")?;
        let ctx = PipelineCtx {
            src_dir: source_root,
            working_copy: output_root,
            blake3_log: BTreeMap::from([(
                "a.jpeg".into(),
                Blake3Entry {
                    out_rel: Some("a.JXL".into()),
                    src: FileProofInput::capture(&source)?.blake3,
                    out: FileProofInput::capture(&output)?.blake3,
                    library_asset: None,
                },
            )]),
            expected_count: 1,
            library_handle: None,
            metadata_policy: crate::metadata::MetadataOutputPolicy::Preserve,
            output_format: Some(crate::image::format_detect::FormatKind::Jxl),
        };
        Ok((ctx, source, output))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn gate1_worker_identity_subprocess() -> io::Result<()> {
        let Some(directory) = std::env::var_os("MFB_GATE_WORKER_ID_TEST_DIR") else {
            return Ok(());
        };
        let directory = PathBuf::from(directory);
        let initial = executing_worker_identity()?
            .ok_or_else(|| error("test worker has no verified loaded identity"))?;
        std::fs::write(directory.join("ready"), initial)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !directory.join("replaced").exists() {
            if std::time::Instant::now() >= deadline {
                return Err(error("test worker replacement was not published"));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let after = executing_worker_identity()?;
        std::fs::write(
            directory.join("result"),
            if after.is_none() {
                "disabled"
            } else {
                "incorrectly trusted replacement"
            },
        )?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn gate1_resume_worker_identity_rejects_atomically_replaced_executable() -> io::Result<()> {
        use std::process::{Command, Stdio};
        let temp = tempfile::tempdir()?;
        let worker = temp.path().join("worker");
        std::fs::copy(std::env::current_exe()?, &worker)?;
        let helper = format!(
            "{}::gate1_worker_identity_subprocess",
            module_path!().split_once("::").unwrap().1
        );
        let mut child = Command::new(&worker)
            .args(["--exact", &helper, "--nocapture"])
            .env("MFB_GATE_WORKER_ID_TEST_DIR", temp.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !temp.path().join("ready").exists() {
            if child.try_wait()?.is_some() || std::time::Instant::now() >= deadline {
                if child.try_wait()?.is_none() {
                    child.kill()?;
                }
                let diagnostics = child.wait_with_output()?;
                return Err(error(format!(
                    "test worker did not establish its loaded identity: {} {}",
                    String::from_utf8_lossy(&diagnostics.stdout),
                    String::from_utf8_lossy(&diagnostics.stderr)
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let replacement = temp.path().join("replacement");
        std::fs::copy("/usr/bin/true", &replacement)?;
        std::fs::rename(&replacement, &worker)?;
        std::fs::write(temp.path().join("replaced"), b"replaced")?;
        while !temp.path().join("result").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !temp.path().join("result").exists() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
            return Err(error(
                "test worker did not finish its replacement identity check",
            ));
        }
        assert_eq!(
            std::fs::read_to_string(temp.path().join("result"))?,
            "disabled"
        );
        assert!(child.wait_with_output()?.status.success());
        Ok(())
    }

    #[test]
    fn gate1_resume_legacy_missing_authority_or_mapping_uses_full_verification() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let (mut ctx, _, _) = fixture(temp.path())?;
        let mut marker = WorkingCopyMarker::new(ctx.src_dir.clone(), ctx.working_copy.clone(), 1);
        marker.blake3_log = ctx.blake3_log.clone();
        let uncached = Gate1ProofCache::for_marker(&ctx, &marker)?.unwrap();
        assert!(uncached.connection.is_none());
        assert!(uncached.authority.is_none());
        drop(uncached);
        ctx.blake3_log.get_mut("a.jpeg").unwrap().out_rel = None;
        ctx.output_format = Some(crate::image::format_detect::FormatKind::Avif);
        marker.blake3_log = ctx.blake3_log.clone();
        marker.strategy = "avif".into();
        assert!(Gate1ProofCache::for_marker(&ctx, &marker)?.is_none());
        let old = super::super::verification::Gate1Local
            .run_with_error_mode(&ctx, crate::BatchErrorMode::LogAndContinue);
        let new = super::super::verification::Gate1Local.run_with_error_mode_and_resume(
            &ctx,
            crate::BatchErrorMode::LogAndContinue,
            &marker,
        );
        assert_eq!(old.checks, new.checks);
        assert_eq!(old.passed, new.passed);
        Ok(())
    }

    #[test]
    fn gate1_resume_commits_completed_files_and_never_caches_failure_or_interruption()
    -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let _store =
            mfb_sqlite_store::set_test_store_path_for_tests(temp.path().join("store.sqlite"));
        let (ctx, source, output) = fixture(temp.path())?;
        let cache = Gate1ProofCache::new(&ctx, Some("authority".into()))?;
        cache.verify("embedded_metadata", &source, &output, || Ok(()))?;
        let interrupted = cache.verify("decode", &source, &output, || {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "synthetic interruption",
            ))
        });
        assert!(interrupted.is_err());
        drop(cache);
        let resumed = Gate1ProofCache::new(&ctx, Some("authority".into()))?;
        resumed.verify("embedded_metadata", &source, &output, || {
            panic!("committed proof must resume")
        })?;
        let called = Cell::new(false);
        resumed.verify("decode", &source, &output, || {
            called.set(true);
            Ok(())
        })?;
        assert!(called.get());
        assert_eq!((resumed.reused.get(), resumed.fresh.get()), (1, 1));
        resumed.finish()?;
        Ok(())
    }

    #[test]
    fn gate1_resume_rehashes_same_size_inputs_and_xmp_and_rejects_mid_probe_changes()
    -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let _store =
            mfb_sqlite_store::set_test_store_path_for_tests(temp.path().join("store.sqlite"));
        let (mut ctx, source, output) = fixture(temp.path())?;
        let xmp = source.with_extension("xmp");
        std::fs::write(&xmp, b"xmp one")?;
        let cache = Gate1ProofCache::new(&ctx, Some("authority".into()))?;
        cache.verify("decode", &source, &output, || Ok(()))?;
        drop(cache);
        let modified = std::fs::metadata(&output)?.modified()?;
        std::fs::write(&output, b"synthetic broken")?;
        filetime::set_file_mtime(&output, filetime::FileTime::from_system_time(modified))?;
        let cache = Gate1ProofCache::new(&ctx, Some("authority".into()))?;
        assert!(
            cache
                .verify("decode", &source, &output, || panic!(
                    "hash drift must reject before probing"
                ))
                .is_err()
        );
        drop(cache);
        ctx.blake3_log.get_mut("a.jpeg").unwrap().out = FileProofInput::capture(&output)?.blake3;
        let cache = Gate1ProofCache::new(&ctx, Some("authority".into()))?;
        cache.verify("decode", &source, &output, || Ok(()))?;
        assert_eq!(cache.invalidated.get(), 1);
        drop(cache);
        std::fs::write(&xmp, b"xmp two")?;
        let cache = Gate1ProofCache::new(&ctx, Some("authority".into()))?;
        cache.verify("decode", &source, &output, || Ok(()))?;
        assert_eq!((cache.reused.get(), cache.invalidated.get()), (0, 1));
        assert!(
            cache
                .verify("orientation_reconstruction", &source, &output, || {
                    std::fs::write(&output, b"changed during proof")
                })
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn gate1_resume_authority_binds_worker_tools_contract_and_check_implementation()
    -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let (ctx, _, _) = fixture(temp.path())?;
        let marker = WorkingCopyMarker::new(ctx.src_dir.clone(), ctx.working_copy.clone(), 1);
        let mut contract = FastImgProcessingContract {
            schema: 1,
            tool_policy: crate::runtime_config::ToolPolicy::Fallback,
            fallback_policy: crate::runtime_config::FallbackPolicy::Strict,
            encoding: FastImgEncodingParameters::Jxl {
                jpeg_effort: 11,
                jpeg_effort_search: crate::runtime_config::JpegEffortSearch::Full,
                archive: false,
                allow_expert_options: false,
            },
        };
        let original = authority_digest(&ctx, &marker, &contract, "tools one", "worker one")?;
        assert_ne!(
            original,
            authority_digest(&ctx, &marker, &contract, "tools two", "worker one")?
        );
        assert_ne!(
            original,
            authority_digest(&ctx, &marker, &contract, "tools one", "worker two")?
        );
        if let FastImgEncodingParameters::Jxl { jpeg_effort, .. } = &mut contract.encoding {
            *jpeg_effort = 10;
        }
        assert_ne!(
            original,
            authority_digest(&ctx, &marker, &contract, "tools one", "worker one")?
        );
        Ok(())
    }

    #[test]
    fn gate1_resume_corruption_and_failed_commit_never_become_pass() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let _store =
            mfb_sqlite_store::set_test_store_path_for_tests(temp.path().join("store.sqlite"));
        let (ctx, source, output) = fixture(temp.path())?;
        let cache = Gate1ProofCache::new(&ctx, Some("authority".into()))?;
        let conn = cache.connection.as_ref().unwrap();
        conn.execute_batch("CREATE TRIGGER reject_gate_proof BEFORE INSERT ON blob_store WHEN NEW.namespace='gate1_verified_file' BEGIN SELECT RAISE(ABORT,'synthetic commit failure'); END;").map_err(error)?;
        assert!(cache.verify("decode", &source, &output, || Ok(())).is_err());
        assert_eq!(cache.fresh.get(), 0);
        conn.execute_batch("DROP TRIGGER reject_gate_proof;")
            .map_err(error)?;
        cache.verify("decode", &source, &output, || Ok(()))?;
        conn.execute(
            "UPDATE blob_store SET payload=?1,payload_blake3=?2 WHERE namespace=?3",
            params![
                b"{".as_slice(),
                blake3::hash(b"{").as_bytes().as_slice(),
                NS_GATE1_PROOF
            ],
        )
        .map_err(error)?;
        let corruption = cache
            .verify("decode", &source, &output, || {
                panic!("corrupt receipt cannot pass")
            })
            .expect_err("corrupt saved proof is a shared persistence failure");
        assert_eq!(corruption.kind(), io::ErrorKind::Other);
        assert_eq!(cache.reused.get(), 0);
        Ok(())
    }

    #[test]
    fn gate1_resume_lifecycle_removes_only_reconstructable_proofs() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let _store =
            mfb_sqlite_store::set_test_store_path_for_tests(temp.path().join("store.sqlite"));
        let conn = mfb_sqlite_store::open_store_connection().map_err(error)?;
        mfb_sqlite_store::upsert_blob(
            &conn,
            mfb_sqlite_store::NS_CHECKPOINT,
            "state",
            1,
            None,
            b"retain",
            10,
        )
        .map_err(error)?;
        for (key, now) in [("old", 1), ("new", 100)] {
            mfb_sqlite_store::upsert_blob(&conn, NS_GATE1_PROOF, key, 1, None, b"proof", now)
                .map_err(error)?;
        }
        maintain_at(
            &conn,
            crate::runtime_config::CachePolicy {
                path_tree_max_bytes: 3,
                path_tree_ttl_seconds: 30,
            },
            100,
        )?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM blob_store WHERE namespace=?1",
                [NS_GATE1_PROOF],
                |row| row.get(0),
            )
            .map_err(error)?;
        assert_eq!(count, 0);
        assert_eq!(
            mfb_sqlite_store::read_blob(&conn, mfb_sqlite_store::NS_CHECKPOINT, "state", 1)
                .map_err(error)?,
            Some(b"retain".to_vec())
        );
        Ok(())
    }
}

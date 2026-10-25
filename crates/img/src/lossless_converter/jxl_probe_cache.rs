//! Disposable candidates owned by one JXL search, never persistent delivery proof.

use std::collections::VecDeque;
use std::fs::Metadata;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct JxlProbeCacheStats {
    pub requests: u64,
    pub hits: u64,
    pub invalidated: u64,
    pub evicted: u64,
    pub retained_bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct ReadStamp {
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl ReadStamp {
    fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified()?,
            #[cfg(unix)]
            identity: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        })
    }
}

fn read_stamp(path: &Path) -> io::Result<Option<ReadStamp>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() > 0 =>
        {
            ReadStamp::from_metadata(&metadata).map(Some)
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn fingerprint(path: &Path) -> io::Result<Option<(u64, String)>> {
    let Some(before) = read_stamp(path)? else {
        return Ok(None);
    };
    let hash = match foundation::common_utils::calculate_blake3_hash(path) {
        Ok(hash) => hash,
        Err(error) => {
            let error = error
                .downcast::<io::Error>()
                .unwrap_or_else(io::Error::other);
            return if error.kind() == io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            };
        }
    };
    let Some(after) = read_stamp(path)? else {
        return Ok(None);
    };
    Ok((before == after).then_some((after.length, hash)))
}

struct Candidate {
    key: String,
    path: PathBuf,
    file_bytes: u64,
    hash: String,
    payload_size: u64,
    _guard: foundation::conversion::TempOutputGuard,
}

pub(super) struct JxlProbeCache {
    candidates: VecDeque<Candidate>,
    max_entries: usize,
    max_bytes: u64,
    stats: JxlProbeCacheStats,
}

fn next_counter(value: u64) -> io::Result<u64> {
    value
        .checked_add(1)
        .ok_or_else(|| io::Error::other("JXL probe reuse counter overflow"))
}

fn with_cleanup_error(error: io::Error, path: &Path) -> io::Error {
    match foundation::io_utils::safe_remove_file(path) {
        Ok(()) => error,
        Err(cleanup) => io::Error::other(format!("{error}; candidate cleanup failed: {cleanup}")),
    }
}

impl JxlProbeCache {
    pub(super) fn new(max_entries: usize, max_bytes: u64) -> Self {
        Self {
            candidates: VecDeque::new(),
            max_entries,
            max_bytes,
            stats: JxlProbeCacheStats::default(),
        }
    }

    pub(super) const fn stats(&self) -> JxlProbeCacheStats {
        self.stats
    }

    fn detach(&mut self, index: usize) -> io::Result<Candidate> {
        let candidate = self
            .candidates
            .get(index)
            .ok_or_else(|| io::Error::other("JXL probe candidate disappeared"))?;
        let remaining = self
            .stats
            .retained_bytes
            .checked_sub(candidate.file_bytes)
            .ok_or_else(|| io::Error::other("JXL probe byte accounting underflow"))?;
        let candidate = self
            .candidates
            .remove(index)
            .ok_or_else(|| io::Error::other("JXL probe candidate disappeared"))?;
        self.stats.retained_bytes = remaining;
        Ok(candidate)
    }

    fn discard(&mut self, index: usize) -> io::Result<()> {
        let candidate = self
            .candidates
            .get(index)
            .ok_or_else(|| io::Error::other("JXL probe candidate disappeared"))?;
        foundation::io_utils::safe_remove_file(&candidate.path)?;
        self.detach(index)?;
        Ok(())
    }

    fn invalidate(&mut self, index: usize, reason: &str) -> io::Result<()> {
        let invalidated = next_counter(self.stats.invalidated)?;
        tracing::warn!(target: "jxl_probe_cache", reason, "JXL probe reuse invalidated; candidate must be recomputed");
        self.discard(index)?;
        self.stats.invalidated = invalidated;
        Ok(())
    }

    /// Takes ownership of a disposable, private search output, including on rejection.
    pub(super) fn retain(
        &mut self,
        key: String,
        path: PathBuf,
        payload_size: u64,
    ) -> io::Result<bool> {
        if self
            .candidates
            .iter()
            .any(|candidate| candidate.path == path)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "JXL probe candidate is already owned",
            ));
        }
        let guard = foundation::conversion::TempOutputGuard::new(path.clone());
        let retained = (|| -> io::Result<Option<(u64, String)>> {
            let initial = read_stamp(&path)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JXL probe candidate is missing, non-regular, or empty",
                )
            })?;
            if self.max_entries == 0 || initial.length > self.max_bytes {
                return Ok(None);
            }
            let (length, hash) = fingerprint(&path)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JXL probe candidate is missing, non-regular, empty, or changed while hashing",
                )
            })?;
            if payload_size == 0 || payload_size > length {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JXL probe payload size is invalid",
                ));
            }
            if self.max_entries == 0 || length > self.max_bytes {
                return Ok(None);
            }
            if let Some(index) = self
                .candidates
                .iter()
                .position(|candidate| candidate.key == key)
            {
                let evicted = next_counter(self.stats.evicted)?;
                self.discard(index)?;
                self.stats.evicted = evicted;
            }
            while self.candidates.len() >= self.max_entries
                || self.stats.retained_bytes > self.max_bytes - length
            {
                let evicted = next_counter(self.stats.evicted)?;
                self.discard(0)?;
                self.stats.evicted = evicted;
            }
            Ok(Some((length, hash)))
        })();
        match retained {
            Ok(Some((file_bytes, hash))) => {
                self.stats.retained_bytes += file_bytes;
                self.candidates.push_back(Candidate {
                    key,
                    path,
                    file_bytes,
                    hash,
                    payload_size,
                    _guard: guard,
                });
                Ok(true)
            }
            Ok(None) => {
                foundation::io_utils::safe_remove_file(&path)?;
                Ok(false)
            }
            Err(error) => Err(with_cleanup_error(error, &path)),
        }
    }

    /// Moves a proven candidate to an unoccupied private path; the caller must recheck its media semantics.
    pub(super) fn take(&mut self, key: &str, output: &Path) -> io::Result<Option<u64>> {
        self.stats.requests = next_counter(self.stats.requests)?;
        let Some(index) = self
            .candidates
            .iter()
            .position(|candidate| candidate.key == key)
        else {
            return Ok(None);
        };
        match std::fs::symlink_metadata(output) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "JXL probe reuse destination is occupied",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let candidate = self
            .candidates
            .get(index)
            .ok_or_else(|| io::Error::other("JXL probe candidate disappeared"))?;
        let current = fingerprint(&candidate.path)?;
        if !current.as_ref().is_some_and(|(length, hash)| {
            *length == candidate.file_bytes && *hash == candidate.hash
        }) {
            self.invalidate(index, "retained payload changed or disappeared")?;
            return Ok(None);
        }
        let hits = next_counter(self.stats.hits)?;
        let invalidated = next_counter(self.stats.invalidated)?;
        let parent = output.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "JXL probe output has no parent",
            )
        })?;
        let staged = foundation::media_conversion_gate::delivery_named_tempfile_in_parent_or_err(
            "JXL probe reuse",
            parent,
            ".mfb-jxl-probe-reuse-",
            "",
        )?
        .into_temp_path();
        if let Err(error) = foundation::io_utils::robust_move(&candidate.path, &staged) {
            return Err(with_cleanup_error(error, &staged));
        }
        if let Err(error) = staged.persist_noclobber(output) {
            let failure = with_cleanup_error(error.error, &error.path);
            if let Err(cleanup) = self.invalidate(index, "candidate transport did not commit") {
                return Err(io::Error::other(format!(
                    "{failure}; retained candidate cleanup failed: {cleanup}"
                )));
            }
            return Err(failure);
        }
        let candidate = match self.detach(index) {
            Ok(candidate) => candidate,
            Err(error) => return Err(with_cleanup_error(error, output)),
        };
        let verified = fingerprint(output);
        match verified {
            Ok(Some((length, hash)))
                if length == candidate.file_bytes && hash == candidate.hash =>
            {
                self.stats.hits = hits;
                Ok(Some(candidate.payload_size))
            }
            Ok(_) => {
                tracing::warn!(target: "jxl_probe_cache", "JXL probe reuse invalidated after transport; candidate must be recomputed");
                self.stats.invalidated = invalidated;
                foundation::io_utils::safe_remove_file(output)?;
                Ok(None)
            }
            Err(error) => Err(with_cleanup_error(error, output)),
        }
    }

    pub(super) fn clear(&mut self) -> io::Result<()> {
        while !self.candidates.is_empty() {
            self.discard(0)?;
        }
        Ok(())
    }
}

impl Drop for JxlProbeCache {
    fn drop(&mut self) {
        for candidate in &self.candidates {
            if let Err(error) = foundation::io_utils::safe_remove_file(&candidate.path) {
                foundation::media_conversion_gate::delivery_cleanup_audit(
                    &candidate.path,
                    "jxl_probe_cache_drop",
                    error,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(root: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        let path = root.join(name);
        std::fs::write(&path, bytes)?;
        Ok(path)
    }

    #[test]
    fn jxl_probe_cache_moves_exact_candidate_and_counts_requests_not_encodes() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = candidate(root.path(), "candidate.jxl", b"complete candidate bytes")?;
        let original = std::fs::metadata(&source)?;
        let mut cache = JxlProbeCache::new(2, 100);
        assert!(cache.retain("exact-key".to_string(), source.clone(), 10)?);
        let output = root.path().join("reuse.jxl");
        assert_eq!(cache.take("other-key", &output)?, None);
        assert!(source.exists());
        assert_eq!(cache.take("exact-key", &output)?, Some(10));
        assert!(!source.exists());
        assert_eq!(std::fs::read(&output)?, b"complete candidate bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&output)?.ino(), original.ino());
        }
        #[cfg(not(unix))]
        assert_eq!(std::fs::metadata(&output)?.len(), original.len());
        assert_eq!(
            cache.stats(),
            JxlProbeCacheStats {
                requests: 2,
                hits: 1,
                ..JxlProbeCacheStats::default()
            }
        );
        cache.clear()?;
        assert!(output.exists());
        Ok(())
    }

    #[test]
    fn jxl_probe_cache_invalidates_corrupt_or_missing_candidates() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        for missing in [false, true] {
            let source = candidate(root.path(), "candidate.jxl", b"original bytes")?;
            let mut cache = JxlProbeCache::new(1, 100);
            cache.retain("key".to_string(), source.clone(), 10)?;
            if missing {
                std::fs::remove_file(&source)?;
            } else {
                std::fs::write(&source, b"modified bytes")?;
            }
            let output = root.path().join("reuse.jxl");
            assert_eq!(cache.take("key", &output)?, None);
            assert_eq!(cache.stats().invalidated, 1);
            assert_eq!(cache.stats().retained_bytes, 0);
            assert!(!source.exists());
            assert!(!output.exists());
        }
        Ok(())
    }

    #[test]
    fn jxl_probe_cache_bounds_disk_bytes_with_fifo_and_cleans_rejection() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let first = candidate(root.path(), "first.jxl", b"1234")?;
        let second = candidate(root.path(), "second.jxl", b"5678")?;
        let third = candidate(root.path(), "third.jxl", b"9012")?;
        let oversized = candidate(root.path(), "oversized.jxl", b"123456789")?;
        let mut cache = JxlProbeCache::new(2, 8);
        cache.retain("first".to_string(), first.clone(), 3)?;
        cache.retain("second".to_string(), second.clone(), 3)?;
        cache.retain("third".to_string(), third.clone(), 3)?;
        assert!(!first.exists());
        assert!(second.exists() && third.exists());
        assert!(!cache.retain("oversized".to_string(), oversized.clone(), 4)?);
        assert!(!oversized.exists());
        assert_eq!(cache.stats().evicted, 1);
        assert_eq!(cache.stats().retained_bytes, 8);
        cache.clear()?;
        assert!(!second.exists() && !third.exists());
        assert_eq!(cache.stats().retained_bytes, 0);
        let mut byte_limited = JxlProbeCache::new(8, 5);
        byte_limited.retain(
            "first".to_string(),
            candidate(root.path(), "first.jxl", b"1234")?,
            3,
        )?;
        byte_limited.retain(
            "second".to_string(),
            candidate(root.path(), "second.jxl", b"5678")?,
            3,
        )?;
        assert!(!first.exists());
        assert_eq!(byte_limited.stats().evicted, 1);
        assert_eq!(byte_limited.stats().retained_bytes, 4);
        byte_limited.clear()?;
        let rejected = candidate(root.path(), "disabled.jxl", b"1")?;
        let mut disabled = JxlProbeCache::new(0, 0);
        assert!(!disabled.retain("disabled".to_string(), rejected.clone(), 1)?);
        assert!(!rejected.exists());
        Ok(())
    }

    #[test]
    fn jxl_probe_cache_preserves_occupied_destination_and_drop_cleans_only_owned_files()
    -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = candidate(root.path(), "candidate.jxl", b"candidate")?;
        let output = candidate(root.path(), "occupied.jxl", b"existing output")?;
        {
            let mut cache = JxlProbeCache::new(1, 100);
            cache.retain("key".to_string(), source.clone(), 5)?;
            let error = cache
                .take("key", &output)
                .expect_err("occupied output must never be replaced");
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert!(source.exists());
            assert_eq!(std::fs::read(&output)?, b"existing output");
        }
        assert!(!source.exists());
        assert_eq!(std::fs::read(&output)?, b"existing output");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn jxl_probe_cache_rejects_symlink_candidate_without_deleting_target() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let target = candidate(root.path(), "target.jxl", b"original target")?;
        let source = root.path().join("candidate.jxl");
        let mut cache = JxlProbeCache::new(1, 100);
        cache.retain(
            "key".to_string(),
            candidate(root.path(), "candidate.jxl", b"candidate")?,
            5,
        )?;
        std::fs::remove_file(&source)?;
        std::os::unix::fs::symlink(&target, &source)?;
        assert_eq!(cache.take("key", &root.path().join("reuse.jxl"))?, None);
        assert!(!source.exists());
        assert_eq!(std::fs::read(&target)?, b"original target");
        Ok(())
    }
}

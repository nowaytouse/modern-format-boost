//! Persistent, single-writer `PhotoKit` transport. No retry after an import intent.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

pub(super) struct Client {
    stream: BufReader<UnixStream>,
    socket_dir: Option<tempfile::TempDir>,
    pending: Option<(Value, crate::image::photos_import_metrics::Timer)>,
}

fn retain_worker_log(socket_dir: tempfile::TempDir, failure: Option<&[u8]>) -> String {
    let directory = socket_dir.keep();
    if let Some(failure) = failure {
        const MAX_FAILURE_BYTES: usize = 64 * 1024;
        let result = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("failure.log"))
            .and_then(|mut file| file.write_all(&failure[..failure.len().min(MAX_FAILURE_BYTES)]));
        if let Err(error) = result {
            return format!(
                "private diagnostic directory: {}; could not save additional failure details: {error}",
                directory.display()
            );
        }
    }
    format!("private diagnostic directory: {}", directory.display())
}

impl Client {
    pub(super) fn start(app: &Path, state: &Path, witnesses: &[String]) -> anyhow::Result<Self> {
        anyhow::ensure!(app.is_dir(), "PhotoKit helper app is unavailable");
        anyhow::ensure!(
            !witnesses.is_empty(),
            "PhotoKit needs selected-library witnesses"
        );
        std::fs::create_dir_all(state)?;
        anyhow::ensure!(
            !std::fs::symlink_metadata(state)?.file_type().is_symlink(),
            "native state directory is a symlink"
        );
        std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o700))?;
        let socket_dir = tempfile::Builder::new()
            .prefix("mfb-pk-")
            .tempdir_in("/tmp")?;
        std::fs::set_permissions(socket_dir.path(), std::fs::Permissions::from_mode(0o700))?;
        let socket = socket_dir.path().join("ipc");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let writer_lock = crate::infra::production_lock::native_writer_lock_path()?;
        let launch = std::process::Command::new("/usr/bin/open")
            .args(["-n", "-a"])
            .arg(app)
            .arg("--stderr")
            .arg(socket_dir.path().join("worker.log"))
            .arg("--args")
            .arg("--socket")
            .arg(&socket)
            .arg("--lock")
            .arg(writer_lock)
            .output();
        let launch = match launch {
            Ok(launch) => launch,
            Err(error) => anyhow::bail!(
                "PhotoKit helper launch failed: {error}; {}",
                retain_worker_log(socket_dir, None)
            ),
        };
        anyhow::ensure!(
            launch.status.success(),
            "PhotoKit helper launch failed: {}; {}",
            launch.status,
            retain_worker_log(socket_dir, Some(&launch.stderr))
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => anyhow::bail!(
                    "PhotoKit helper connection failed: {error}; {}",
                    retain_worker_log(socket_dir, None)
                ),
            }
        };
        // macOS accepts inherit the listening socket's nonblocking flag.
        if let Err(error) = (|| -> std::io::Result<()> {
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(630)))?;
            stream.set_write_timeout(Some(Duration::from_secs(30)))?;
            Ok(())
        })() {
            anyhow::bail!(
                "PhotoKit helper socket setup failed: {error}; {}",
                retain_worker_log(socket_dir, None)
            );
        }
        let mut client = Self {
            stream: BufReader::new(stream),
            socket_dir: Some(socket_dir),
            pending: None,
        };
        let reply = client
            .request(&serde_json::json!({"version":1,"operation":"probe",
            "batchID":"probe","witnessIdentifiers":witnesses}))
            .map_err(|error| error.context("PhotoKit startup probe failed"))?;
        if reply["state"] != "ready" {
            let failure = serde_json::to_vec(&reply)?;
            anyhow::bail!(
                "PhotoKit authorization/target probe failed; {}",
                client.retain_log(Some(&failure))
            );
        }
        Ok(client)
    }

    fn retain_log(&mut self, failure: Option<&[u8]>) -> String {
        self.socket_dir.take().map_or_else(
            || "private diagnostics unavailable".into(),
            |directory| retain_worker_log(directory, failure),
        )
    }

    pub(super) fn request(&mut self, request: &Value) -> anyhow::Result<Value> {
        self.begin_request(request)?;
        self.finish_request()
    }

    pub(super) fn begin_request(&mut self, request: &Value) -> anyhow::Result<()> {
        anyhow::ensure!(self.pending.is_none(), "PhotoKit request already in flight");
        let mut bytes = serde_json::to_vec(request)?;
        anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "PhotoKit request too large");
        bytes.push(b'\n');
        let timing = crate::image::photos_import_metrics::timer("native_request_wall");
        if let Err(error) = self.stream.get_mut().write_all(&bytes) {
            anyhow::bail!(
                "PhotoKit request write failed: {error}; {}; reconcile durable journals",
                self.retain_log(None)
            );
        }
        self.pending = Some((request.clone(), timing));
        Ok(())
    }

    pub(super) fn finish_request(&mut self) -> anyhow::Result<Value> {
        let (request, _timing) = self
            .pending
            .take()
            .ok_or_else(|| anyhow::anyhow!("no PhotoKit request in flight"))?;
        let mut line = Vec::new();
        if let Err(error) = self
            .stream
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)
        {
            anyhow::bail!(
                "PhotoKit reply read failed: {error}; {}; reconcile durable journals",
                self.retain_log(None)
            );
        }
        if line.last() != Some(&b'\n') {
            anyhow::bail!(
                "PhotoKit reply ended before completion: {}; reconcile durable journals",
                self.retain_log(None)
            );
        }
        if line.len() > 8 * 1024 * 1024 {
            anyhow::bail!(
                "PhotoKit reply oversized; {}; reconcile durable journals",
                self.retain_log(None)
            );
        }
        let reply: Value = serde_json::from_slice(&line).map_err(|error| {
            anyhow::anyhow!(
                "PhotoKit reply invalid at line {}, column {}; {}",
                error.line(),
                error.column(),
                self.retain_log(None)
            )
        })?;
        if reply["version"] != 1 || reply["batchID"] != request["batchID"] {
            anyhow::bail!(
                "PhotoKit reply identity mismatch; {}; reconcile durable journals",
                self.retain_log(None)
            );
        }
        if reply["state"] == "error" {
            anyhow::bail!(
                "PhotoKit request failed; {}; reconcile durable journals before retrying an import",
                self.retain_log(Some(&line))
            );
        }
        Ok(reply)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // EOF stops the idle helper. An in-flight transaction retains its own OS
        // writer lock until completion/deadline, so a new parent cannot overlap it.
        let _ = self.stream.get_mut().shutdown(Shutdown::Both);
    }
}

/// Bind replies by source entry ID, never `PhotoKit` enumeration order.
pub(super) fn committed_pairs(
    request: &Value,
    record: &Value,
) -> anyhow::Result<Vec<(String, String)>> {
    super::fast_img::retry_checkpoint::native_committed_pairs(request, record)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_failure_keeps_private_diagnostics_out_of_reported_error() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let socket_dir = tempfile::Builder::new().tempdir_in(root.path())?;
        let directory = socket_dir.path().to_owned();
        let filename = "/Synthetic/Private Photo.jxl";
        let uuid = "DEADBEAF-0000-4000-8000-000000000001";
        std::fs::write(directory.join("worker.log"), filename)?;
        let (stream, mut peer) = UnixStream::pair()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        peer.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut client = Client {
            stream: BufReader::new(stream),
            socket_dir: Some(socket_dir),
            pending: None,
        };
        client.begin_request(&serde_json::json!({"version": 1, "batchID": "batch"}))?;
        let worker = std::thread::spawn(move || -> anyhow::Result<()> {
            let reply = serde_json::json!({"version": 1, "batchID": "batch", "state": "error",
                "error": format!("{filename} {uuid} {}", "x".repeat(128 * 1024))});
            writeln!(peer, "{reply}")?;
            Ok(())
        });
        let message = client.finish_request().unwrap_err().to_string();
        worker.join().expect("mock helper panicked")?;
        assert!(message.contains("PhotoKit request failed"));
        assert!(message.contains(&directory.display().to_string()));
        assert!(!message.contains(filename) && !message.contains(uuid));
        assert_eq!(
            std::fs::read_to_string(directory.join("worker.log"))?,
            filename
        );
        let path = directory.join("failure.log");
        let failure = std::fs::read_to_string(&path)?;
        assert!(failure.contains(filename) && failure.contains(uuid));
        assert_eq!(failure.len(), 64 * 1024);
        assert_eq!(std::fs::metadata(path)?.permissions().mode() & 0o077, 0);
        Ok(())
    }

    #[test]
    fn journal_requires_exact_request_and_unique_complete_identity_mapping() {
        let request = serde_json::json!({"version":1,"batchID":"b","assets":[{"entryID":"a"},{"entryID":"b"}]});
        let record = serde_json::json!({"version":1,"batchID":"b","state":"committed","request":request,
            "identities":[{"entryID":"b","localIdentifier":"id-b"},{"entryID":"a","localIdentifier":"id-a"}]});
        assert_eq!(
            committed_pairs(&request, &record).unwrap(),
            vec![("a".into(), "id-a".into()), ("b".into(), "id-b".into())]
        );
        for alteration in ["missing", "duplicate", "wrong-source", "submitted"] {
            let mut changed = record.clone();
            match alteration {
                "missing" => {
                    changed["identities"].as_array_mut().unwrap().pop();
                }
                "duplicate" => {
                    changed["identities"][0]["localIdentifier"] = Value::String("id-a".into());
                }
                "wrong-source" => {
                    changed["request"]["assets"][0]["entryID"] = Value::String("other".into());
                }
                _ => changed["state"] = Value::String("submitted".into()),
            }
            assert!(committed_pairs(&request, &changed).is_err(), "{alteration}");
        }
    }

    #[test]
    fn ten_thousand_assets_reuse_one_transport_and_reject_wrong_reply() -> anyhow::Result<()> {
        use crate::image::photos_import_schedule::{Schedule, Step};
        let (stream, peer) = UnixStream::pair()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        peer.set_read_timeout(Some(Duration::from_secs(5)))?;
        let server = std::thread::spawn(move || -> anyhow::Result<usize> {
            let mut peer = BufReader::new(peer);
            let mut calls = 0;
            loop {
                let mut line = String::new();
                if peer.read_line(&mut line)? == 0 {
                    return Ok(calls);
                }
                let request: Value = serde_json::from_str(&line)?;
                let assets = request["assets"].as_array().unwrap();
                let identities = assets
                    .iter()
                    .rev()
                    .map(|asset| {
                        serde_json::json!({"entryID": asset["entryID"],
                        "localIdentifier": format!("id-{}", asset["entryID"].as_str().unwrap())})
                    })
                    .collect::<Vec<_>>();
                let reply = serde_json::json!({"version": 1,
                    "batchID": if assets.is_empty() { serde_json::json!("wrong") } else { request["batchID"].clone() },
                    "state": "committed", "request": request, "identities": identities});
                writeln!(peer.get_mut(), "{reply}")?;
                if assets.is_empty() {
                    return Ok(calls);
                }
                calls += 1;
            }
        });
        let mut client = Client {
            stream: BufReader::new(stream),
            socket_dir: Some(tempfile::tempdir()?),
            pending: None,
        };
        let policy = crate::runtime_config::PhotosPolicy {
            native_batch_size: 250,
            verification_batch_size: 128,
            ..Default::default()
        };
        let mut schedule = Schedule::new(10_003, &policy)?;
        let mut batch = 0;
        let mut verified = 0;
        while let Some(step) = schedule.next() {
            match step {
                Step::Import(range) => {
                    let assets = range
                        .map(|index| serde_json::json!({"entryID": format!("{index:05}")}))
                        .collect::<Vec<_>>();
                    let request =
                        serde_json::json!({"version": 1, "batchID": batch, "assets": assets});
                    let reply = client.request(&request)?;
                    let pairs = committed_pairs(&request, &reply)?;
                    assert_eq!(pairs.len(), assets.len());
                    for (entry, id) in pairs {
                        assert_eq!(id, format!("id-{entry}"));
                    }
                    batch += 1;
                }
                Step::Verify(range) => verified += range.len(),
            }
        }
        assert_eq!(verified, 10_003);
        assert_eq!(batch, 41);
        assert!(
            client
                .request(&serde_json::json!({"version": 1, "batchID": "last", "assets": []}))
                .is_err()
        );
        drop(client);
        assert_eq!(server.join().expect("mock helper panicked")?, 41);
        Ok(())
    }

    #[test]
    fn helper_import_remains_pending_during_verification_and_rejects_second_write()
    -> anyhow::Result<()> {
        use std::sync::mpsc;
        let (stream, peer) = UnixStream::pair()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let (received_tx, received_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || -> anyhow::Result<()> {
            let mut peer = BufReader::new(peer);
            let mut line = String::new();
            peer.read_line(&mut line)?;
            let request: Value = serde_json::from_str(&line)?;
            received_tx.send(())?;
            release_rx.recv()?;
            let reply = serde_json::json!({"version":1,"batchID":request["batchID"],
                "state":"committed","request":request,
                "identities":[{"entryID":"next","localIdentifier":"id-next"}]});
            writeln!(peer.get_mut(), "{reply}")?;
            Ok(())
        });
        let mut client = Client {
            stream: BufReader::new(stream),
            socket_dir: Some(tempfile::tempdir()?),
            pending: None,
        };
        let request = serde_json::json!({"version":1,"batchID":"next-batch",
            "assets":[{"entryID":"next"}]});
        client.begin_request(&request)?;
        received_rx.recv_timeout(Duration::from_secs(5))?;
        // This is the verification/checkpoint interval: the worker has received
        // the import but its response is still blocked by the mock transport.
        assert!(client.pending.is_some());
        assert!(client.begin_request(&request).is_err());
        release_tx.send(())?;
        let reply = client.finish_request()?;
        assert_eq!(
            committed_pairs(&request, &reply)?,
            vec![("next".into(), "id-next".into())]
        );
        assert!(client.pending.is_none());
        server.join().expect("mock helper panicked")?;
        Ok(())
    }
}

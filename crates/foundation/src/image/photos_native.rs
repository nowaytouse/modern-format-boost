//! Persistent, single-writer `PhotoKit` transport. No retry after an import intent.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;

pub(super) struct Client {
    stream: BufReader<UnixStream>,
    socket_dir: Option<tempfile::TempDir>,
    pending: Option<(Value, crate::image::photos_import_metrics::Timer)>,
}

fn retain_worker_log(socket_dir: tempfile::TempDir) -> String {
    let path = socket_dir.keep().join("worker.log");
    match std::fs::read_to_string(&path) {
        Ok(log) => format!("worker log retained at {}: {log}", path.display()),
        Err(error) => format!(
            "worker log retained at {} but unreadable: {error}",
            path.display()
        ),
    }
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
        let launch = std::process::Command::new("/usr/bin/open")
            .args(["-n", "-a"])
            .arg(app)
            .arg("--stderr")
            .arg(socket_dir.path().join("worker.log"))
            .arg("--args")
            .arg("--socket")
            .arg(&socket)
            .arg("--lock")
            .arg(state.join("writer.lock"))
            .output();
        let launch = match launch {
            Ok(launch) => launch,
            Err(error) => anyhow::bail!(
                "PhotoKit helper launch failed: {error}; {}",
                retain_worker_log(socket_dir)
            ),
        };
        anyhow::ensure!(
            launch.status.success(),
            "PhotoKit helper launch failed: {}; {}",
            String::from_utf8_lossy(&launch.stderr),
            retain_worker_log(socket_dir)
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
                    retain_worker_log(socket_dir)
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
                retain_worker_log(socket_dir)
            );
        }
        let mut client = Self {
            stream: BufReader::new(stream),
            socket_dir: Some(socket_dir),
            pending: None,
        };
        let reply = client.request(&serde_json::json!({"version":1,"operation":"probe",
            "batchID":"probe","witnessIdentifiers":witnesses}));
        let reply = match reply {
            Ok(reply) => reply,
            Err(error) => anyhow::bail!(
                "PhotoKit startup probe failed: {error:#}; {}",
                client.retain_log()
            ),
        };
        if reply["state"] != "ready" {
            anyhow::bail!(
                "PhotoKit authorization/target probe failed: {reply}; {}",
                client.retain_log()
            );
        }
        Ok(client)
    }

    fn retain_log(&mut self) -> String {
        self.socket_dir
            .take()
            .map_or_else(|| "worker log unavailable".into(), retain_worker_log)
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
        self.stream.get_mut().write_all(&bytes)?;
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
                self.retain_log()
            );
        }
        if line.last() != Some(&b'\n') {
            anyhow::bail!(
                "PhotoKit reply ended before completion: {}; reconcile durable journals",
                self.retain_log()
            );
        }
        if line.len() > 8 * 1024 * 1024 {
            anyhow::bail!(
                "PhotoKit reply oversized; {}; reconcile durable journals",
                self.retain_log()
            );
        }
        let reply: Value = serde_json::from_slice(&line).map_err(|error| {
            anyhow::anyhow!("PhotoKit reply invalid: {error}; {}", self.retain_log())
        })?;
        if reply["version"] != 1 || reply["batchID"] != request["batchID"] {
            anyhow::bail!(
                "PhotoKit reply identity mismatch; {}; reconcile durable journals",
                self.retain_log()
            );
        }
        if reply["state"] == "error" {
            anyhow::bail!("PhotoKit request failed: {reply}; {}", self.retain_log());
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
    use std::collections::{BTreeMap, BTreeSet};
    anyhow::ensure!(
        record["version"] == 1
            && record["batchID"] == request["batchID"]
            && record["request"] == *request,
        "PhotoKit journal belongs to another request"
    );
    anyhow::ensure!(
        matches!(
            record["state"].as_str(),
            Some("committed" | "identifier-known")
        ),
        "PhotoKit commit is uncertain without complete identifiers; original files retained"
    );
    let assets = request["assets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing requested assets"))?;
    let expected = assets
        .iter()
        .map(|asset| {
            asset["entryID"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("missing entry ID"))
        })
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    anyhow::ensure!(expected.len() == assets.len(), "duplicate task entry IDs");
    let mut pairs = BTreeMap::new();
    let mut seen_ids = BTreeSet::new();
    for identity in record["identities"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing durable asset identifiers"))?
    {
        let entry = identity["entryID"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing entry ID"))?;
        let id = identity["localIdentifier"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing asset identifier"))?;
        anyhow::ensure!(
            expected.contains(entry)
                && !id.is_empty()
                && seen_ids.insert(id.to_owned())
                && pairs.insert(entry.to_owned(), id.to_owned()).is_none(),
            "ambiguous PhotoKit identity mapping"
        );
    }
    anyhow::ensure!(
        pairs.len() == expected.len(),
        "partial PhotoKit identity mapping; do not reimport"
    );
    Ok(pairs.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
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

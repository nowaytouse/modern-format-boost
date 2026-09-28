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
    _socket_dir: tempfile::TempDir,
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
            .output()?;
        anyhow::ensure!(
            launch.status.success(),
            "PhotoKit helper launch failed: {}",
            String::from_utf8_lossy(&launch.stderr)
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
                    std::fs::read_to_string(socket_dir.path().join("worker.log"))
                        .unwrap_or_default()
                ),
            }
        };
        // macOS accepts inherit the listening socket's nonblocking flag.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(630)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let mut client = Self {
            stream: BufReader::new(stream),
            _socket_dir: socket_dir,
        };
        let reply = client.request(&serde_json::json!({"version":1,"operation":"probe",
            "batchID":"probe","witnessIdentifiers":witnesses}))?;
        anyhow::ensure!(
            reply["state"] == "ready",
            "PhotoKit authorization/target probe failed: {reply}"
        );
        Ok(client)
    }

    pub(super) fn request(&mut self, request: &Value) -> anyhow::Result<Value> {
        let mut bytes = serde_json::to_vec(request)?;
        anyhow::ensure!(bytes.len() <= 4 * 1024 * 1024, "PhotoKit request too large");
        bytes.push(b'\n');
        self.stream.get_mut().write_all(&bytes)?;
        let mut line = Vec::new();
        self.stream
            .by_ref()
            .take(8 * 1024 * 1024 + 1)
            .read_until(b'\n', &mut line)?;
        anyhow::ensure!(
            line.len() <= 8 * 1024 * 1024 && line.last() == Some(&b'\n'),
            "PhotoKit reply missing/oversized; retain all inputs and reconcile durable journals"
        );
        let reply: Value = serde_json::from_slice(&line)?;
        anyhow::ensure!(
            reply["version"] == 1 && reply["batchID"] == request["batchID"],
            "PhotoKit reply identity mismatch"
        );
        anyhow::ensure!(
            reply["state"] != "error",
            "PhotoKit request failed: {reply}"
        );
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
                    changed["identities"][0]["localIdentifier"] = Value::String("id-a".into())
                }
                "wrong-source" => {
                    changed["request"]["assets"][0]["entryID"] = Value::String("other".into())
                }
                _ => changed["state"] = Value::String("submitted".into()),
            }
            assert!(committed_pairs(&request, &changed).is_err(), "{alteration}");
        }
    }
}

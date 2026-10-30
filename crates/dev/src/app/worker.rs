//! Shared worker invocation, execution, and launch/configuration observation.

use crate::infra::hardening::delegated_exit_code;
use crate::infra::history_store::HistoryOutputWriter;
use crate::infra::log_paths::append_jsonl_audit_record;
use crate::infra::process_stream::{
    ChildProcessObservation, ProcessorStats, stream_process_with_pty_observed,
};
use crate::infra::signal_handlers::set_child_active;
use anyhow::{Context, Result, bail};
use foundation::infra::runtime_config::RuntimeConfig;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub const INVOCATION_MAX_ARGUMENTS: usize = 4096;
pub const INVOCATION_MAX_ARGUMENT_BYTES: usize = 256 * 1024;
static INVOCATION_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The session facilities needed while running a worker, without coupling the
/// executor to a particular launcher's session implementation.
pub trait WorkerSession {
    fn stamp(&self) -> &str;
    fn run_id(&self) -> Option<&str> {
        None
    }
    fn log_dir(&self) -> &Path;
    fn session_audit(&self) -> &Path;
    fn tool_snapshots(&self) -> &ToolSnapshotBindings;
    fn append_history_event(&self, name: &str, payload: serde_json::Value) -> Result<()>;
    fn append_session_line(&self, event: &str) -> Result<()>;
    fn append_verbose_line(&self, event: &str) -> Result<()>;
}

#[derive(Default)]
pub struct ToolSnapshotBindings {
    pipelines: std::sync::Mutex<BTreeMap<String, PipelineToolSnapshot>>,
}

#[derive(Default)]
struct PipelineToolSnapshot {
    snapshot: Option<foundation::tool_snapshot::Snapshot>,
    configured_paths: Option<BTreeMap<String, PathBuf>>,
    blocked: Option<String>,
}

impl ToolSnapshotBindings {
    fn manifest(&self, pipeline: &str) -> Result<Option<String>> {
        let pipelines = self
            .pipelines
            .lock()
            .map_err(|_| anyhow::anyhow!("tool snapshot bindings lock poisoned"))?;
        let Some(state) = pipelines.get(pipeline) else {
            return Ok(None);
        };
        anyhow::ensure!(
            state.blocked.is_none(),
            "{pipeline} tool runtime blocked: {}",
            state.blocked.as_deref().unwrap_or("")
        );
        state
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .manifest_path
                    .to_str()
                    .map(str::to_owned)
                    .context("tool snapshot manifest path is not UTF-8")
            })
            .transpose()
    }

    fn observe_config(&self, pipeline: &str, paths: &BTreeMap<String, PathBuf>) -> Result<()> {
        let mut pipelines = self
            .pipelines
            .lock()
            .map_err(|_| anyhow::anyhow!("tool snapshot bindings lock poisoned"))?;
        let state = pipelines.entry(pipeline.to_owned()).or_default();
        let result = (|| {
            anyhow::ensure!(
                state.blocked.is_none(),
                "{pipeline} tool runtime is already blocked"
            );
            if let Some(previous) = &state.configured_paths {
                anyhow::ensure!(
                    previous == paths,
                    "explicit tool selections changed within the {pipeline} pipeline"
                );
            }
            if let Some(snapshot) = &state.snapshot {
                for (name, path) in paths {
                    if snapshot.tools.contains_key(name) {
                        snapshot.verify_selection(name, path)?;
                    }
                }
            }
            state.configured_paths = Some(paths.clone());
            Ok(())
        })();
        if let Err(error) = &result {
            state.blocked.get_or_insert_with(|| format!("{error:#}"));
        }
        result
    }

    fn observe(&self, pipeline: &str, payload: &str) -> Result<()> {
        let mut pipelines = self
            .pipelines
            .lock()
            .map_err(|_| anyhow::anyhow!("tool snapshot bindings lock poisoned"))?;
        let state = pipelines.entry(pipeline.to_owned()).or_default();
        let result = (|| {
            anyhow::ensure!(
                state.blocked.is_none(),
                "{pipeline} tool runtime is already blocked"
            );
            let event: foundation::tool_snapshot::SnapshotEvent =
                serde_json::from_str(payload).context("decode tool snapshot event")?;
            anyhow::ensure!(
                event.schema_version == 1
                    && event.pipeline == pipeline
                    && serde_json::from_str::<serde_json::Value>(payload)?
                        == serde_json::to_value(&event)?,
                "tool snapshot event schema or pipeline mismatch"
            );
            match event.state.as_str() {
                "preparing" => {
                    anyhow::ensure!(
                        event.identity.is_none()
                            && event.manifest_path.is_none()
                            && event.tool_names.is_empty()
                            && event.observed_tool_names.is_empty(),
                        "invalid preparing tool snapshot event"
                    );
                }
                "ready" if event.boundary == "persisted-external" => {
                    let path = event
                        .manifest_path
                        .context("ready tool snapshot has no manifest")?;
                    anyhow::ensure!(
                        path.is_absolute(),
                        "tool snapshot manifest path is not absolute"
                    );
                    let snapshot = foundation::tool_snapshot::load_and_verify(&path)?;
                    anyhow::ensure!(
                        event.identity.as_deref() == Some(snapshot.identity.as_str())
                            && event.tool_names
                                == snapshot.tools.keys().cloned().collect::<Vec<_>>(),
                        "tool snapshot event disagrees with verified manifest"
                    );
                    if let Some(previous) = &state.snapshot {
                        anyhow::ensure!(
                            previous.identity == snapshot.identity
                                && previous.manifest_path == snapshot.manifest_path,
                            "tool snapshot changed within the {pipeline} pipeline"
                        );
                    }
                    if let Some(paths) = &state.configured_paths {
                        for (name, path) in paths {
                            if snapshot.tools.contains_key(name) {
                                snapshot.verify_selection(name, path)?;
                            }
                        }
                    }
                    state.snapshot = Some(snapshot);
                }
                "ready" if event.boundary == "live-observed" => {
                    anyhow::ensure!(
                        state.snapshot.is_none()
                            && event.identity.is_none()
                            && event.manifest_path.is_none()
                            && event.tool_names.is_empty(),
                        "tool snapshot cannot be downgraded to live selection"
                    );
                }
                "blocked" => anyhow::bail!(
                    "{pipeline} tool runtime blocked ({}): {}",
                    event
                        .cause_code
                        .context("blocked tool snapshot has no cause code")?,
                    event.message
                ),
                _ => anyhow::bail!("unsupported tool snapshot state or boundary"),
            }
            Ok(())
        })();
        if let Err(error) = &result {
            state.blocked.get_or_insert_with(|| format!("{error:#}"));
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env_overrides: Vec<(String, Option<String>)>,
    pub sources: Vec<foundation::batch::DiscoveredSource>,
}

impl LaunchCommand {
    #[must_use]
    pub fn with_sources(mut self, sources: Vec<foundation::batch::DiscoveredSource>) -> Self {
        self.sources = sources;
        self
    }
}

#[derive(Serialize)]
pub struct BinaryObservation {
    pub scope: &'static str,
    pub bytes: Option<u64>,
    pub blake3: Option<String>,
    pub diagnostic: Option<String>,
}

impl BinaryObservation {
    pub fn observe(program: Option<&Path>) -> Self {
        let observed = (|| -> Result<(u64, String)> {
            let program = program
                .context("executable path was not resolved from the child launch environment")?;
            let mut file =
                fs::File::open(program).context("open worker binary for launch observation")?;
            let before = file.metadata()?;
            anyhow::ensure!(
                before.is_file(),
                "worker launch observation is not a regular file"
            );
            let mut hasher = blake3::Hasher::new();
            let mut buffer = [0_u8; 64 * 1024];
            let mut bytes = 0_u64;
            loop {
                let length = file.read(&mut buffer)?;
                if length == 0 {
                    break;
                }
                hasher.update(&buffer[..length]);
                bytes = bytes
                    .checked_add(u64::try_from(length)?)
                    .context("worker binary observation length overflow")?;
            }
            let after = file.metadata()?;
            anyhow::ensure!(
                bytes == before.len()
                    && before.len() == after.len()
                    && before.modified()? == after.modified()?,
                "worker binary changed during launch observation"
            );
            Ok((bytes, hasher.finalize().to_hex().to_string()))
        })();
        match observed {
            Ok((bytes, blake3)) => Self {
                scope: "observed_before_launch",
                bytes: Some(bytes),
                blake3: Some(blake3),
                diagnostic: None,
            },
            Err(error) => Self {
                scope: "observed_before_launch",
                bytes: None,
                blake3: None,
                diagnostic: Some(format!("{error:#}")),
            },
        }
    }
}

#[derive(Serialize)]
pub struct LaunchInvocation<'a> {
    pub schema_version: u8,
    pub invocation_id: String,
    pub run_id: Option<String>,
    pub pipeline: &'static str,
    pub state: &'static str,
    pub program_requested: &'a str,
    pub program_path: Option<PathBuf>,
    pub arguments: &'a [String],
    pub binary: BinaryObservation,
}

#[derive(Serialize)]
struct WorkerExecution<'a> {
    schema_version: u8,
    scope: &'static str,
    invocation_id: &'a str,
    run_id: Option<&'a str>,
    pipeline: &'static str,
    state: &'static str,
    pid: u32,
    exit_code: Option<i32>,
    signal: Option<i32>,
}

impl<'a> WorkerExecution<'a> {
    fn observed(
        invocation: &'a LaunchInvocation<'_>,
        observation: ChildProcessObservation,
    ) -> Self {
        let (state, pid, status) = match observation {
            ChildProcessObservation::Spawned { pid } => ("spawned", pid, None),
            ChildProcessObservation::Exited { pid, status } => ("exited", pid, Some(status)),
        };
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            status.and_then(|value| value.signal())
        };
        #[cfg(not(unix))]
        let signal = None;
        Self {
            schema_version: 1,
            scope: "os_child_process",
            invocation_id: &invocation.invocation_id,
            run_id: invocation.run_id.as_deref(),
            pipeline: invocation.pipeline,
            state,
            pid,
            exit_code: status.and_then(|value| value.code()),
            signal,
        }
    }
}

#[derive(Serialize)]
struct WorkerOutputStorage<'a> {
    schema_version: u8,
    scope: &'static str,
    invocation_id: &'a str,
    run_id: Option<&'a str>,
    pipeline: &'static str,
    state: &'static str,
    observed_records: Option<u64>,
    committed_records: u64,
    committed_utf8_bytes: u64,
    capture_complete: bool,
    diagnostic: Option<String>,
}

impl<'a> WorkerOutputStorage<'a> {
    fn observed(
        invocation: &'a LaunchInvocation<'_>,
        observed_records: Option<u64>,
        committed: (u64, u64),
        capture_complete: bool,
        error: Option<&anyhow::Error>,
    ) -> Self {
        Self {
            schema_version: 1,
            scope: "captured_worker_output",
            invocation_id: &invocation.invocation_id,
            run_id: invocation.run_id.as_deref(),
            pipeline: invocation.pipeline,
            state: if error.is_some() {
                "failed"
            } else {
                "committed"
            },
            observed_records,
            committed_records: committed.0,
            committed_utf8_bytes: committed.1,
            capture_complete,
            diagnostic: error.map(|error| format!("{error:#}").chars().take(4096).collect()),
        }
    }
}

#[derive(Debug)]
pub struct CollectedChildError {
    pub stats: ProcessorStats,
    pub cause: anyhow::Error,
}

impl std::fmt::Display for CollectedChildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.cause)
    }
}

impl std::error::Error for CollectedChildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChildConfigMarker {
    pub schema_version: u32,
    pub pipeline: String,
    pub package_version: String,
    pub config: RuntimeConfig,
    pub sources: BTreeMap<String, String>,
}

impl ChildConfigMarker {
    pub fn parse(payload: &str, expected_pipeline: &str) -> Result<Self> {
        anyhow::ensure!(
            matches!(expected_pipeline, "img" | "vid"),
            "unexpected config marker from non-media child"
        );
        let raw: serde_json::Value =
            serde_json::from_str(payload).context("parse child config marker")?;
        let marker: Self =
            serde_json::from_value(raw.clone()).context("decode child config marker")?;
        anyhow::ensure!(
            marker.schema_version == 1,
            "unsupported child config marker version"
        );
        anyhow::ensure!(
            marker.pipeline == expected_pipeline,
            "child config marker pipeline mismatch: expected {expected_pipeline}"
        );
        anyhow::ensure!(
            !marker.package_version.trim().is_empty(),
            "child config marker package_version is empty"
        );
        marker.config.validate()?;
        anyhow::ensure!(
            raw.get("config") == Some(&serde_json::to_value(&marker.config)?),
            "child config marker omits or changes resolved configuration fields"
        );
        let mut leaf_paths = BTreeSet::new();
        collect_config_leaf_paths(&serde_json::to_value(&marker.config)?, "", &mut leaf_paths);
        anyhow::ensure!(
            marker.sources.keys().all(|key| !key.is_empty())
                && marker.sources.values().all(|source| !source.is_empty())
                && marker.sources.keys().cloned().collect::<BTreeSet<_>>() == leaf_paths,
            "child config marker sources do not cover the resolved configuration"
        );
        Ok(marker)
    }
}

/// Collect dotted leaf paths from a resolved configuration snapshot.
pub fn collect_config_leaf_paths(
    value: &serde_json::Value,
    prefix: &str,
    paths: &mut BTreeSet<String>,
) {
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            collect_config_leaf_paths(child, &path, paths);
        }
    } else {
        paths.insert(prefix.to_owned());
    }
}

impl LaunchCommand {
    pub fn invocation(&self, session: &dyn WorkerSession) -> Result<LaunchInvocation<'_>> {
        anyhow::ensure!(
            session.run_id().is_none_or(|id| !id.is_empty()
                && id.len() <= 256
                && id.bytes().all(|byte| byte.is_ascii_graphic())),
            "worker run identity is invalid"
        );
        let program = self
            .program
            .to_str()
            .context("worker program is not UTF-8")?;
        anyhow::ensure!(
            !program.is_empty() && !program.contains('\0') && program.len() <= 32 * 1024,
            "worker program is empty, invalid, or exceeds the history limit"
        );
        anyhow::ensure!(
            self.args.len() <= INVOCATION_MAX_ARGUMENTS,
            "worker argument count exceeds the history limit"
        );
        let argument_bytes = self.args.iter().try_fold(0_usize, |bytes, argument| {
            anyhow::ensure!(!argument.contains('\0'), "worker argument contains a NUL");
            bytes
                .checked_add(argument.len())
                .context("worker argument length overflow")
        })?;
        anyhow::ensure!(
            argument_bytes <= INVOCATION_MAX_ARGUMENT_BYTES,
            "worker arguments exceed the history byte limit"
        );
        let (program_path, binary) = match self.observed_program_path() {
            Ok(path) => {
                let binary = BinaryObservation::observe(Some(&path));
                (Some(path), binary)
            }
            Err(error) => (
                None,
                BinaryObservation {
                    scope: "observed_before_launch",
                    bytes: None,
                    blake3: None,
                    diagnostic: Some(format!("{error:#}")),
                },
            ),
        };
        let sequence = INVOCATION_SEQUENCE
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| anyhow::anyhow!("worker invocation sequence exhausted"))?;
        Ok(LaunchInvocation {
            schema_version: 1,
            invocation_id: format!(
                "{}-{}-{sequence}",
                std::process::id(),
                blake3::hash(session.stamp().as_bytes()).to_hex()
            ),
            run_id: session.run_id().map(str::to_owned),
            pipeline: match self.pipeline_label() {
                "IMG" => "img",
                "VID" => "vid",
                _ => "cmd",
            },
            state: "launch_requested",
            program_requested: program,
            program_path,
            arguments: &self.args,
            binary,
        })
    }

    fn observed_program_path(&self) -> Result<PathBuf> {
        if self.program.components().count() > 1 || self.program.is_absolute() {
            return std::path::absolute(&self.program)
                .context("resolve requested worker executable absolute path");
        }
        #[cfg(unix)]
        {
            let path = self
                .env_overrides
                .iter()
                .rev()
                .find(|(key, _)| key == "PATH")
                .map_or_else(
                    || std::env::var_os("PATH"),
                    |(_, value)| value.as_ref().map(std::ffi::OsString::from),
                )
                .context("PATH is absent from the child launch environment")?;
            for directory in std::env::split_paths(&path) {
                let candidate = directory.join(&self.program);
                let candidate = std::path::absolute(candidate)
                    .context("resolve child PATH executable candidate")?;
                let encoded = std::ffi::CString::new(candidate.as_os_str().as_encoded_bytes())
                    .context("child PATH executable candidate contains a NUL")?;
                // Observe the exact PATH candidate; execution retains the requested program.
                if candidate.is_file() && unsafe { libc::access(encoded.as_ptr(), libc::X_OK) } == 0
                {
                    return Ok(candidate);
                }
            }
        }
        anyhow::bail!("requested executable was not resolved from the child launch environment")
    }

    fn verify_sources_unchanged(&self, invocation_error: Option<&anyhow::Error>) -> Result<()> {
        for source in &self.sources {
            let result = source.verify_unchanged().with_context(|| {
                format!(
                    "worker source changed before launch: {}",
                    source.path().display()
                )
            });
            if let Err(source_error) = result {
                return Err(match invocation_error {
                    Some(invocation_error) => source_error.context(format!(
                        "worker invocation history write failed: {invocation_error:#}"
                    )),
                    None => source_error,
                });
            }
        }
        Ok(())
    }

    pub fn from_argv(argv: Vec<String>) -> Result<Self> {
        let mut iter = argv.into_iter();
        let Some(program) = iter.next() else {
            bail!("internal launcher error: empty command argv");
        };
        Ok(Self {
            program: PathBuf::from(program),
            args: iter.collect(),
            env_overrides: Vec::new(),
            sources: Vec::new(),
        })
    }

    pub fn with_error_mode(mut self, mode: Option<foundation::BatchErrorMode>) -> Self {
        if let Some(mode) = mode {
            self.args
                .extend(["--error-mode".to_owned(), mode.as_str().to_owned()]);
        }
        self
    }

    pub fn display(&self) -> String {
        let mut parts = Vec::with_capacity(self.args.len() + 1);
        parts.push(self.program.display().to_string());
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }

    pub fn pipeline_label(&self) -> &'static str {
        match self.program.file_name().and_then(|name| name.to_str()) {
            Some("img") => "IMG",
            Some("vid") => "VID",
            _ => "CMD",
        }
    }

    pub fn run(&self, dry_run: bool) -> Result<()> {
        self.run_with_session(dry_run, None).map(|_| ())
    }

    pub fn run_with_session(
        &self,
        dry_run: bool,
        session: Option<&dyn WorkerSession>,
    ) -> Result<ProcessorStats> {
        self.run_collecting(dry_run, session, true)
    }

    pub fn run_collecting(
        &self,
        dry_run: bool,
        session: Option<&dyn WorkerSession>,
        bail_on_failure: bool,
    ) -> Result<ProcessorStats> {
        println!("+ {}", self.display());
        if dry_run {
            return Ok(ProcessorStats::default());
        }
        let expected_pipeline = match self.pipeline_label() {
            "IMG" => "img",
            "VID" => "vid",
            _ => "",
        };
        let mut launch_env = self.env_overrides.clone();
        if let Some(session) = session.filter(|_| !expected_pipeline.is_empty()) {
            let bound = session.tool_snapshots().manifest(expected_pipeline)?;
            if let Some(bound) = &bound {
                for (_, value) in launch_env
                    .iter()
                    .filter(|(name, _)| name == foundation::tool_snapshot::MANIFEST_ENV)
                {
                    anyhow::ensure!(
                        value.as_ref() == Some(bound),
                        "worker tool snapshot override conflicts with the active pipeline"
                    );
                }
            }
            if bound.is_some()
                || !launch_env
                    .iter()
                    .any(|(name, _)| name == foundation::tool_snapshot::MANIFEST_ENV)
            {
                launch_env.retain(|(name, _)| name != foundation::tool_snapshot::MANIFEST_ENV);
                launch_env.push((foundation::tool_snapshot::MANIFEST_ENV.into(), bound));
            }
        }
        foundation::batch_control::checkpoint()?;
        if self.program.components().count() > 1 && !self.program.is_file() {
            bail!(
                "Rust CLI binary missing: {}. Build it with `cargo build -p {}` first.",
                self.program.display(),
                self.program
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("dev"))
                    .to_string_lossy()
            );
        }
        if let Some(session) = session {
            let event = format!(
                "{}_PIPELINE_LAUNCH_REQUEST cmd={}",
                self.pipeline_label(),
                self.display()
            );
            append_jsonl_audit_record(session.session_audit(), &event)?;
            session.append_session_line(&event)?;
            session.append_verbose_line(&event)?;
        }

        let invocation = session
            .map(|session| self.invocation(session))
            .transpose()?;
        let invocation_id = invocation
            .as_ref()
            .map(|invocation| invocation.invocation_id.as_str());
        let invocation_error = match (session, invocation.as_ref()) {
            (Some(session), Some(invocation)) => session
                .append_history_event("MFB_HISTORY_INVOCATION", serde_json::to_value(invocation)?)
                .context("worker invocation history write failed")
                .err(),
            _ => None,
        };

        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        for (name, value) in &launch_env {
            if let Some(value) = value {
                cmd.env(name, value);
            } else {
                cmd.env_remove(name);
            }
        }
        cmd.env_remove("MFB_RUN_ID");
        cmd.env_remove("MFB_INVOCATION_ID");
        cmd.env("COLUMNS", "223");
        cmd.env("LINES", "45");
        if let Some(config) = foundation::runtime_config::active() {
            let mode = clap::ValueEnum::to_possible_value(&config.performance.mode)
                .context("performance mode has no CLI value")?;
            cmd.env(foundation::constants::ENV_MFB_PERF_TIER, mode.get_name());
        }
        let mut collected_error = None;
        let mut child_status: Option<ExitStatus> = None;
        let stats = if let Some(session) = session {
            let mut output = HistoryOutputWriter::open(session.log_dir(), session.stamp())?;
            let heartbeat = session.session_audit().to_path_buf();
            let mut last_heartbeat = Instant::now();
            let pipeline_label = self.pipeline_label().to_string();
            let expected_config_pipeline = expected_pipeline;
            let pipeline_started_at = Instant::now();
            let argv = self.argv();
            let mut env_overrides = launch_env
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_deref()))
                .collect::<Vec<_>>();
            env_overrides.push(("MFB_SESSION_ID", Some(session.stamp())));
            env_overrides.push((
                "MFB_RUN_ID",
                invocation
                    .as_ref()
                    .and_then(|value| value.run_id.as_deref()),
            ));
            env_overrides.push(("MFB_INVOCATION_ID", invocation_id));
            self.verify_sources_unchanged(invocation_error.as_ref())?;
            set_child_active(true);
            let mut session_log_error = None;
            let mut observed_records = Some(0_u64);
            let mut config_marker_error = None;
            let mut snapshot_marker_error = None;
            let mut execution_marker_error = None;
            let mut seen_config_marker = false;
            let result = stream_process_with_pty_observed(
                &argv,
                None,
                &env_overrides,
                |line| {
                    if let Some(payload) = line.strip_prefix("MFB_HISTORY_CONFIG=") {
                        if config_marker_error.is_some() {
                            return;
                        }
                        if seen_config_marker {
                            config_marker_error = Some(anyhow::anyhow!(
                                "duplicate child config marker in one invocation"
                            ));
                        } else {
                            seen_config_marker = true;
                            if let Err(error) =
                                ChildConfigMarker::parse(payload, expected_config_pipeline)
                                    .and_then(|marker| {
                                        session.tool_snapshots().observe_config(
                                            expected_config_pipeline,
                                            &marker.config.tools.paths,
                                        )?;
                                        let mut marker = serde_json::to_value(marker)?;
                                        marker["invocation_id"] =
                                            serde_json::to_value(invocation_id)?;
                                        marker["run_id"] = serde_json::to_value(session.run_id())?;
                                        session.append_history_event("MFB_HISTORY_CONFIG", marker)
                                    })
                            {
                                config_marker_error = Some(error);
                            }
                        }
                        return;
                    }
                    if !expected_pipeline.is_empty()
                        && snapshot_marker_error.is_none()
                        && let Some(payload) = line.strip_prefix("MFB_TOOL_SNAPSHOT=")
                        && let Err(error) =
                            session.tool_snapshots().observe(expected_pipeline, payload)
                    {
                        snapshot_marker_error =
                            Some(error.context("worker tool snapshot admission failed"));
                    }
                    observed_records = observed_records.and_then(|count| count.checked_add(1));
                    if observed_records.is_none() && session_log_error.is_none() {
                        session_log_error = Some(anyhow::anyhow!(
                            "captured worker output record count overflow"
                        ));
                    }
                    if !line.starts_with("MFB_CONVERTED_BYTES=") {
                        println!("{line}");
                        let _ = io::stdout().flush();
                    }
                    if session_log_error.is_none()
                        && let Err(error) = output.push_line(&pipeline_label, line)
                    {
                        session_log_error = Some(error);
                    }
                },
                || {
                    if last_heartbeat.elapsed() >= std::time::Duration::from_mins(1) {
                        let event = format!(
                            "SESSION_HEARTBEAT pipeline={pipeline_label} elapsed_secs={}",
                            pipeline_started_at.elapsed().as_secs()
                        );
                        let _ = append_jsonl_audit_record(&heartbeat, &event);
                        last_heartbeat = Instant::now();
                    }
                },
                |observation| {
                    if let ChildProcessObservation::Exited { status, .. } = observation {
                        child_status = Some(status);
                    }
                    if let Some(invocation) = invocation.as_ref() {
                        let result = serde_json::to_value(WorkerExecution::observed(
                            invocation,
                            observation,
                        ))
                        .context("serialize worker execution observation")
                        .and_then(|payload| {
                            let marker = serde_json::to_string(&payload)
                                .context("serialize live worker execution observation")?;
                            session
                                .append_history_event("MFB_HISTORY_WORKER_EXECUTION", payload)?;
                            let mut stdout = io::stdout().lock();
                            writeln!(stdout, "MFB_HISTORY_WORKER_EXECUTION={marker}")
                                .context("write live worker execution observation")?;
                            stdout
                                .flush()
                                .context("flush live worker execution observation")
                        })
                        .context("worker execution history write failed");
                        if let Err(error) = result
                            && execution_marker_error.is_none()
                        {
                            execution_marker_error = Some(error);
                        }
                    }
                },
            );
            set_child_active(false);
            if let Err(error) = output.flush()
                && session_log_error.is_none()
            {
                session_log_error = Some(error);
            }
            let committed = output.committed_totals();
            if session_log_error.is_none() && observed_records != Some(committed.0) {
                session_log_error = Some(anyhow::anyhow!(
                    "captured worker output records were not all committed"
                ));
            }
            let storage_receipt_error = invocation.as_ref().and_then(|invocation| {
                serde_json::to_value(WorkerOutputStorage::observed(
                    invocation,
                    observed_records,
                    committed,
                    result.is_ok(),
                    session_log_error.as_ref(),
                ))
                .context("serialize worker output storage receipt")
                .and_then(|payload| {
                    session.append_history_event("MFB_HISTORY_OUTPUT_STORAGE", payload)
                })
                .context("worker output storage receipt write failed")
                .err()
            });
            let stats = match result {
                Ok(stats) if session_log_error.is_some() || config_marker_error.is_some() => {
                    collected_error = match (session_log_error, config_marker_error) {
                        (Some(output_error), Some(config_error)) => Some(anyhow::anyhow!(
                            "processing history output write failed: {output_error:#}; child configuration marker failed: {config_error:#}"
                        )),
                        (Some(error), None) | (None, Some(error)) => Some(error),
                        (None, None) => unreachable!("checked child history errors"),
                    };
                    stats
                }
                Ok(stats) => stats,
                Err(error) => {
                    if let Some(invocation_error) = &invocation_error {
                        eprintln!("[LOG] {invocation_error:#}");
                    }
                    let event = format!(
                        "{pipeline_label}_PIPELINE_STREAM_ERROR elapsed_secs={} error={error}",
                        pipeline_started_at.elapsed().as_secs()
                    );
                    let _ = append_jsonl_audit_record(&heartbeat, &event);
                    if let Some(log_error) = session_log_error {
                        eprintln!("[LOG] processing history output write failed: {log_error:#}");
                    }
                    if let Some(config_error) = config_marker_error {
                        eprintln!("[LOG] child configuration marker failed: {config_error:#}");
                    }
                    if let Some(snapshot_error) = snapshot_marker_error {
                        eprintln!("[LOG] {snapshot_error:#}");
                    }
                    if let Some(execution_error) = execution_marker_error {
                        eprintln!("[LOG] {execution_error:#}");
                    }
                    if let Some(receipt_error) = storage_receipt_error {
                        eprintln!("[LOG] {receipt_error:#}");
                    }
                    if let Err(log_error) = session.append_session_line(&event) {
                        eprintln!("[LOG] session stream error write failed: {log_error:#}");
                    }
                    return Err(error);
                }
            };
            if let Some(error) = execution_marker_error {
                collected_error = Some(match collected_error {
                    Some(cause) => anyhow::anyhow!("{cause:#}; {error:#}"),
                    None => error,
                });
            }
            if let Some(error) = snapshot_marker_error {
                collected_error = Some(match collected_error {
                    Some(cause) => anyhow::anyhow!("{cause:#}; {error:#}"),
                    None => error,
                });
            }
            if let Some(error) = storage_receipt_error {
                collected_error = Some(match collected_error {
                    Some(cause) => anyhow::anyhow!("{cause:#}; {error:#}"),
                    None => error,
                });
            }
            stats
        } else {
            self.verify_sources_unchanged(invocation_error.as_ref())?;
            foundation::infra::production_lock::inherit_active_lease(&mut cmd)
                .context("attach active production task lease to worker")?;
            let mut child = cmd
                .spawn()
                .with_context(|| format!("launch {}", self.display()))?;
            let status = child
                .wait()
                .with_context(|| format!("wait {}", self.display()))?;
            child_status = Some(status);
            ProcessorStats {
                exit_code: delegated_exit_code(
                    status,
                    &self.program.to_string_lossy(),
                    "LaunchCommand",
                ),
                ..ProcessorStats::default()
            }
        };
        if let Some(error) = invocation_error {
            collected_error = Some(match collected_error {
                Some(cause) => anyhow::anyhow!("{cause:#}; {error:#}"),
                None => error,
            });
        }
        let os_exit_code = child_status.and_then(|status| status.code());
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            child_status.and_then(|status| status.signal())
        };
        #[cfg(not(unix))]
        let signal: Option<i32> = None;
        let termination = if signal.is_some() {
            "signal"
        } else if os_exit_code.is_some() {
            "exit"
        } else {
            "unknown"
        };
        let failure_description = signal.map_or_else(
            || format!("failed with exit {}", stats.exit_code),
            |signal| format!("terminated by signal {signal}"),
        );
        if let Some(session) = session {
            let count = |index: usize, value: usize| {
                stats.reported[index]
                    .then_some(value)
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
            };
            let event = format!(
                "{}_PIPELINE_EXIT code={} termination={} exit_code={} signal={} succeeded={} skipped={} ignored={} failed={} unprocessed={}",
                self.pipeline_label(),
                stats.exit_code,
                termination,
                os_exit_code.map_or_else(|| "none".to_owned(), |code| code.to_string()),
                signal.map_or_else(|| "none".to_owned(), |signal| signal.to_string()),
                count(0, stats.succeeded),
                count(1, stats.skipped),
                count(2, stats.ignored),
                count(3, stats.failed),
                stats
                    .unprocessed
                    .map_or_else(|| "unknown".to_owned(), |value| value.to_string())
            );
            for result in [
                append_jsonl_audit_record(session.session_audit(), &event),
                session.append_session_line(&event),
                session.append_verbose_line(&event),
            ] {
                if let Err(cause) = result
                    && collected_error.is_none()
                {
                    collected_error = Some(cause);
                }
            }
            if stats.exit_code != 0 {
                let error_event = format!(
                    "{}_PIPELINE_ERROR command {}: {}",
                    self.pipeline_label(),
                    failure_description,
                    self.display()
                );
                for result in [
                    session.append_session_line(&error_event),
                    session.append_verbose_line(&error_event),
                ] {
                    if let Err(cause) = result
                        && collected_error.is_none()
                    {
                        collected_error = Some(cause);
                    }
                }
            }
        }
        if let Some(cause) = collected_error {
            return Err(CollectedChildError { stats, cause }.into());
        }
        foundation::batch_control::checkpoint()?;
        if stats.exit_code != 0 && bail_on_failure {
            bail!("command {}: {}", failure_description, self.display());
        }
        Ok(stats)
    }

    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::with_capacity(self.args.len() + 1);
        argv.push(self.program.to_string_lossy().into_owned());
        argv.extend(self.args.iter().cloned());
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestSession {
        stamp: String,
        run_id: Option<String>,
        log_dir: PathBuf,
        audit: PathBuf,
        lines: PathBuf,
        events: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
        fail_execution: bool,
        fail_output_receipt: bool,
        tool_snapshots: ToolSnapshotBindings,
    }

    impl WorkerSession for TestSession {
        fn stamp(&self) -> &str {
            &self.stamp
        }

        fn run_id(&self) -> Option<&str> {
            self.run_id.as_deref()
        }

        fn log_dir(&self) -> &Path {
            &self.log_dir
        }

        fn session_audit(&self) -> &Path {
            &self.audit
        }

        fn tool_snapshots(&self) -> &ToolSnapshotBindings {
            &self.tool_snapshots
        }

        fn append_history_event(&self, name: &str, payload: serde_json::Value) -> Result<()> {
            if self.fail_execution && name == "MFB_HISTORY_WORKER_EXECUTION" {
                bail!("synthetic execution observation storage failure");
            }
            if self.fail_output_receipt && name == "MFB_HISTORY_OUTPUT_STORAGE" {
                bail!("synthetic output receipt storage failure");
            }
            self.events
                .lock()
                .map_err(|_| anyhow::anyhow!("test history event lock poisoned"))?
                .push((name.to_owned(), payload));
            Ok(())
        }

        fn append_session_line(&self, event: &str) -> Result<()> {
            use std::io::Write as _;
            writeln!(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.lines)?,
                "{event}"
            )?;
            Ok(())
        }

        fn append_verbose_line(&self, event: &str) -> Result<()> {
            self.append_session_line(event)
        }
    }

    fn test_session(directory: &Path, run_id: Option<&str>) -> TestSession {
        TestSession {
            stamp: "test-session".to_owned(),
            run_id: run_id.map(str::to_owned),
            log_dir: directory.to_owned(),
            audit: directory.join("session-audit.jsonl"),
            lines: directory.join("session.log"),
            events: std::sync::Mutex::new(Vec::new()),
            fail_execution: false,
            fail_output_receipt: false,
            tool_snapshots: ToolSnapshotBindings::default(),
        }
    }

    #[test]
    fn snapshot_bindings_reject_invalid_receipts_without_cross_pipeline_state() {
        let event = foundation::tool_snapshot::SnapshotEvent {
            schema_version: 1,
            pipeline: "img".into(),
            state: "preparing".into(),
            boundary: "persisted-external".into(),
            identity: None,
            manifest_path: None,
            tool_names: Vec::new(),
            observed_tool_names: Vec::new(),
            cause_code: None,
            tool: None,
            message: "Preparing".into(),
        };
        let bindings = ToolSnapshotBindings::default();
        let raw = serde_json::to_string(&event).unwrap();
        bindings.observe("img", &raw).unwrap();
        assert!(bindings.manifest("img").unwrap().is_none());
        assert!(bindings.observe("vid", &raw).is_err());
        assert!(bindings.manifest("vid").is_err());
        assert!(bindings.manifest("img").unwrap().is_none());
        for (field, value) in [
            ("schema_version", serde_json::json!(2)),
            ("state", serde_json::json!("unknown")),
            ("identity", serde_json::json!("forged")),
            ("extra", serde_json::json!(true)),
        ] {
            let invalid = ToolSnapshotBindings::default();
            let mut payload = serde_json::to_value(&event).unwrap();
            payload[field] = value;
            assert!(invalid.observe("img", &payload.to_string()).is_err());
            assert!(invalid.manifest("img").is_err());
            assert!(invalid.observe("img", &raw).is_err());
        }
        let incomplete = ToolSnapshotBindings::default();
        let mut payload = serde_json::to_value(&event).unwrap();
        payload.as_object_mut().unwrap().remove("identity");
        assert!(incomplete.observe("img", &payload.to_string()).is_err());
    }

    #[test]
    #[cfg(target_os = "macos")]
    #[serial_test::serial]
    fn snapshot_bindings_reuse_one_runtime_across_workers_and_refuse_conflicts() -> Result<()> {
        use foundation::tool_snapshot::{MANIFEST_ENV, SnapshotEvent, prepare};
        use std::os::unix::fs::PermissionsExt;
        let _dyld = foundation::common_utils::EnvGuard::set("DYLD_FALLBACK_LIBRARY_PATH", "");
        // SAFETY: this serial fixture temporarily removes Cargo's injected loader override.
        unsafe { std::env::remove_var("DYLD_FALLBACK_LIBRARY_PATH") };
        let directory = tempfile::tempdir()?;
        let snapshot = prepare(
            directory.path(),
            &BTreeMap::from([("cjxl".into(), PathBuf::from("/bin/sh"))]),
        )?;
        let event = SnapshotEvent {
            schema_version: 1,
            pipeline: "img".into(),
            state: "ready".into(),
            boundary: "persisted-external".into(),
            identity: Some(snapshot.identity.clone()),
            manifest_path: Some(snapshot.manifest_path.clone()),
            tool_names: snapshot.tools.keys().cloned().collect(),
            observed_tool_names: Vec::new(),
            cause_code: None,
            tool: None,
            message: "Private runtime ready".into(),
        };
        let raw = serde_json::to_string(&event)?;
        let session = test_session(directory.path(), Some("snapshot-run"));
        let worker = directory.path().join("img");
        fs::write(
            &worker,
            "#!/bin/sh\nif [ \"$1\" = emit ]; then printf 'MFB_TOOL_SNAPSHOT=%s\\n' \"$2\"; else [ \"$MFB_TOOL_SNAPSHOT_MANIFEST\" = \"$2\" ] || exit 91; fi\nprintf 'Succeeded: 1\\nSkipped: 0\\nIgnored: 0\\nFailed: 0\\n'\n",
        )?;
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o755))?;
        let command = |operation: &str, value: String| {
            LaunchCommand::from_argv(vec![
                worker.to_string_lossy().into_owned(),
                operation.into(),
                value,
            ])
        };
        let first = command("emit", raw.clone())?.run_collecting(false, Some(&session), false)?;
        assert_eq!(first.exit_code, 0);
        let manifest = snapshot.manifest_path.to_string_lossy().into_owned();
        let next = command("inherit", manifest.clone())?;
        assert_eq!(
            next.run_collecting(false, Some(&session), false)?.exit_code,
            0
        );
        session.tool_snapshots.observe("img", &raw)?;
        assert_eq!(
            session.tool_snapshots.manifest("img")?,
            Some(manifest.clone())
        );
        assert!(session.tool_snapshots.manifest("vid")?.is_none());
        let fresh = test_session(directory.path(), Some("fresh-run"));
        assert_eq!(
            next.run_collecting(false, Some(&fresh), false)?.exit_code,
            91
        );
        let mut conflict = next.clone();
        conflict
            .env_overrides
            .push((MANIFEST_ENV.into(), Some("/unrelated/manifest.json".into())));
        assert!(
            conflict
                .run_collecting(false, Some(&session), false)
                .is_err()
        );
        let mut changed = event.clone();
        changed.identity = Some("0".repeat(64));
        assert!(
            ToolSnapshotBindings::default()
                .observe("img", &serde_json::to_string(&changed)?)
                .is_err()
        );
        let other = prepare(
            directory.path(),
            &BTreeMap::from([("cjxl".into(), PathBuf::from("/bin/bash"))]),
        )?;
        changed.identity = Some(other.identity);
        changed.manifest_path = Some(other.manifest_path);
        assert!(
            session
                .tool_snapshots
                .observe("img", &serde_json::to_string(&changed)?)
                .is_err()
        );
        assert!(session.tool_snapshots.manifest("img").is_err());
        assert_eq!(
            session.tool_snapshots.pipelines.lock().unwrap()["img"]
                .snapshot
                .as_ref()
                .unwrap()
                .identity,
            snapshot.identity
        );
        assert!(session.tool_snapshots.observe("img", &raw).is_err());
        let configured = ToolSnapshotBindings::default();
        configured.observe_config(
            "img",
            &BTreeMap::from([("cjxl".into(), PathBuf::from("/bin/sh"))]),
        )?;
        configured.observe("img", &raw)?;
        assert!(
            configured
                .observe_config(
                    "img",
                    &BTreeMap::from([("cjxl".into(), PathBuf::from("/bin/ls"))])
                )
                .is_err()
        );
        assert!(configured.manifest("img").is_err());
        Ok(())
    }

    #[test]
    fn worker_child_marker_probe() {
        if let Some(path) = std::env::var_os("MFB_WORKER_TEST_MARKER") {
            fs::write(path, b"launched").unwrap();
        }
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn worker_execution_records_os_pid_exit_and_preserves_failed_counts() {
        let directory = tempfile::tempdir().unwrap();
        let session = test_session(directory.path(), Some("run-execution"));
        let snapshot_markers = [
            serde_json::json!({
                "schema_version": 1, "state": "ready", "identity": "a".repeat(64),
                "manifest_path": directory.path().join("snapshot.json"),
                "boundary": "macos_snapshot", "cause_code": null, "message": null,
            }),
            serde_json::json!({
                "schema_version": 1, "state": "blocked", "identity": null,
                "manifest_path": null, "boundary": "blocked", "cause_code": "snapshot_failed",
                "message": "Tool unavailable; Failed: 999 is diagnostic text, not a file count",
            }),
        ]
        .map(|value| format!("MFB_TOOL_SNAPSHOT={value}"));
        let command = LaunchCommand::from_argv(vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf '%s\\n' \"$1\"; printf '%s\\n' \"$2\" >&2; printf 'Succeeded: 2\\nSkipped: 0\\nIgnored: 0\\nFailed: 1\\n'; exit 7".into(),
            "test".into(),
            snapshot_markers[0].clone(),
            snapshot_markers[1].clone(),
        ])
        .unwrap();
        let stats = command
            .run_collecting(false, Some(&session), false)
            .unwrap();
        assert_eq!(stats.exit_code, 7);
        assert_eq!((stats.succeeded, stats.failed), (2, 1));
        let events = session.events.lock().unwrap();
        let observations = events
            .iter()
            .filter(|(name, _)| name == "MFB_HISTORY_WORKER_EXECUTION")
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0]["state"], "spawned");
        assert_eq!(observations[1]["state"], "exited");
        assert_eq!(observations[0]["pid"], observations[1]["pid"]);
        assert_ne!(
            observations[0]["pid"].as_u64(),
            Some(u64::from(std::process::id()))
        );
        assert_eq!(observations[1]["exit_code"], 7);
        assert!(observations[1]["signal"].is_null());
        assert_eq!(observations[0]["run_id"], "run-execution");
        assert_eq!(
            observations[0]["invocation_id"],
            events[0].1["invocation_id"]
        );
        let storage = events
            .iter()
            .filter(|(name, _)| name == "MFB_HISTORY_OUTPUT_STORAGE")
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>();
        assert_eq!(storage.len(), 1);
        let storage = storage[0];
        assert_eq!(storage["scope"], "captured_worker_output");
        assert_eq!(storage["run_id"], "run-execution");
        assert_eq!(storage["invocation_id"], events[0].1["invocation_id"]);
        assert_eq!(storage["pipeline"], "cmd");
        assert_eq!(storage["state"], "committed");
        assert_eq!(storage["capture_complete"], true);
        assert!(storage["diagnostic"].is_null());
        let database = rusqlite::Connection::open(
            directory
                .path()
                .join(crate::infra::history_store::HISTORY_DATABASE),
        )
        .unwrap();
        let (rows, bytes): (i64, i64) = database
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(length(CAST(line AS BLOB))), 0) FROM history_output",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(rows >= 4);
        assert_eq!(storage["observed_records"], rows);
        assert_eq!(storage["committed_records"], rows);
        assert_eq!(storage["committed_utf8_bytes"], bytes);
        for marker in &snapshot_markers {
            let stored: i64 = database
                .query_row(
                    "SELECT COUNT(*) FROM history_output WHERE line=?1",
                    [marker],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                stored, 1,
                "tool snapshot control line must be retained exactly"
            );
        }
        drop(events);
        let mut failing = test_session(directory.path(), None);
        failing.fail_execution = true;
        let error = command
            .run_collecting(false, Some(&failing), false)
            .unwrap_err();
        let observed = error.downcast_ref::<CollectedChildError>().unwrap();
        assert_eq!(observed.stats.exit_code, 7);
        assert_eq!((observed.stats.succeeded, observed.stats.failed), (2, 1));
        assert!(
            observed
                .cause
                .to_string()
                .contains("worker execution history write failed")
        );
        let mut receipt_failure = test_session(directory.path(), None);
        receipt_failure.fail_output_receipt = true;
        let error = command
            .run_collecting(false, Some(&receipt_failure), false)
            .unwrap_err();
        let observed = error.downcast_ref::<CollectedChildError>().unwrap();
        assert_eq!(observed.stats.exit_code, 7);
        assert_eq!((observed.stats.succeeded, observed.stats.failed), (2, 1));
        assert!(
            format!("{:#}", observed.cause).contains("worker output storage receipt write failed")
        );
        let dry = test_session(directory.path(), None);
        command.run_collecting(true, Some(&dry), false).unwrap();
        assert!(dry.events.lock().unwrap().is_empty());
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn worker_output_storage_failure_records_only_committed_text_and_drains_child() {
        let directory = tempfile::tempdir().unwrap();
        let session = test_session(directory.path(), Some("run-storage-failure"));
        drop(HistoryOutputWriter::open(directory.path(), session.stamp()).unwrap());
        let database = rusqlite::Connection::open(
            directory
                .path()
                .join(crate::infra::history_store::HISTORY_DATABASE),
        )
        .unwrap();
        database
            .execute_batch(
                "CREATE TRIGGER reject_output BEFORE INSERT ON history_output BEGIN SELECT RAISE(ABORT, 'synthetic output failure'); END;",
            )
            .unwrap();
        let finished = directory.path().join("finished");
        let command = LaunchCommand::from_argv(vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'MFB_HISTORY_OUTPUT_STORAGE={}\\n'; i=0; while [ $i -lt 140 ]; do printf 'synthetic text\\n'; i=$((i+1)); done; printf 'Succeeded: 2\\nFailed: 1\\n'; printf done > \"$1\"; exit 7".into(),
            "test".into(),
            finished.to_string_lossy().into_owned(),
        ])
        .unwrap();
        let error = command
            .run_collecting(false, Some(&session), false)
            .unwrap_err();
        let observed = error.downcast_ref::<CollectedChildError>().unwrap();
        assert_eq!(observed.stats.exit_code, 7);
        assert_eq!((observed.stats.succeeded, observed.stats.failed), (2, 1));
        assert_eq!(fs::read_to_string(finished).unwrap(), "done");
        let events = session.events.lock().unwrap();
        let receipts = events
            .iter()
            .filter(|(name, _)| name == "MFB_HISTORY_OUTPUT_STORAGE")
            .map(|(_, payload)| payload)
            .collect::<Vec<_>>();
        assert_eq!(receipts.len(), 1);
        let receipt = receipts[0];
        assert_eq!(receipt["state"], "failed");
        assert_eq!(receipt["capture_complete"], true);
        assert!(receipt["observed_records"].as_u64().unwrap() >= 143);
        assert_eq!(receipt["committed_records"], 0);
        assert_eq!(receipt["committed_utf8_bytes"], 0);
        assert!(
            receipt["diagnostic"]
                .as_str()
                .unwrap()
                .contains("synthetic output failure")
        );
        let rows: i64 = database
            .query_row("SELECT COUNT(*) FROM history_output", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    #[cfg(unix)]
    #[serial_test::serial]
    fn worker_execution_separates_signal_from_exit_code_and_spawn_failure() {
        let directory = tempfile::tempdir().unwrap();
        let session = test_session(directory.path(), None);
        let command =
            LaunchCommand::from_argv(vec!["/bin/sh".into(), "-c".into(), "kill -TERM $$".into()])
                .unwrap();
        let stats = command
            .run_collecting(false, Some(&session), false)
            .unwrap();
        let events = session.events.lock().unwrap();
        let last = events
            .iter()
            .rfind(|(name, _)| name == "MFB_HISTORY_WORKER_EXECUTION")
            .unwrap();
        assert_eq!(last.1["signal"], libc::SIGTERM);
        assert!(last.1["exit_code"].is_null());
        drop(events);
        assert!(!stats.counts_complete());
        assert_eq!(stats.reported, [false; 4]);
        let audit = fs::read_to_string(&session.lines).unwrap();
        assert!(
            audit.contains("termination=signal exit_code=none signal=15"),
            "session logs must preserve the observed OS termination: {audit}"
        );
        assert!(!audit.contains("command failed with exit -1"));
        let failed = test_session(directory.path(), None);
        let command =
            LaunchCommand::from_argv(vec![directory.path().to_string_lossy().into_owned()])
                .unwrap();
        assert!(command.run_collecting(false, Some(&failed), false).is_err());
        assert!(
            !failed
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|(name, _)| name == "MFB_HISTORY_WORKER_EXECUTION")
        );
    }

    #[test]
    fn launch_observation_records_exact_binary_and_unknown_diagnostics() {
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("worker");
        fs::write(&program, b"worker-bytes").unwrap();

        let observed = BinaryObservation::observe(Some(&program));
        assert_eq!(observed.scope, "observed_before_launch");
        assert_eq!(observed.bytes, Some(12));
        assert_eq!(
            observed.blake3.as_deref(),
            Some(blake3::hash(b"worker-bytes").to_hex().as_str())
        );
        assert_eq!(observed.diagnostic, None);

        let unknown = BinaryObservation::observe(Some(&directory.path().join("missing")));
        assert_eq!(unknown.bytes, None);
        assert_eq!(unknown.blake3, None);
        assert!(unknown.diagnostic.is_some());

        #[cfg(unix)]
        {
            let session = test_session(directory.path(), Some("run-test"));
            let mut command = LaunchCommand::from_argv(vec!["worker".to_owned()]).unwrap();
            for (path, diagnostic) in [
                (None, "PATH is absent"),
                (Some("invalid\0PATH".to_owned()), "contains a NUL"),
            ] {
                command.env_overrides.push(("PATH".to_owned(), path));
                let unresolved = command.invocation(&session).unwrap();
                assert_eq!(unresolved.program_path, None);
                assert_eq!(unresolved.binary.bytes, None);
                assert_eq!(unresolved.binary.blake3, None);
                assert!(
                    unresolved
                        .binary
                        .diagnostic
                        .as_ref()
                        .unwrap()
                        .contains(diagnostic)
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn child_reserved_identity_environment_uses_launch_observation() {
        let directory = tempfile::tempdir().unwrap();
        let session = test_session(directory.path(), Some("run-123"));
        let receipt = directory.path().join("identity.txt");
        let mut command = LaunchCommand::from_argv(vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "printf '%s\\n%s' \"${MFB_RUN_ID-}\" \"${MFB_INVOCATION_ID-}\" > \"$1\"".to_owned(),
            "test".to_owned(),
            receipt.to_string_lossy().into_owned(),
        ])
        .unwrap();
        command.env_overrides.extend([
            ("MFB_RUN_ID".to_owned(), Some("stale-run".to_owned())),
            (
                "MFB_INVOCATION_ID".to_owned(),
                Some("stale-invocation".to_owned()),
            ),
        ]);

        command
            .run_collecting(false, Some(&session), false)
            .unwrap();

        let lines = fs::read_to_string(receipt).unwrap();
        let mut lines = lines.lines();
        assert_eq!(lines.next(), Some("run-123"));
        let child_invocation = lines.next().unwrap();
        assert!(!child_invocation.is_empty());
        assert_ne!(child_invocation, "stale-invocation");
        let events = session.events.lock().unwrap();
        let invocation = events
            .iter()
            .find(|(name, _)| name == "MFB_HISTORY_INVOCATION")
            .map(|(_, payload)| payload)
            .unwrap();
        assert_eq!(invocation["run_id"], "run-123");
        assert_eq!(invocation["invocation_id"], child_invocation);
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn child_reserved_identity_environment_removes_stale_run_id() {
        let directory = tempfile::tempdir().unwrap();
        let session = test_session(directory.path(), None);
        let receipt = directory.path().join("identity.txt");
        let mut command = LaunchCommand::from_argv(vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "printf '%s\\n%s' \"${MFB_RUN_ID-}\" \"${MFB_INVOCATION_ID-}\" > \"$1\"".to_owned(),
            "test".to_owned(),
            receipt.to_string_lossy().into_owned(),
        ])
        .unwrap();
        command.env_overrides.extend([
            ("MFB_RUN_ID".to_owned(), Some("stale-run".to_owned())),
            (
                "MFB_INVOCATION_ID".to_owned(),
                Some("stale-invocation".to_owned()),
            ),
        ]);

        command
            .run_collecting(false, Some(&session), false)
            .unwrap();

        let lines = fs::read_to_string(receipt).unwrap();
        let mut lines = lines.lines();
        assert_eq!(lines.next(), Some(""));
        let child_invocation = lines.next().unwrap();
        assert!(!child_invocation.is_empty());
        assert_ne!(child_invocation, "stale-invocation");
        let events = session.events.lock().unwrap();
        let invocation = events
            .iter()
            .find(|(name, _)| name == "MFB_HISTORY_INVOCATION")
            .map(|(_, payload)| payload)
            .unwrap();
        assert!(invocation["run_id"].is_null());
        assert_eq!(invocation["invocation_id"], child_invocation);
    }

    #[test]
    fn changed_preflight_source_refuses_before_spawning_child() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("source");
        fs::write(&source_path, b"before").unwrap();
        let metadata = fs::metadata(&source_path).unwrap();
        let source =
            foundation::batch::DiscoveredSource::from_metadata(&source_path, &metadata).unwrap();
        fs::write(&source_path, b"changed").unwrap();

        let child_marker = directory.path().join("child-was-launched");
        let mut argv = vec![
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ];
        argv.extend([
            "--exact".to_owned(),
            "app::worker::tests::worker_child_marker_probe".to_owned(),
        ]);
        let mut command = LaunchCommand::from_argv(argv).unwrap();
        command.env_overrides.push((
            "MFB_WORKER_TEST_MARKER".to_owned(),
            Some(child_marker.to_string_lossy().into_owned()),
        ));
        let mut command = command.with_sources(vec![source]);
        let error = command
            .run_collecting(false, None, false)
            .expect_err("changed source must prevent launch");

        assert!(format!("{error:#}").contains("worker source changed before launch"));
        assert!(
            !child_marker.exists(),
            "changed source must not spawn worker"
        );
        assert_eq!(fs::read(&source_path).unwrap(), b"changed");
        let refused = command
            .verify_sources_unchanged(Some(&anyhow::anyhow!("synthetic history write failure")))
            .unwrap_err();
        assert!(format!("{refused:#}").contains("synthetic history write failure"));
        assert!(format!("{refused:#}").contains("worker source changed before launch"));
        assert!(refused.downcast_ref::<io::Error>().is_some());
        command.sources = vec![
            foundation::batch::DiscoveredSource::from_metadata(
                &source_path,
                &fs::metadata(&source_path).unwrap(),
            )
            .unwrap(),
        ];
        assert_eq!(
            command
                .run_collecting(false, None, false)
                .unwrap()
                .exit_code,
            0
        );
        assert_eq!(fs::read(&child_marker).unwrap(), b"launched");
    }

    #[test]
    fn dry_run_does_not_verify_preflight_sources() {
        let directory = tempfile::tempdir().unwrap();
        let source_path = directory.path().join("source");
        fs::write(&source_path, b"before").unwrap();
        let metadata = fs::metadata(&source_path).unwrap();
        let source =
            foundation::batch::DiscoveredSource::from_metadata(&source_path, &metadata).unwrap();
        fs::write(&source_path, b"changed").unwrap();

        let command = LaunchCommand::from_argv(vec!["worker".to_owned()])
            .unwrap()
            .with_sources(vec![source]);
        assert!(command.run_collecting(true, None, false).is_ok());
        assert_eq!(fs::read(&source_path).unwrap(), b"changed");
    }

    #[test]
    fn child_config_marker_rejects_mismatched_or_incomplete_snapshots() {
        let config = RuntimeConfig::default();
        let mut paths = BTreeSet::new();
        collect_config_leaf_paths(&serde_json::to_value(&config).unwrap(), "", &mut paths);
        let sources = paths
            .into_iter()
            .map(|path| (path, "default"))
            .collect::<BTreeMap<_, _>>();
        let valid = serde_json::json!({
            "schema_version": 1,
            "pipeline": "img",
            "package_version": "0.12.0",
            "config": config,
            "sources": sources,
        });
        assert!(ChildConfigMarker::parse(&valid.to_string(), "img").is_ok());

        let mut invalid = valid.clone();
        invalid["pipeline"] = serde_json::json!("vid");
        assert!(ChildConfigMarker::parse(&invalid.to_string(), "img").is_err());
        invalid = valid.clone();
        invalid["sources"]
            .as_object_mut()
            .unwrap()
            .remove("vid.codec");
        assert!(ChildConfigMarker::parse(&invalid.to_string(), "img").is_err());
    }
}

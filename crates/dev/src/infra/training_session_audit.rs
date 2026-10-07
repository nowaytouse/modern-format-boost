use anyhow::Result;
use chrono::Utc;
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const TRAINING_SESSION_AUDIT_JSONL: &str = "training_session_audit.jsonl";
const TRAINING_SESSION_EXIT_JSON: &str = "training_session_exit.json";
const DEFAULT_HEARTBEAT_SECS: f64 = 60.0;

fn utc_now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub struct TrainingSessionRecorder {
    _log_dir: PathBuf,
    session_stamp: String,
    heartbeat_secs: f64,
    started_mono: Instant,
    phase: String,
    finalized: bool,
    last_heartbeat_mono: Instant,
    audit_path: PathBuf,
    exit_path: PathBuf,
}

impl TrainingSessionRecorder {
    pub fn new(
        log_dir: &Path,
        session_stamp: &str,
        heartbeat_secs: Option<f64>,
    ) -> Result<Arc<Mutex<Self>>> {
        let log_dir = log_dir.to_path_buf();
        fs::create_dir_all(&log_dir)?;

        let mut hb = heartbeat_secs.unwrap_or(DEFAULT_HEARTBEAT_SECS);
        if hb < 15.0 {
            hb = 15.0;
        }

        let audit_path = log_dir.join(TRAINING_SESSION_AUDIT_JSONL);
        let exit_path = log_dir.join(TRAINING_SESSION_EXIT_JSON);

        let recorder = Self {
            _log_dir: log_dir,
            session_stamp: session_stamp.trim().to_string(),
            heartbeat_secs: hb,
            started_mono: Instant::now(),
            phase: "init".to_string(),
            finalized: false,
            last_heartbeat_mono: Instant::now(),
            audit_path,
            exit_path,
        };

        let recorder_arc = Arc::new(Mutex::new(recorder));

        // Setup ctrl-c handler if possible, though we may only want one handler per
        // process. We'll leave the actual ctrlc::set_handler to the binary that
        // uses this, since setting it here could conflict with existing
        // handlers.

        Ok(recorder_arc)
    }

    pub fn emit(&self, event: &str, fields: Option<serde_json::Map<String, Value>>) -> Result<()> {
        let mut record = json!({
            "ts": utc_now(),
            "event": event,
            "pid": process::id(),
            "session_stamp": if self.session_stamp.is_empty() { Value::Null } else { Value::String(self.session_stamp.clone()) },
            "lane": match std::env::var("MFB_TRAINING_LANE") {
                Ok(val) if !val.trim().is_empty() => Value::String(val.trim().to_string()),
                _ => Value::Null,
            },
            "phase": self.phase,
        });

        if let Some(map) = fields
            && let Value::Object(ref mut obj) = record
        {
            for (k, v) in map {
                obj.insert(k, v);
            }
        }

        let mut line = serde_json::to_string(&record)?;
        line.push('\n');

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.audit_path)?;

        file.write_all(line.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    }

    pub fn set_phase(
        &mut self,
        phase: &str,
        fields: Option<serde_json::Map<String, Value>>,
    ) -> Result<()> {
        self.phase = phase.to_string();
        self.emit("phase", fields)
    }

    pub fn maybe_heartbeat(
        &mut self,
        fields: Option<serde_json::Map<String, Value>>,
    ) -> Result<()> {
        let now = Instant::now();
        if now.duration_since(self.last_heartbeat_mono).as_secs_f64() < self.heartbeat_secs {
            return Ok(());
        }
        self.last_heartbeat_mono = now;

        let elapsed = now.duration_since(self.started_mono).as_secs_f64();
        let mut final_fields = serde_json::Map::new();
        final_fields.insert(
            "elapsed_secs".to_string(),
            json!((elapsed * 10.0).round() / 10.0),
        );

        if let Some(map) = fields {
            for (k, v) in map {
                final_fields.insert(k, v);
            }
        }

        self.emit("heartbeat", Some(final_fields))
    }

    pub fn finalize(
        &mut self,
        exit_code: i32,
        reason: &str,
        interrupted: bool,
        fields: Option<serde_json::Map<String, Value>>,
    ) -> Result<()> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        let elapsed = Instant::now()
            .duration_since(self.started_mono)
            .as_secs_f64();
        let elapsed_rounded = (elapsed * 10.0).round() / 10.0;

        let mut payload = serde_json::Map::new();
        payload.insert(
            "session_stamp".to_string(),
            if self.session_stamp.is_empty() {
                Value::Null
            } else {
                Value::String(self.session_stamp.clone())
            },
        );
        payload.insert(
            "lane".to_string(),
            match std::env::var("MFB_TRAINING_LANE") {
                Ok(val) if !val.trim().is_empty() => Value::String(val.trim().to_string()),
                _ => Value::Null,
            },
        );
        payload.insert("pid".to_string(), json!(process::id()));
        payload.insert("exit_code".to_string(), json!(exit_code));
        payload.insert("reason".to_string(), json!(reason));
        payload.insert("phase".to_string(), json!(self.phase));
        payload.insert("interrupted".to_string(), json!(interrupted));
        payload.insert("elapsed_secs".to_string(), json!(elapsed_rounded));
        payload.insert("finished_at".to_string(), json!(utc_now()));

        if let Some(map) = fields {
            for (k, v) in map {
                payload.insert(k, v);
            }
        }

        let json_str = serde_json::to_string_pretty(&Value::Object(payload.clone()))?;
        fs::write(&self.exit_path, format!("{}\n", json_str))?;

        self.emit("session_exit", Some(payload))?;

        eprintln!(
            "  [TRAINING-EXIT] code={} reason={} phase={} elapsed={}s audit={}",
            exit_code,
            reason,
            self.phase,
            elapsed_rounded,
            self.audit_path.display()
        );

        Ok(())
    }

    pub fn read_exit_snapshot(&self) -> Option<Value> {
        if !self.exit_path.is_file() {
            return None;
        }
        let data = match fs::read_to_string(&self.exit_path) {
            Ok(v) => v,
            Err(err) => {
                eprintln!(
                    "[AUDIT] exit snapshot read failed ({}): {err}",
                    self.exit_path.display()
                );
                return None;
            }
        };
        let val = match serde_json::from_str::<Value>(&data) {
            Ok(v) => v,
            Err(err) => {
                eprintln!(
                    "[AUDIT] exit snapshot parse failed ({}): {err}",
                    self.exit_path.display()
                );
                return None;
            }
        };
        if val.is_object() { Some(val) } else { None }
    }

    /// Install SIGINT/SIGTERM handlers that finalize audit on abrupt exit.
    pub fn install_handlers(rec: Arc<Mutex<Self>>) {
        let weak = Arc::downgrade(&rec);
        let _ = ctrlc::set_handler(move || {
            if let Some(rec) = weak.upgrade() {
                match rec.lock() {
                    Ok(mut guard) => {
                        let _ = guard.finalize(130, "signal:SIGINT", true, None);
                    }
                    Err(err) => eprintln!("[AUDIT] signal handler lock poisoned: {err}"),
                }
            }
            process::exit(130);
        });
    }
}

pub fn summarize_argv(argv: Option<Vec<String>>) -> Vec<String> {
    let tail = argv.unwrap_or_else(|| std::env::args().collect());
    let mut out = Vec::new();
    let mut skip_next = false;

    for arg in tail {
        if skip_next {
            out.push("<redacted>".to_string());
            skip_next = false;
            continue;
        }
        if arg == "--password" || arg == "--connstr" || arg.starts_with("--pg-") {
            out.push(arg);
            skip_next = true;
            continue;
        }
        out.push(arg);
    }

    let skip = out.len().saturating_sub(24);
    out.into_iter().skip(skip).collect()
}

//! Startup admission diagnostics are distinct from external-tool observations.

#![expect(
    clippy::redundant_pub_crate,
    reason = "Startup diagnostics stay internal; public visibility conflicts with unreachable_pub"
)]

use super::Commands;
use anyhow::{Context, Result};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub(super) enum SettingsError {
    Unknown,
    Changed(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown => formatter.write_str(
                "MFB_RESUME_CONFIG_UNKNOWN: saved fast-img task has no processing-settings identity; its outputs and recovery evidence were retained. Use --no-resume for an isolated new task instead of assigning current settings to old results",
            ),
            Self::Changed(message) => write!(formatter, "MFB_RESUME_CONFIG_CHANGED: {message}"),
        }
    }
}

impl std::error::Error for SettingsError {}

#[derive(Debug, Serialize)]
pub(super) struct Blocked {
    pub(super) schema_version: u8,
    pub(super) pipeline: &'static str,
    pub(super) cause_code: &'static str,
    pub(super) input_path: Option<PathBuf>,
    pub(super) requested_output: Option<PathBuf>,
    pub(super) message: String,
}

impl Blocked {
    pub(super) fn from_error(command: &Commands, error: &anyhow::Error) -> Result<Self> {
        let (input, output) = match command {
            Commands::Run { input, output, .. }
            | Commands::FastImg { input, output, .. }
            | Commands::RestoreJpeg { input, output, .. } => {
                (Some(input.as_path()), output.as_deref())
            }
            _ => (None, None),
        };
        let absolute = |path: Option<&Path>| path.map(std::path::absolute).transpose();
        let cause_code = match error
            .chain()
            .find_map(|cause| cause.downcast_ref::<SettingsError>())
        {
            Some(SettingsError::Unknown) => "resume_config_unknown",
            Some(SettingsError::Changed(_)) => "resume_config_changed",
            None => "admission_failed",
        };
        Ok(Self {
            schema_version: 1,
            pipeline: "img",
            cause_code,
            input_path: absolute(input).context("resolve admission diagnostic input")?,
            requested_output: absolute(output).context("resolve admission diagnostic output")?,
            message: format!("{error:#}").chars().take(2048).collect(),
        })
    }
}

pub(super) fn emit(event: &Blocked) -> Result<()> {
    let encoded = serde_json::to_string(event)?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "MFB_IMG_ADMISSION_BLOCKED={encoded}")?;
    stdout.flush()?;
    Ok(())
}

pub(super) fn is_interrupted(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::Interrupted)
    })
}

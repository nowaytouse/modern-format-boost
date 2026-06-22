//! Gate-compliant helpers for dev tooling (no silent parse/IO residue).

use foundation::media_conversion_gate::process_exit_code_for_context;
use std::path::Path;
use std::process::ExitStatus;

/// Map subprocess exit status through the shared conversion gate.
pub fn delegated_exit_code(status: ExitStatus, tool: &str, context: &str) -> i32 {
    process_exit_code_for_context(status.code(), tool, context)
}

/// Read a non-empty environment variable.
#[must_use]
pub fn optional_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(raw) if !raw.trim().is_empty() => Some(raw),
        Ok(_) => None,
        Err(err) => {
            eprintln!("[HARDENING] env {name} unavailable: {err}");
            None
        }
    }
}

/// Parse a positive integer environment variable.
#[must_use]
pub fn positive_usize_env(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(parsed) if parsed > 0 => parsed,
            Ok(_) | Err(_) => default,
        },
        Err(err) => {
            eprintln!("[HARDENING] env {name} unavailable: {err}");
            default
        }
    }
}

/// Parse a positive float environment variable.
#[must_use]
pub fn positive_f64_env(name: &str, default: f64) -> f64 {
    match std::env::var(name) {
        Ok(raw) => match raw.trim().parse::<f64>() {
            Ok(parsed) if parsed > 0.0 => parsed,
            Ok(_) | Err(_) => default,
        },
        Err(err) => {
            eprintln!("[HARDENING] env {name} unavailable: {err}");
            default
        }
    }
}

/// Parse optional unsigned field from whitespace-separated line (e.g. meminfo).
pub fn parse_kb_token(line: &str) -> Option<u64> {
    let token = line.split_whitespace().nth(1)?;
    match token.parse::<u64>() {
        Ok(v) => Some(v),
        Err(err) => {
            eprintln!("[HARDENING] kb parse failed for {line:?}: {err}");
            None
        }
    }
}

/// Parse optional summary integer; logs and returns `default` when label is absent.
#[must_use]
pub fn summary_int_or_default(
    summary: &str,
    label: &str,
    default: i32,
    lookup: impl Fn(&str, &str) -> Option<i32>,
) -> i32 {
    match lookup(summary, label) {
        Some(v) => v,
        None => default,
    }
}

#[must_use]
pub fn path_is_executable(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                let _ = meta;
                true
            }
        }
        Err(err) => {
            eprintln!("[HARDENING] metadata failed ({}): {err}", path.display());
            false
        }
    }
}

pub fn read_text_file(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) => {
            eprintln!("[HARDENING] read failed ({}): {err}", path.display());
            None
        }
    }
}

pub fn ensure_parent_dir(path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return true;
    };
    match std::fs::create_dir_all(parent) {
        Ok(()) => true,
        Err(err) => {
            eprintln!("[HARDENING] mkdir failed ({}): {err}", parent.display());
            false
        }
    }
}

pub fn flush_stdout() {
    if let Err(err) = std::io::Write::flush(&mut std::io::stdout()) {
        eprintln!("[HARDENING] stdout flush failed: {err}");
    }
}

pub fn read_stdin_line(buf: &mut String) -> bool {
    match std::io::stdin().read_line(buf) {
        Ok(0) => false,
        Ok(_) => true,
        Err(err) => {
            eprintln!("[HARDENING] stdin read failed: {err}");
            false
        }
    }
}

pub fn parse_usize(raw: &str, label: &str) -> Option<usize> {
    match raw.trim().parse::<usize>() {
        Ok(v) => Some(v),
        Err(err) => {
            eprintln!("[HARDENING] usize parse failed for {label}: {err}");
            None
        }
    }
}

pub fn parse_f64(raw: f64, label: &str) -> f64 {
    if raw.is_finite() {
        raw
    } else {
        eprintln!("[HARDENING] non-finite f64 for {label}; using 0.0");
        0.0
    }
}

pub fn file_name_display(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => {
            eprintln!("[HARDENING] path has no file_name: {}", path.display());
            String::new()
        }
    }
}

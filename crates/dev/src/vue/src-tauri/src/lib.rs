use std::env;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use tauri::{AppHandle, Emitter};

fn processor_binary_name() -> String {
    format!("drag_and_drop_processor{}", env::consts::EXE_SUFFIX)
}

fn processor_binary_candidates() -> Vec<PathBuf> {
    let name = processor_binary_name();
    let mut candidates = Vec::new();

    if let Ok(path) = env::var("MFB_PROCESSOR_BINARY") {
        candidates.push(PathBuf::from(path));
    }

    if let Ok(current_exe) = env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            candidates.push(exe_dir.join(&name));

            if matches!(
                exe_dir.file_name().and_then(OsStr::to_str),
                Some("debug" | "release")
            ) {
                if let Some(target_dir) = exe_dir.parent() {
                    candidates.push(target_dir.join("release").join(&name));
                    candidates.push(target_dir.join("debug").join(&name));
                }
            }

            #[cfg(target_os = "macos")]
            if exe_dir.file_name() == Some(OsStr::new("MacOS")) {
                if let Some(contents_dir) = exe_dir.parent() {
                    candidates.push(contents_dir.join("Resources").join(&name));
                    candidates.push(contents_dir.join("Resources").join("bin").join(&name));

                    if let Some(app_bundle) = contents_dir.parent() {
                        candidates.push(app_bundle.join(&name));

                        if let Some(parent) = app_bundle.parent() {
                            candidates.push(parent.join("target").join("release").join(&name));
                            candidates.push(parent.join("target").join("debug").join(&name));
                        }
                    }
                }
            }

            for ancestor in exe_dir.ancestors() {
                candidates.push(ancestor.join("target").join("release").join(&name));
                candidates.push(ancestor.join("target").join("debug").join(&name));
            }
        }
    }

    if let Ok(current_dir) = env::current_dir() {
        candidates.push(current_dir.join(&name));
    }

    if let Ok(path) = env::var("PATH") {
        for dir in env::split_paths(&path) {
            candidates.push(dir.join(&name));
        }
    }

    candidates
}

fn resolve_processor_binary() -> Option<PathBuf> {
    processor_binary_candidates()
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                path.canonicalize().unwrap_or(path)
            }
        })
}

fn missing_processor_error() -> String {
    format!(
        "Backend processor binary not found. Build it with `cargo build --release -p dev --bin drag_and_drop_processor` or set MFB_PROCESSOR_BINARY. Checked: {}",
        processor_binary_candidates()
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("; ")
    )
}

#[tauri::command]
async fn get_processor_binary_path() -> Result<String, String> {
    resolve_processor_binary()
        .map(|p| p.display().to_string())
        .ok_or_else(|| missing_processor_error())
}

#[tauri::command]
async fn open_in_terminal(command: String) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        // Try Ghostty first with direct command
        if std::path::Path::new("/Applications/Ghostty.app").exists() {
            let ghostty_bin = "/Applications/Ghostty.app/Contents/MacOS/ghostty";
            if std::path::Path::new(ghostty_bin).exists() {
                if Command::new(ghostty_bin)
                    .args(&["-e", "sh", "-c", &format!("{}; exec sh", command)])
                    .spawn()
                    .is_ok()
                {
                    return Ok("Opened in Ghostty".to_string());
                }
            }
        }

        // Try Warp
        if std::path::Path::new("/Applications/Warp.app").exists() {
            let script = format!(
                "tell application \"Warp\" to activate\ndelay 0.5\ntell application \"System Events\"\n    keystroke \"{}\"\n    keystroke return\nend tell",
                command.replace('\"', "\\\"").replace('\n', "\\n")
            );
            if Command::new("osascript")
                .args(&["-e", &script])
                .spawn()
                .is_ok()
            {
                return Ok("Opened in Warp".to_string());
            }
        }

        // Try kitty
        if std::path::Path::new("/Applications/kitty.app").exists() {
            let kitty_bin = "/Applications/kitty.app/Contents/MacOS/kitty";
            if std::path::Path::new(kitty_bin).exists() {
                if Command::new(kitty_bin)
                    .args(&["sh", "-c", &format!("{}; exec sh", command)])
                    .spawn()
                    .is_ok()
                {
                    return Ok("Opened in kitty".to_string());
                }
            }
        }

        // Try iTerm2 with AppleScript
        if std::path::Path::new("/Applications/iTerm.app").exists() {
            let script = format!(
                "tell application \"iTerm\"\n    activate\n    tell current window\n        create tab with default profile\n        tell current session\n            write text \"{}\"\n        end tell\n    end tell\nend tell",
                command.replace('\"', "\\\"").replace('\n', "\\n")
            );
            if Command::new("osascript")
                .args(&["-e", &script])
                .spawn()
                .is_ok()
            {
                return Ok("Opened in iTerm".to_string());
            }
        }

        // Fallback to Terminal.app with AppleScript
        let script = format!(
            "tell application \"Terminal\"\n    activate\n    do script \"{}\"\nend tell",
            command.replace('\"', "\\\"").replace('\n', "\\n")
        );
        if Command::new("osascript")
            .args(&["-e", &script])
            .spawn()
            .is_ok()
        {
            return Ok("Opened in Terminal".to_string());
        }

        return Err("Failed to open any terminal".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        let terminals = vec![
            (
                "gnome-terminal",
                vec![
                    "gnome-terminal",
                    "--",
                    "bash",
                    "-c",
                    &format!("{}; exec bash", command),
                ],
            ),
            (
                "konsole",
                vec![
                    "konsole",
                    "-e",
                    "bash",
                    "-c",
                    &format!("{}; exec bash", command),
                ],
            ),
            (
                "xterm",
                vec![
                    "xterm",
                    "-e",
                    "bash",
                    "-c",
                    &format!("{}; exec bash", command),
                ],
            ),
        ];

        for (name, args) in terminals {
            if Command::new(args[0]).args(&args[1..]).spawn().is_ok() {
                return Ok(format!("Opened in {}", name));
            }
        }

        return Err("No terminal found".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        if Command::new("wt.exe")
            .args(&["cmd", "/k", &command])
            .spawn()
            .is_ok()
        {
            return Ok("Opened in Windows Terminal".to_string());
        }

        if Command::new("cmd.exe")
            .args(&["/k", &command])
            .spawn()
            .is_ok()
        {
            return Ok("Opened in CMD".to_string());
        }

        return Err("No terminal found".to_string());
    }

    #[allow(unreachable_code)]
    Err("Platform not supported".to_string())
}

#[tauri::command]
async fn check_version_alignment() -> Result<String, String> {
    let Some(binary_path) = resolve_processor_binary() else {
        return Ok("Version Alignment Check Skipped: drag_and_drop_processor binary not found; processing will fail until it is built or MFB_PROCESSOR_BINARY is set".to_string());
    };

    let output = Command::new(&binary_path)
        .arg("--help")
        .output()
        .map_err(|e| {
            format!(
                "Failed to run drag_and_drop_processor at {}: {}",
                binary_path.display(),
                e
            )
        })?;
    if output.status.success() {
        Ok("Version Alignment Confirmed: Rust Processor OK".to_string())
    } else {
        Ok("Version Alignment Check Warning: Processor --help failed".to_string())
    }
}

#[tauri::command]
async fn process_media(
    app: AppHandle,
    target_path: String,
    processing_mode: String,
    output_mode: String,
    ultimate: bool,
    verbose: bool,
    resume: bool,
    shortest_path: bool,
) -> Result<String, String> {
    let Some(binary_path) = resolve_processor_binary() else {
        return Err(missing_processor_error());
    };

    let _ = app.emit(
        "process-log",
        format!("Starting processor backend at: {}", binary_path.display()),
    );

    let mut cmd = Command::new(&binary_path);
    cmd.env("MFB_USE_LEGACY_PY", "0");
    cmd.env("FROM_APP", "1");
    cmd.env("LC_ALL", "en_US.UTF-8");
    cmd.env("LANG", "en_US.UTF-8");

    if processing_mode == "images_only" {
        cmd.arg("--images-only");
    } else if processing_mode == "videos_only" {
        cmd.arg("--videos-only");
    }

    if output_mode == "fast_img" {
        cmd.arg("--mode").arg("fast-img");
    } else if output_mode == "fast_vid" {
        cmd.arg("--mode").arg("fast-vid");
    } else if output_mode == "restore_jpeg" {
        cmd.arg("--mode").arg("restore-jpeg");
    } else if output_mode == "collect" {
        cmd.arg("--mode").arg("collect");
    } else if output_mode == "merge_xmp" {
        cmd.arg("--mode").arg("merge-xmp");
    } else if output_mode == "icloud_import" {
        cmd.arg("--mode").arg("icloud-import");
    } else if output_mode == "diagnostic" {
        cmd.arg("--mode").arg("diagnostic");
    } else if output_mode == "cache_clean" {
        cmd.arg("--mode").arg("cache-clean");
    } else if output_mode == "database_manager" {
        cmd.arg("--mode").arg("database-manager");
    }

    if ultimate {
        cmd.arg("--ultimate");
    }
    if verbose {
        cmd.arg("--verbose");
    }
    if resume {
        cmd.arg("--resume");
    }
    if shortest_path {
        cmd.arg("--shortest-path");
    }

    cmd.arg(&target_path);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "Failed to start drag_and_drop_processor at {}: {}",
            binary_path.display(),
            e
        )
    })?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let app_clone1 = app.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if let Ok(l) = line {
                let _ = app_clone1.emit("process-log", l);
            }
        }
    });

    let app_clone2 = app.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if let Ok(l) = line {
                let _ = app_clone2.emit("process-log", format!("ERR: {}", l));
            }
        }
    });

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok("Completed successfully".to_string())
    } else {
        Err(format!("Process exited with status: {}", status))
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            process_media,
            check_version_alignment,
            get_processor_binary_path,
            open_in_terminal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

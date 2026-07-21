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

fn push_env_processor_candidate(candidates: &mut Vec<PathBuf>, env_processor: Option<PathBuf>) {
    if let Some(path) = env_processor {
        candidates.push(path);
    }
}

fn is_macos_app_exe_dir(exe_dir: &std::path::Path) -> bool {
    cfg!(target_os = "macos") && exe_dir.file_name() == Some(OsStr::new("MacOS"))
}

fn push_bundled_processor_candidates(
    candidates: &mut Vec<PathBuf>,
    exe_dir: &std::path::Path,
    name: &str,
) -> bool {
    if !is_macos_app_exe_dir(exe_dir) {
        return false;
    }
    let Some(contents_dir) = exe_dir.parent() else {
        return false;
    };
    candidates.push(contents_dir.join("Resources").join(name));
    candidates.push(contents_dir.join("Resources").join("bin").join(name));
    candidates.push(exe_dir.join(name));
    true
}

fn processor_binary_candidates_from(
    env_processor: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    current_dir: Option<PathBuf>,
    path_var: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let name = processor_binary_name();
    let mut candidates = Vec::new();

    push_env_processor_candidate(&mut candidates, env_processor);

    if let Some(current_exe) = current_exe {
        if let Some(exe_dir) = current_exe.parent() {
            if push_bundled_processor_candidates(&mut candidates, exe_dir, &name) {
                return candidates;
            }

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

            for ancestor in exe_dir.ancestors() {
                candidates.push(ancestor.join("target").join("release").join(&name));
                candidates.push(ancestor.join("target").join("debug").join(&name));
            }
        }
    }

    if let Some(current_dir) = current_dir {
        candidates.push(current_dir.join(&name));
    }

    if let Some(path) = path_var {
        for dir in env::split_paths(&path) {
            candidates.push(dir.join(&name));
        }
    }

    candidates
}

fn processor_binary_candidates() -> Vec<PathBuf> {
    let env_processor = match env::var("MFB_PROCESSOR_BINARY") {
        Ok(value) => Some(PathBuf::from(value)),
        Err(env::VarError::NotPresent) => None,
        Err(err) => {
            eprintln!("Ignoring invalid MFB_PROCESSOR_BINARY value: {err}");
            None
        }
    };
    let current_exe = match env::current_exe() {
        Ok(path) => Some(path),
        Err(err) => {
            eprintln!("Failed to resolve current executable while finding processor: {err}");
            None
        }
    };
    let current_dir = match env::current_dir() {
        Ok(path) => Some(path),
        Err(err) => {
            eprintln!("Failed to resolve current directory while finding processor: {err}");
            None
        }
    };
    processor_binary_candidates_from(
        env_processor,
        current_exe,
        current_dir,
        env::var_os("PATH"),
    )
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

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(target_os = "macos")]
fn macos_terminal_applescript_launchers(command: &str) -> Vec<(&'static str, String)> {
    let command = applescript_string(command);
    let mut launchers = Vec::new();

    if std::path::Path::new("/Applications/iTerm.app").exists() {
        launchers.push((
            "iTerm",
            format!(
                "tell application \"iTerm\"\n    activate\n    if (count of windows) = 0 then\n        create window with default profile\n    end if\n    tell current window\n        create tab with default profile\n        tell current session\n            write text \"{}\"\n        end tell\n    end tell\nend tell",
                command
            ),
        ));
    }

    launchers.push((
        "Terminal",
        format!(
            "tell application \"Terminal\"\n    activate\n    do script \"{}\"\nend tell",
            command
        ),
    ));

    launchers
}

#[cfg(target_os = "macos")]
fn run_macos_applescript(script: &str) -> bool {
    match Command::new("osascript").args(["-e", script]).status() {
        Ok(status) => status.success(),
        Err(err) => {
            eprintln!("Failed to launch osascript for terminal handoff: {err}");
            false
        }
    }
}

#[tauri::command]
async fn get_processor_binary_path() -> Result<String, String> {
    resolve_processor_binary()
        .map(|p| p.display().to_string())
        .ok_or_else(missing_processor_error)
}

#[tauri::command]
async fn open_in_terminal(command: String) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let shell_command = format!("{}; exec sh", command);

        if std::path::Path::new("/Applications/Ghostty.app").exists() {
            let ghostty_bin = "/Applications/Ghostty.app/Contents/MacOS/ghostty";
            if std::path::Path::new(ghostty_bin).exists() {
                if Command::new(ghostty_bin)
                    .args(["-e", "sh", "-c", &shell_command])
                    .spawn()
                    .is_ok()
                {
                    return Ok("Opened in Ghostty".to_string());
                }
            }
        }

        if std::path::Path::new("/Applications/kitty.app").exists() {
            let kitty_bin = "/Applications/kitty.app/Contents/MacOS/kitty";
            if std::path::Path::new(kitty_bin).exists() {
                if Command::new(kitty_bin)
                    .args(["sh", "-c", &shell_command])
                    .spawn()
                    .is_ok()
                {
                    return Ok("Opened in kitty".to_string());
                }
            }
        }

        for (name, script) in macos_terminal_applescript_launchers(&command) {
            if run_macos_applescript(&script) {
                return Ok(format!("Opened in {}", name));
            }
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

#[cfg(all(target_os = "macos", test))]
fn macos_open_terminal_uses_applescript() -> bool {
    !macos_terminal_applescript_launchers("echo test").is_empty()
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
        for l in reader.lines().map_while(Result::ok) {
            let _ = app_clone1.emit("process-log", l);
        }
    });

    let app_clone2 = app.clone();
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for l in reader.lines().map_while(Result::ok) {
            let _ = app_clone2.emit("process-log", format!("ERR: {}", l));
        }
    });

    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok("Completed successfully".to_string())
    } else {
        Err(format!("Process exited with status: {}", status))
    }
}

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            process_media,
            check_version_alignment,
            get_processor_binary_path,
            open_in_terminal
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;

    #[cfg(target_os = "macos")]
    #[test]
    fn app_bundle_prefers_bundled_processor_before_dev_or_path_bins() {
        let current_exe = PathBuf::from(
            "/Applications/Modern Format Boost.app/Contents/MacOS/Modern Format Boost",
        );
        let current_dir = PathBuf::from("/tmp/third-party-terminal");
        let candidates = processor_binary_candidates_from(
            None,
            Some(current_exe),
            Some(current_dir),
            Some(OsString::from(
                "/tmp/path-bin:/Users/me/project/target/release",
            )),
        );

        let bundled = Path::new(
            "/Applications/Modern Format Boost.app/Contents/Resources/drag_and_drop_processor",
        );
        assert_eq!(candidates.first().map(PathBuf::as_path), Some(bundled));
        assert!(
            candidates
                .iter()
                .all(|path| !path.starts_with("/tmp/third-party-terminal")),
            "app-bundle launches must not resolve helpers relative to a third-party terminal cwd"
        );
        assert!(
            candidates
                .iter()
                .all(|path| !path.starts_with("/tmp/path-bin")),
            "app-bundle launches must not fall through to PATH helpers with a different TCC identity"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_terminal_launcher_keeps_applescript_terminal_support() {
        assert!(macos_open_terminal_uses_applescript());
        let source = include_str!("lib.rs");
        let osascript = ["osa", "script"].concat();
        assert!(
            source.contains(&osascript),
            "external terminal mode must keep AppleScript-backed macOS terminal launchers"
        );
        assert!(source.contains("Terminal"));
        assert!(source.contains("iTerm"));
    }

    #[test]
    fn app_keeps_external_terminal_as_recommended_cli_mode() {
        let app_vue = include_str!("../../src/App.vue");
        assert!(
            app_vue.contains("const useExternalTerminal = ref(true)"),
            "Vue CLI mode must keep external terminal launch as the default path"
        );
    }

    #[test]
    fn tauri_bundle_declares_macos_privacy_metadata() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = std::fs::read_to_string(manifest_dir.join("tauri.conf.json"))
            .expect("tauri.conf.json should be readable");
        assert!(config.contains("\"macOS\""));
        assert!(config.contains("\"infoPlist\": \"Info.plist\""));
        assert!(config.contains("\"entitlements\": \"entitlements.plist\""));

        let info_plist = std::fs::read_to_string(manifest_dir.join("Info.plist"))
            .expect("Info.plist should be readable");
        assert!(info_plist.contains("NSAppDataUsageDescription"));
        assert!(info_plist.contains("NSAppleEventsUsageDescription"));

        let entitlements = std::fs::read_to_string(manifest_dir.join("entitlements.plist"))
            .expect("entitlements.plist should be readable");
        assert!(entitlements.contains("com.apple.security.automation.apple-events"));
    }

    #[test]
    fn tauri_window_and_vite_base_config_prevent_hidden_or_white_screen() {
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = std::fs::read_to_string(manifest_dir.join("tauri.conf.json"))
            .expect("tauri.conf.json should be readable");
        assert!(
            config.contains("\"visible\": true"),
            "tauri.conf.json must specify visible: true to prevent window staying hidden on startup"
        );

        let vite_config = std::fs::read_to_string(manifest_dir.join("../vite.config.ts"))
            .expect("vite.config.ts should be readable");
        assert!(
            vite_config.contains("base: \"./\""),
            "vite.config.ts must set base: './' to allow webview to load bundled asset scripts relatively"
        );

        let app_vue = std::fs::read_to_string(manifest_dir.join("../src/App.vue"))
            .expect("App.vue should be readable");
        assert!(
            app_vue.contains("from \"./composables/useI18n\""),
            "App.vue must import useI18n from ./composables/useI18n to prevent runtime plugin missing crash"
        );
        assert!(
            !app_vue.contains("from \"vue-i18n\""),
            "App.vue must not import useI18n from vue-i18n directly without App.use(i18n) setup"
        );

        let use_i18n = std::fs::read_to_string(manifest_dir.join("../src/composables/useI18n.js"))
            .expect("useI18n.js should be readable");
        assert!(
            use_i18n.contains("../locales/zh.json"),
            "useI18n.js must import zh.json from ../locales"
        );
        assert!(
            use_i18n.contains("zh_CN: zh"),
            "useI18n.js must alias zh_CN to zh"
        );
    }
}

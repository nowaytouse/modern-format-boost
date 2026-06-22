//! Watch mode for filesystem event monitoring.
//! Mirrors `watch_video_route_changes()` from drag_and_drop_processor.py.

use anyhow::Result;
use notify::event::{CreateKind, ModifyKind, RemoveKind};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};

fn route_modified_after_optimized(route: &Path, optimized_root: &Path) -> bool {
    let route_meta = match route.metadata() {
        Ok(meta) => meta,
        Err(err) => {
            eprintln!("[WATCH] route metadata failed ({}): {err}", route.display());
            return false;
        }
    };
    let route_modified = match route_meta.modified() {
        Ok(modified) => modified,
        Err(err) => {
            eprintln!(
                "[WATCH] route modified time failed ({}): {err}",
                route.display()
            );
            return false;
        }
    };
    let Some(parent) = optimized_root.parent() else {
        return false;
    };
    let opt_meta = match parent.metadata() {
        Ok(meta) => meta,
        Err(err) => {
            eprintln!(
                "[WATCH] optimized parent metadata failed ({}): {err}",
                parent.display()
            );
            return false;
        }
    };
    match opt_meta.modified() {
        Ok(opt_modified) => route_modified > opt_modified,
        Err(err) => {
            eprintln!(
                "[WATCH] optimized modified time failed ({}): {err}",
                parent.display()
            );
            false
        }
    }
}

/// Check if path needs reprocessing based on modification time.
pub fn needs_reprocessing(route: &Path, optimized_root: &Path) -> bool {
    route_modified_after_optimized(route, optimized_root)
}

/// Get parent directory of video-route (used for watch mode).
/// Finds the "videos" directory in the path hierarchy and returns it.
pub fn get_route_parent(route: &Path) -> Option<PathBuf> {
    let mut current = route.parent()?;
    while current.file_name()?.to_string_lossy() != "videos" {
        current = current.parent()?;
    }
    Some(current.to_path_buf())
}

/// Watch directory for filesystem changes with debounce.
/// Mirrors Python watchdog-based watch_video_route_changes().
pub fn watch_directory_with_debounce<F>(
    root: &Path,
    debounce_ms: u64,
    mut on_event: F,
) -> Result<()>
where
    F: FnMut(notify::Event),
{
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(root, RecursiveMode::Recursive)?;

    let debounce = Duration::from_millis(debounce_ms);

    while let Ok(res) = rx.recv() {
        let Ok(event) = res else {
            continue;
        };
        if !is_relevant_watch_event(&event) {
            continue;
        }
        let mut pending = Some(event);
        let mut deadline = Some(Instant::now() + debounce);

        while let Some(target) = deadline {
            let remaining = target.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(Ok(next)) if is_relevant_watch_event(&next) => {
                    pending = Some(next);
                    deadline = Some(Instant::now() + debounce);
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }

        if let Some(event) = pending.take() {
            on_event(event);
        }
    }
    Ok(())
}

fn is_relevant_watch_event(event: &Event) -> bool {
    match event.kind {
        EventKind::Any
        | EventKind::Create(CreateKind::Any)
        | EventKind::Modify(ModifyKind::Any)
        | EventKind::Remove(RemoveKind::Any) => true,
        EventKind::Create(CreateKind::File)
        | EventKind::Modify(ModifyKind::Data(_))
        | EventKind::Modify(ModifyKind::Name(_))
        | EventKind::Remove(RemoveKind::File) => true,
        _ => false,
    }
}

/// Video file extensions that trigger reprocessing.
pub const VIDEO_TRIGGER_EXTS: &[&str] = &[
    ".jpg", ".jpeg", ".png", ".gif", ".heic", ".mp4", ".mov", ".mkv",
];

/// Check if extension should trigger watch processing.
pub fn is_watch_trigger_ext(ext: &str) -> bool {
    VIDEO_TRIGGER_EXTS.contains(&ext.to_lowercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_route_parent_finds_videos_dir() {
        let path = Path::new("/Users/test/videos/drag_and_drop/2024-01-15/route");
        let parent = get_route_parent(path);
        assert_eq!(parent, Some(PathBuf::from("/Users/test/videos")));
    }

    #[test]
    fn test_watch_trigger_extensions() {
        assert!(is_watch_trigger_ext(".jpg"));
        assert!(is_watch_trigger_ext(".MP4"));
        assert!(!is_watch_trigger_ext(".txt"));
    }
}

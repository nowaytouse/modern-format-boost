//! Private, no-overwrite publication shared by verified SQLite snapshots.

use anyhow::{Context, Result, ensure};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn require_new_destination(destination: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut path = destination.as_os_str().to_os_string();
        path.push(suffix);
        match fs::symlink_metadata(Path::new(&path)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect backup destination"),
            Ok(_) => anyhow::bail!(
                "backup destination or SQLite sidecar already exists; nothing overwritten"
            ),
        }
    }
    Ok(())
}

pub fn resolve_destination(source: &Path, destination: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(destination)?;
    let parent = absolute
        .parent()
        .context("backup destination has no parent")?
        .canonicalize()?;
    let name = absolute
        .file_name()
        .context("backup destination has no filename")?;
    ensure!(
        !parent.starts_with(source.parent().context("backup source has no parent")?),
        "backup destination must be outside the active database directory"
    );
    require_private_directory(&parent)?;
    let destination = parent.join(name);
    require_new_destination(&destination)?;
    Ok(destination)
}

fn require_private_directory(parent: &Path) -> Result<()> {
    for ancestor in parent.ancestors() {
        ensure!(
            !ancestor.join(".git").try_exists()?,
            "backup destination must be outside Git workspaces to protect private state"
        );
    }
    Ok(())
}

pub fn synchronize_private_snapshot(backup: &Path) -> Result<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(backup, fs::Permissions::from_mode(0o600))?;
    }
    let file = fs::OpenOptions::new().read(true).write(true).open(backup)?;
    let bytes = file.metadata()?.len();
    ensure!(bytes > 0, "SQLite backup is empty; not published");
    file.sync_all()?;
    Ok(bytes)
}

pub fn publish_snapshot(backup: &Path, destination: &Path) -> Result<()> {
    require_private_directory(
        destination
            .parent()
            .context("backup destination has no parent")?,
    )?;
    require_new_destination(destination)?;
    // shortcut: atomic publication requires hard-link support, add native no-replace rename if supported filesystems require it.
    fs::hard_link(backup, destination)
        .context("publish verified backup without overwriting an existing destination")?;
    #[cfg(unix)]
    fs::File::open(
        destination
            .parent()
            .context("backup destination has no parent")?,
    )?
    .sync_all()
    .context("backup published but destination directory could not be synchronized")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_rechecks_late_git_marker_and_preserves_existing_targets() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::create_dir(&source)?;
        fs::create_dir(&target)?;
        let staged = source.join("snapshot.sqlite");
        fs::write(&staged, b"synthetic snapshot")?;
        let destination = resolve_destination(&staged, &target.join("copy.sqlite"))?;
        fs::write(target.join(".git"), b"synthetic worktree marker")?;
        assert!(publish_snapshot(&staged, &destination).is_err());
        assert!(!destination.exists());
        fs::remove_file(target.join(".git"))?;
        fs::write(&destination, b"existing target")?;
        assert!(publish_snapshot(&staged, &destination).is_err());
        assert_eq!(fs::read(&destination)?, b"existing target");
        assert_eq!(fs::read(&staged)?, b"synthetic snapshot");
        fs::remove_file(&destination)?;
        publish_snapshot(&staged, &destination)?;
        assert_eq!(fs::read(&destination)?, b"synthetic snapshot");
        Ok(())
    }
}

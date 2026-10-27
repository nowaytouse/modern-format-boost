//! Per-exploration identity for direct cjxl candidates; no persistent cache state.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

type Environment = BTreeMap<OsString, OsString>;

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    length: u64,
    modified: SystemTime,
    readonly: bool,
    #[cfg(unix)]
    unix: (u64, u64, i64, i64, u32),
}

impl FileStamp {
    fn new(metadata: &fs::Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified()?,
            readonly: metadata.permissions().readonly(),
            #[cfg(unix)]
            unix: (
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
                metadata.mode(),
            ),
        })
    }

    fn append(&self, key: &mut Vec<u8>) {
        field(key, &self.length.to_le_bytes());
        let (before_epoch, duration) = match self.modified.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(duration) => (false, duration),
            Err(error) => (true, error.duration()),
        };
        field(key, &[u8::from(before_epoch)]);
        field(key, &duration.as_secs().to_le_bytes());
        field(key, &duration.subsec_nanos().to_le_bytes());
        field(key, &[u8::from(self.readonly)]);
        #[cfg(unix)]
        {
            field(key, &self.unix.0.to_le_bytes());
            field(key, &self.unix.1.to_le_bytes());
            field(key, &self.unix.2.to_le_bytes());
            field(key, &self.unix.3.to_le_bytes());
            field(key, &self.unix.4.to_le_bytes());
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    resolved: PathBuf,
    link: Option<PathBuf>,
    entry: FileStamp,
    target: FileStamp,
}

impl FileState {
    fn read(path: &Path) -> io::Result<Self> {
        let entry = fs::symlink_metadata(path)?;
        if !entry.is_file() && !entry.file_type().is_symlink() {
            return Err(invalid(
                "JXL probe input is not a regular file or file symlink",
            ));
        }
        let resolved = fs::canonicalize(path)?;
        let target = fs::metadata(&resolved)?;
        if !target.is_file() {
            return Err(invalid(
                "JXL probe input does not resolve to a regular file",
            ));
        }
        Ok(Self {
            resolved,
            link: entry
                .file_type()
                .is_symlink()
                .then(|| fs::read_link(path))
                .transpose()?,
            entry: FileStamp::new(&entry)?,
            target: FileStamp::new(&target)?,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BoundFile {
    path: PathBuf,
    state: FileState,
    hash: String,
}

impl BoundFile {
    fn read(path: &Path) -> io::Result<Self> {
        let before = FileState::read(path)?;
        let hash = foundation::common_utils::calculate_blake3_hash(&before.resolved)
            .map_err(io::Error::other)?;
        let after = FileState::read(path)?;
        if before != after {
            return Err(invalid("JXL probe input changed while hashing"));
        }
        Ok(Self {
            path: path.to_path_buf(),
            state: before,
            hash,
        })
    }

    fn validate(&self) -> io::Result<()> {
        if Self::read(&self.path)? != *self {
            return Err(invalid(
                "JXL probe input or encoder changed during exploration",
            ));
        }
        Ok(())
    }

    fn append(&self, key: &mut Vec<u8>) {
        field(key, self.path.as_os_str().as_encoded_bytes());
        field(key, self.state.resolved.as_os_str().as_encoded_bytes());
        field(key, &[u8::from(self.state.link.is_some())]);
        if let Some(link) = &self.state.link {
            field(key, link.as_os_str().as_encoded_bytes());
        }
        self.state.entry.append(key);
        self.state.target.append(key);
        field(key, self.hash.as_bytes());
    }
}

pub(super) struct JxlProbeIdentity {
    source: BoundFile,
    actual: Option<BoundFile>,
    icc: Option<BoundFile>,
    sidecar: Option<BoundFile>,
    encoder: BoundFile,
    environment: Environment,
    cwd: PathBuf,
}

impl JxlProbeIdentity {
    /// Accept only fresh direct-builder commands that inherit their parent environment, not `env_clear` commands.
    pub(super) fn new(
        input: &Path,
        actual_input: &Path,
        icc: Option<&Path>,
        command: &Command,
    ) -> io::Result<Self> {
        let cwd = fs::canonicalize(
            foundation::media_conversion_gate::delivery_cwd_or_err("JXL probe identity")
                .map_err(io::Error::other)?,
        )?;
        let environment = std::env::vars_os().collect();
        let source = BoundFile::read(&absolute(input, &cwd))?;
        let actual_path = absolute(actual_input, &cwd);
        let actual = if actual_path.as_os_str() == source.path.as_os_str() {
            None
        } else {
            Some(BoundFile::read(&actual_path)?)
        };
        let icc = icc
            .map(|path| BoundFile::read(&absolute(path, &cwd)))
            .transpose()?;
        let sidecar = selected_sidecar(&source.path)?;
        let effective = effective_environment(&environment, command);
        let command_cwd = effective_cwd(&cwd, command)?;
        let encoder = BoundFile::read(&resolve_program(command, &effective, &command_cwd)?)?;
        let identity = Self {
            source,
            actual,
            icc,
            sidecar,
            encoder,
            environment,
            cwd,
        };
        identity.validate_association(command)?;
        Ok(identity)
    }

    /// Prepare a fresh inherited-environment command once; do not prepare it again or change it before execution.
    pub(super) fn prepare_and_key(
        &self,
        command: &mut Command,
        output: &Path,
    ) -> io::Result<String> {
        self.validate(command)?;
        let environment = effective_environment(&self.environment, command);
        let cwd = effective_cwd(&self.cwd, command)?;
        let output_arg = foundation::safe_path_arg(output);
        let args = command
            .get_args()
            .map(OsStr::to_os_string)
            .collect::<Vec<_>>();
        if args
            .iter()
            .filter(|arg| arg.as_os_str() == OsStr::new(output_arg.as_ref()))
            .count()
            != 1
        {
            return Err(invalid(
                "JXL probe output must match exactly one complete command argument",
            ));
        }
        let mut key = Vec::new();
        field(&mut key, b"mfb-jxl-probe-v1");
        self.source.append(&mut key);
        for file in [&self.actual, &self.icc, &self.sidecar] {
            field(&mut key, &[u8::from(file.is_some())]);
            if let Some(file) = file {
                file.append(&mut key);
            }
        }
        self.encoder.append(&mut key);
        field(
            &mut key,
            self.encoder.state.resolved.as_os_str().as_encoded_bytes(),
        );
        field(&mut key, cwd.as_os_str().as_encoded_bytes());
        for arg in &args {
            let transport_output = arg.as_os_str() == OsStr::new(output_arg.as_ref());
            field(
                &mut key,
                if transport_output {
                    b"output"
                } else {
                    b"argument"
                },
            );
            if !transport_output {
                field(&mut key, arg.as_encoded_bytes());
            }
        }
        field(&mut key, b"environment");
        for (name, value) in &environment {
            field(&mut key, name.as_encoded_bytes());
            field(&mut key, value.as_encoded_bytes());
        }
        command.env_clear().envs(&environment).current_dir(cwd);
        Ok(foundation::common_utils::blake3_hex(&key))
    }

    pub(super) fn validate(&self, command: &Command) -> io::Result<()> {
        self.source.validate()?;
        if let Some(actual) = &self.actual {
            actual.validate()?;
        }
        if let Some(icc) = &self.icc {
            icc.validate()?;
        }
        if let Some(sidecar) = &self.sidecar {
            sidecar.validate()?;
        }
        let environment = effective_environment(&self.environment, command);
        let cwd = effective_cwd(&self.cwd, command)?;
        if resolve_program(command, &environment, &cwd)? != self.encoder.path {
            return Err(invalid("JXL probe encoder resolution changed"));
        }
        self.encoder.validate()
    }

    /// Direct pixel probes do not consume sidecars; rediscover their association at search boundaries.
    pub(super) fn validate_association(&self, command: &Command) -> io::Result<()> {
        self.validate(command)?;
        if selected_sidecar(&self.source.path)? != self.sidecar {
            return Err(invalid(
                "JXL probe source XMP association or contents changed",
            ));
        }
        Ok(())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&value.len().to_le_bytes());
    bytes.extend_from_slice(value);
}

fn absolute(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn effective_cwd(parent: &Path, command: &Command) -> io::Result<PathBuf> {
    fs::canonicalize(
        command
            .get_current_dir()
            .map_or_else(|| parent.to_path_buf(), |path| absolute(path, parent)),
    )
}

fn effective_environment(parent: &Environment, command: &Command) -> Environment {
    let mut effective = parent.clone();
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            effective.insert(name.to_os_string(), value.to_os_string());
        } else {
            effective.remove(name);
        }
    }
    effective
}

fn resolve_program(
    command: &Command,
    environment: &Environment,
    cwd: &Path,
) -> io::Result<PathBuf> {
    let program = Path::new(command.get_program());
    if program.is_absolute() || program.components().count() > 1 {
        let path = absolute(program, cwd);
        fs::metadata(&path)?;
        return Ok(path);
    }
    let path = environment
        .get(OsStr::new("PATH"))
        .ok_or_else(|| invalid("JXL probe encoder has no explicit path or effective PATH"))?;
    for directory in std::env::split_paths(path) {
        let candidate = absolute(&directory, cwd).join(program);
        match fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                return Ok(candidate);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "JXL probe encoder cannot be resolved in its frozen PATH",
    ))
}

fn selected_sidecar(source: &Path) -> io::Result<Option<BoundFile>> {
    foundation::metadata::find_xmp_sidecar(source)?
        .as_deref()
        .map(BoundFile::read)
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(tool: &Path, source: &Path, output: &Path, distance: &str) -> Command {
        let mut command = Command::new(tool);
        command
            .arg(foundation::safe_path_arg(source).as_ref())
            .arg(foundation::safe_path_arg(output).as_ref())
            .args(["-d", distance, "-e", "10"]);
        command
    }

    #[test]
    fn identity_normalizes_only_output_and_freezes_execution_context() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source.png");
        let tool = root.path().join("cjxl");
        fs::write(&source, b"source")?;
        fs::write(&tool, b"encoder")?;
        let output = root.path().join("one.jxl");
        let other_output = root.path().join("two.jxl");
        let mut first = command(&tool, &source, &output, "0.125");
        first
            .env("MFB_PROBE_TEST", "private")
            .env_remove("MFB_PROBE_REMOVED");
        let identity = JxlProbeIdentity::new(&source, &source, None, &first)?;
        assert!(
            identity.actual.is_none(),
            "same source path must not be hashed twice"
        );
        let key = identity.prepare_and_key(&mut first, &output)?;
        let mut same = command(&tool, &source, &other_output, "0.125");
        same.env("MFB_PROBE_TEST", "private")
            .env_remove("MFB_PROBE_REMOVED");
        assert_eq!(key, identity.prepare_and_key(&mut same, &other_output)?);
        let actual_env = first
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| (name.to_os_string(), value.to_os_string()))
            })
            .collect::<Environment>();
        assert_eq!(
            actual_env.get(OsStr::new("MFB_PROBE_TEST")),
            Some(&OsString::from("private"))
        );
        assert!(!actual_env.contains_key(OsStr::new("MFB_PROBE_REMOVED")));
        assert!(first.get_current_dir().is_some_and(Path::is_absolute));
        for kind in 0..4 {
            let mut changed = command(
                &tool,
                &source,
                &output,
                if kind == 0 { "0.126" } else { "0.125" },
            );
            changed
                .env("MFB_PROBE_TEST", "private")
                .env_remove("MFB_PROBE_REMOVED");
            match kind {
                1 => {
                    changed.arg("--effort=8");
                }
                2 => {
                    changed.env("MFB_PROBE_TEST", "different");
                }
                3 => {
                    changed.current_dir(root.path());
                }
                _ => {}
            }
            assert_ne!(key, identity.prepare_and_key(&mut changed, &output)?);
        }
        let mut ambiguous = command(&tool, &source, &output, "0.125");
        ambiguous.arg(foundation::safe_path_arg(&output).as_ref());
        assert!(identity.prepare_and_key(&mut ambiguous, &output).is_err());
        Ok(())
    }

    #[test]
    fn identity_refuses_changed_files_sidecar_association_and_read_failures() -> io::Result<()> {
        for kind in 0..6 {
            let root = tempfile::tempdir()?;
            let source = root.path().join("source.png");
            let actual = root.path().join("prepared.png");
            let icc = root.path().join("color.icc");
            let tool = root.path().join("cjxl");
            for path in [&source, &actual, &icc, &tool] {
                fs::write(path, b"initial")?;
            }
            let output = root.path().join("out.jxl");
            let command = command(&tool, &actual, &output, "0.125");
            let identity = JxlProbeIdentity::new(&source, &actual, Some(&icc), &command)?;
            match kind {
                0 => fs::write(&source, b"changed")?,
                1 => fs::write(&actual, b"changed")?,
                2 => fs::write(&icc, b"changed")?,
                3 => fs::write(&tool, b"changed")?,
                4 => fs::write(source.with_extension("xmp"), b"<xmp/>")?,
                _ => fs::remove_file(&actual)?,
            }
            if kind == 4 {
                identity.validate(&command)?;
            } else {
                assert!(identity.validate(&command).is_err());
            }
            assert!(identity.validate_association(&command).is_err());
        }
        Ok(())
    }

    #[test]
    fn identity_refuses_changed_selected_sidecar_and_retargeted_symlinks() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source.png");
        let sidecar = source.with_extension("xmp");
        let tool = root.path().join("cjxl");
        fs::write(&source, b"source")?;
        fs::write(&sidecar, b"<xmp/>")?;
        fs::write(&tool, b"encoder")?;
        let output = root.path().join("out.jxl");
        let command = command(&tool, &source, &output, "0.125");
        let identity = JxlProbeIdentity::new(&source, &source, None, &command)?;
        fs::write(&sidecar, b"<changed/>")?;
        assert!(identity.validate(&command).is_err());
        #[cfg(unix)]
        {
            let link = root.path().join("linked.png");
            let other = root.path().join("other.png");
            fs::write(&other, b"source")?;
            std::os::unix::fs::symlink(&source, &link)?;
            let identity = JxlProbeIdentity::new(&link, &link, None, &command)?;
            identity.validate(&command)?;
            fs::remove_file(&link)?;
            std::os::unix::fs::symlink(&other, &link)?;
            assert!(identity.validate(&command).is_err());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn prepared_command_executes_with_explicit_environment_and_working_directory() -> io::Result<()>
    {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source.png");
        fs::write(&source, b"source")?;
        let output = root.path().join("out.jxl");
        let cwd = fs::canonicalize(root.path())?;
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "test \"$MFB_JXL_IDENTITY_VALUE\" = frozen && test -z \"${HOME+x}\" && test \"$PWD\" = \"$1\""])
            .arg(foundation::safe_path_arg(&output).as_ref()).arg(&cwd)
            .env("MFB_JXL_IDENTITY_VALUE", "frozen").env_remove("HOME").current_dir(&cwd);
        let identity = JxlProbeIdentity::new(&source, &source, None, &command)?;
        identity.prepare_and_key(&mut command, &output)?;
        let result = command.output()?;
        assert!(
            result.status.success(),
            "frozen command context was not applied"
        );
        Ok(())
    }
}

//! Private, content-addressed external tool runtimes. These are recovery state,
//! not disposable media cache; no automatic deletion is performed here.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const MANIFEST_ENV: &str = "MFB_TOOL_SNAPSHOT_MANIFEST";

/// Shared wire contract for early runtime admission and failure observations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotEvent {
    pub schema_version: u8,
    pub pipeline: String,
    pub state: String,
    pub boundary: String,
    pub identity: Option<String>,
    pub manifest_path: Option<PathBuf>,
    pub tool_names: Vec<String>,
    pub observed_tool_names: Vec<String>,
    pub cause_code: Option<String>,
    pub tool: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub manifest_path: PathBuf,
    pub tools: BTreeMap<String, PathBuf>,
    pub identity: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u8,
    kind: String,
    source_identity: String,
    system_identity: String,
    selections: BTreeMap<String, Selection>,
    tools: BTreeMap<String, PathBuf>,
    members: BTreeMap<PathBuf, Member>,
    system_dependencies: BTreeSet<PathBuf>,
    external_inputs: BTreeSet<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    requested: PathBuf,
    canonical: PathBuf,
    blake3: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    source: Option<PathBuf>,
    source_blake3: Option<String>,
    blake3: String,
    bytes: u64,
    executable: bool,
}

fn digest(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn relative(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "tool snapshot member must be a nonempty relative path: {}",
        path.display()
    );
    Ok(())
}

fn system_library(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        && (path.starts_with("/usr/lib") || path.starts_with("/System/Library"))
}

fn system_program(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        && (path.starts_with("/usr/bin") || path.starts_with("/bin"))
}

fn read_regular(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect tool runtime member {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "tool runtime member is not a regular file: {}",
        path.display()
    );
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path)?;
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    ensure!(
        before.is_file()
            && before.len() == u64::try_from(bytes.len())?
            && before.len() == after.len()
            && before.modified()? == after.modified()?,
        "tool runtime changed while reading: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "tool runtime changed while reading: {}",
            path.display()
        );
    }
    Ok(bytes)
}

fn manifest_identity(manifest: &Manifest) -> Result<String> {
    Ok(digest(&serde_json::to_vec(manifest)?))
}

fn system_identity() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let version = read_regular(Path::new(
            "/System/Library/CoreServices/SystemVersion.plist",
        ))?;
        Ok(digest(&serde_json::to_vec(&(
            std::env::consts::ARCH,
            digest(&version),
        ))?))
    }
    #[cfg(not(target_os = "macos"))]
    {
        bail!("the protected macOS runtime boundary is unavailable on this platform")
    }
}

#[cfg(target_os = "macos")]
fn guard_loader_environment() -> Result<()> {
    for (key, _) in std::env::vars_os() {
        ensure!(
            !key.as_encoded_bytes().starts_with(b"DYLD_"),
            "external {} prevents a closed tool runtime; no media admitted",
            key.to_string_lossy()
        );
    }
    Ok(())
}

/// Load only a complete published snapshot; source installations are never read.
pub fn load_and_verify(path: &Path) -> Result<Snapshot> {
    #[cfg(target_os = "macos")]
    guard_loader_environment()?;
    let data = read_regular(path)?;
    let manifest: Manifest =
        serde_json::from_slice(&data).context("decode tool runtime manifest")?;
    ensure!(
        manifest.schema_version == 1 && manifest.kind == "mfb_tool_snapshot",
        "unsupported tool runtime manifest"
    );
    ensure!(
        manifest.system_identity == system_identity()?,
        "protected OS runtime identity changed; the existing tool runtime cannot be reused"
    );
    ensure!(
        manifest.source_identity.len() == 64
            && manifest
                .source_identity
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit()),
        "invalid tool runtime source identity"
    );
    ensure!(
        manifest
            .system_dependencies
            .iter()
            .all(|path| system_library(path) || system_program(path)),
        "tool runtime declares an unprotected system boundary"
    );
    let identity = manifest_identity(&manifest)?;
    let root = path
        .parent()
        .context("tool runtime manifest has no parent")?;
    ensure!(
        path.file_name().is_some_and(|name| name == "manifest.json")
            && root.file_name().and_then(|name| name.to_str()) == Some(identity.as_str())
            && root
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "toolchains")
            && fs::symlink_metadata(root)?.is_dir(),
        "tool runtime identity or publication directory does not match"
    );
    let root = root.canonicalize()?;
    for (name, member) in &manifest.members {
        relative(name)?;
        let full = root.join(name);
        ensure!(
            full.canonicalize()?.starts_with(&root),
            "tool runtime member escapes its snapshot"
        );
        let bytes = read_regular(&full)?;
        ensure!(
            u64::try_from(bytes.len())? == member.bytes && digest(&bytes) == member.blake3,
            "tool runtime member content changed: {}",
            name.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            ensure!(
                !member.executable || fs::metadata(&full)?.permissions().mode() & 0o111 != 0,
                "tool runtime executable permission lost: {}",
                name.display()
            );
        }
    }
    let mut tools = BTreeMap::new();
    for (name, target) in &manifest.tools {
        crate::runtime_config::parse_tool_override(&format!("{name}=/unused"))
            .map_err(anyhow::Error::msg)?;
        if target.is_absolute() {
            ensure!(
                system_program(target) && manifest.system_dependencies.contains(target),
                "unbound external tool runtime target"
            );
            tools.insert(name.clone(), target.clone());
        } else {
            relative(target)?;
            ensure!(
                manifest
                    .members
                    .get(target)
                    .is_some_and(|member| member.executable),
                "tool runtime target is not a declared executable"
            );
            tools.insert(name.clone(), root.join(target));
        }
    }
    ensure!(!tools.is_empty(), "empty tool runtime manifest");
    Ok(Snapshot {
        manifest_path: root.join("manifest.json"),
        tools,
        identity,
    })
}

impl Snapshot {
    /// User config and protected OS runtime inputs are deliberately reported separately.
    pub fn external_inputs(&self) -> Result<BTreeSet<String>> {
        let manifest: Manifest = serde_json::from_slice(&read_regular(&self.manifest_path)?)?;
        ensure!(
            manifest_identity(&manifest)? == self.identity,
            "tool runtime manifest changed"
        );
        Ok(manifest.external_inputs)
    }

    /// An inherited runtime cannot silently override a different explicit tool selection.
    pub fn verify_selection(&self, name: &str, requested: &Path) -> Result<()> {
        ensure!(
            self.tools.contains_key(name),
            "tool {name} is absent from the inherited private runtime"
        );
        let manifest: Manifest = serde_json::from_slice(&read_regular(&self.manifest_path)?)?;
        ensure!(
            manifest_identity(&manifest)? == self.identity,
            "tool runtime manifest changed"
        );
        let selection = manifest
            .selections
            .get(name)
            .context("private runtime has no original tool selection")?;
        ensure!(
            requested == selection.requested || requested == selection.canonical,
            "explicit selection for {name} differs from the inherited private runtime"
        );
        Ok(())
    }

    /// Recheck private files, not the mutable system installation.
    pub fn verify(&self) -> Result<()> {
        let current = load_and_verify(&self.manifest_path)?;
        ensure!(
            current.identity == self.identity && current.tools == self.tools,
            "tool runtime reference changed"
        );
        Ok(())
    }
}

/// Capture the selected tools before admitting media processing.
///
/// macOS Mach-O static dependencies and the recognized `ExifTool` distribution
/// are supported. Unknown interpreters/plugins fail before publication.
pub fn prepare(root: &Path, tools: &BTreeMap<String, PathBuf>) -> Result<Snapshot> {
    #[cfg(target_os = "macos")]
    {
        macos::prepare(root, tools)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (root, tools);
        bail!("persistent external tool runtimes are currently supported only on macOS")
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{
        BTreeMap, BTreeSet, Context, Manifest, Member, Path, PathBuf, Result, Selection, Serialize,
        Snapshot, bail, digest, ensure, fs, guard_loader_environment, load_and_verify,
        manifest_identity, read_regular, system_identity, system_library, system_program,
    };
    use goblin::mach::{Mach, MachO, SingleArch};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[derive(Default, Serialize)]
    struct Graph {
        files: BTreeMap<PathBuf, Node>,
        data: BTreeMap<PathBuf, (PathBuf, String)>,
        tools: BTreeMap<String, PathBuf>,
        wrappers: BTreeMap<PathBuf, String>,
        system: BTreeSet<PathBuf>,
        external: BTreeSet<String>,
        system_identity: String,
        selections: BTreeMap<String, Selection>,
        #[serde(skip)]
        visited_contexts: BTreeSet<(PathBuf, PathBuf, Vec<PathBuf>)>,
    }

    #[derive(Serialize)]
    struct Node {
        source_hash: String,
        relative: PathBuf,
        imports: BTreeMap<String, PathBuf>,
        rpaths: BTreeSet<String>,
        dylib: bool,
    }

    pub(super) struct Slice {
        imports: Vec<String>,
        rpaths: Vec<String>,
        dylib: bool,
        dylinker: Option<PathBuf>,
    }

    pub(super) fn slices(bytes: &[u8]) -> Result<Vec<Slice>> {
        let convert = |mach: MachO<'_>, data: &[u8]| -> Result<Slice> {
            use goblin::mach::load_command::CommandVariant;
            ensure!(
                matches!(
                    mach.header.filetype,
                    goblin::mach::header::MH_EXECUTE | goblin::mach::header::MH_DYLIB
                ),
                "tool runtime expects an executable or dylib Mach-O"
            );
            let mut imports = Vec::new();
            let mut dylinker = None;
            for command in &mach.load_commands {
                let string = |offset: u32| -> Result<String> {
                    let offset = usize::try_from(offset)?;
                    let length = command
                        .command
                        .cmdsize()
                        .checked_sub(offset)
                        .context("runtime load string is outside its command")?;
                    let start = command
                        .offset
                        .checked_add(offset)
                        .context("runtime load string offset overflow")?;
                    let end = start
                        .checked_add(length)
                        .context("runtime load string end overflow")?;
                    ensure!(
                        data.get(start..end).is_some_and(|value| value.contains(&0)),
                        "runtime load string is not terminated inside its command"
                    );
                    let strings = goblin::strtab::Strtab::parse(data, start, length, 0)?;
                    let value = strings
                        .get_at(0)
                        .filter(|value| !value.is_empty())
                        .context("runtime load string is empty")?;
                    Ok(value.to_owned())
                };
                match command.command {
                    CommandVariant::LoadDylib(value)
                    | CommandVariant::ReexportDylib(value)
                    | CommandVariant::LoadWeakDylib(value)
                    | CommandVariant::LazyLoadDylib(value)
                    | CommandVariant::LoadUpwardDylib(value) => {
                        imports.push(string(value.dylib.name)?);
                    }
                    CommandVariant::LoadDylinker(value) => {
                        let path = PathBuf::from(string(value.name)?);
                        ensure!(
                            system_library(&path),
                            "tool runtime uses an unsupported external dynamic loader"
                        );
                        ensure!(
                            dylinker.replace(path).is_none(),
                            "tool runtime has duplicate dynamic loaders"
                        );
                    }
                    CommandVariant::DyldEnvironment(_)
                    | CommandVariant::LoadFvmlib(_)
                    | CommandVariant::Fvmfile(_)
                    | CommandVariant::PreboundDylib(_) => bail!(
                        "tool runtime contains unsupported loader environment or legacy library commands"
                    ),
                    _ => {}
                }
            }
            Ok(Slice {
                imports,
                rpaths: mach.rpaths.iter().map(ToString::to_string).collect(),
                dylib: mach.name.is_some(),
                dylinker,
            })
        };
        match Mach::parse(bytes).context("parse external runtime Mach-O")? {
            Mach::Binary(mach) => Ok(vec![convert(mach, bytes)?]),
            Mach::Fat(fat) => {
                ensure!(fat.narches > 0, "empty universal tool runtime");
                let arches = fat.arches()?;
                (0..fat.narches)
                    .map(|index| match fat.get(index)? {
                        SingleArch::MachO(mach) => convert(mach, arches[index].slice(bytes)),
                        SingleArch::Archive(_) => {
                            bail!("archive is not an executable tool runtime")
                        }
                    })
                    .collect()
            }
        }
    }

    fn expand(path: &str, loader: &Path, executable: &Path) -> Result<PathBuf> {
        if let Some(suffix) = path.strip_prefix("@loader_path/") {
            return Ok(loader.join(suffix));
        }
        if let Some(suffix) = path.strip_prefix("@executable_path/") {
            return Ok(executable.join(suffix));
        }
        let path = PathBuf::from(path);
        ensure!(
            path.is_absolute(),
            "unsupported relative runtime search path: {}",
            path.display()
        );
        Ok(path)
    }

    fn dependency(
        import: &str,
        loader: &Path,
        executable: &Path,
        rpaths: &[PathBuf],
    ) -> Result<PathBuf> {
        if let Some(suffix) = import.strip_prefix("@rpath/") {
            for directory in rpaths {
                let candidate = directory.join(suffix);
                if system_library(&candidate) || candidate.try_exists()? {
                    return Ok(candidate);
                }
            }
            bail!("unresolved @rpath tool runtime dependency: {import}");
        }
        expand(import, loader, executable)
    }

    impl Graph {
        fn collect(
            &mut self,
            path: &Path,
            executable: &Path,
            inherited: &[PathBuf],
            visiting: &mut BTreeSet<PathBuf>,
        ) -> Result<PathBuf> {
            let path = path
                .canonicalize()
                .with_context(|| format!("resolve runtime dependency {}", path.display()))?;
            if system_library(&path) {
                self.system.insert(path.clone());
                return Ok(path);
            }
            let mut unique = BTreeSet::new();
            let inherited = inherited
                .iter()
                .filter(|path| unique.insert((*path).clone()))
                .cloned()
                .collect::<Vec<_>>();
            if !self.visited_contexts.insert((
                path.clone(),
                executable.to_path_buf(),
                inherited.clone(),
            )) {
                return Ok(self
                    .files
                    .get(&path)
                    .context("visited runtime member is missing")?
                    .relative
                    .clone());
            }
            let bytes = read_regular(&path)?;
            let source_hash = digest(&bytes);
            let key = digest(&serde_json::to_vec(&(&path, &source_hash))?);
            let relative = PathBuf::from("files").join(key).join(
                path.file_name()
                    .context("runtime dependency has no filename")?,
            );
            let parsed = slices(&bytes)?;
            let node = self.files.entry(path.clone()).or_insert_with(|| Node {
                source_hash: source_hash.clone(),
                relative: relative.clone(),
                imports: BTreeMap::new(),
                rpaths: BTreeSet::new(),
                dylib: parsed.iter().any(|slice| slice.dylib),
            });
            ensure!(
                node.source_hash == source_hash,
                "runtime source changed while collecting dependencies"
            );
            if !visiting.insert(path.clone()) {
                return Ok(relative);
            }
            let loader = path.parent().context("runtime dependency has no parent")?;
            for slice in parsed {
                if let Some(loader) = slice.dylinker {
                    self.system.insert(loader);
                }
                let mut search = slice
                    .rpaths
                    .iter()
                    .map(|entry| expand(entry, loader, executable))
                    .collect::<Result<Vec<_>>>()?;
                search.extend_from_slice(&inherited);
                for import in slice.imports {
                    let resolved = dependency(&import, loader, executable, &search)?;
                    let target = if system_library(&resolved) {
                        self.system.insert(resolved.clone());
                        resolved
                    } else {
                        self.collect(&resolved, executable, &search, visiting)?
                    };
                    let node = self
                        .files
                        .get_mut(&path)
                        .context("runtime graph lost a member")?;
                    if let Some(previous) = node.imports.insert(import.clone(), target.clone()) {
                        ensure!(
                            previous == target,
                            "universal or shared library resolves {import} to inconsistent dependencies"
                        );
                    }
                }
                self.files
                    .get_mut(&path)
                    .context("runtime graph lost a member")?
                    .rpaths
                    .extend(slice.rpaths);
            }
            visiting.remove(&path);
            Ok(relative)
        }

        fn exiftool(&mut self, name: &str, selected: &Path, bytes: &[u8]) -> Result<()> {
            ensure!(
                name == "exiftool",
                "unknown script tool {name}; its interpreter/module closure is not supported"
            );
            let parent = selected
                .parent()
                .context("ExifTool has no installation directory")?;
            let root = parent
                .parent()
                .context("ExifTool has no installation root")?;
            let (script, modules) = if bytes.starts_with(b"#!/bin/bash\n")
                && root.join("libexec/bin/exiftool").is_file()
            {
                let script = root.join("libexec/bin/exiftool");
                let modules = root.join("libexec/lib/perl5");
                let expected = format!(
                    "#!/bin/bash\nPERL5LIB=\"{}\" exec \"{}\"  \"$@\"\n",
                    modules.display(),
                    script.display()
                );
                ensure!(
                    bytes == expected.as_bytes(),
                    "unrecognized ExifTool wrapper; refusing an incomplete runtime snapshot"
                );
                (script, modules)
            } else if bytes.starts_with(b"#!/bin/sh\nbase=${0%/*}\n")
                && parent.join("exiftool.pl").is_file()
            {
                (parent.join("exiftool.pl"), parent.join("lib"))
            } else {
                ensure!(
                    bytes.starts_with(b"#!/usr/bin/perl"),
                    "unrecognized ExifTool script/interpreter"
                );
                (selected.to_path_buf(), parent.join("lib"))
            };
            let script = script.canonicalize()?;
            let script_bytes = read_regular(&script)?;
            let interpreter = std::str::from_utf8(
                script_bytes
                    .split(|byte| *byte == b'\n')
                    .next()
                    .context("empty ExifTool script")?,
            )?
            .strip_prefix("#!")
            .context("ExifTool has no interpreter")?;
            ensure!(
                system_program(Path::new(interpreter))
                    && interpreter
                        .strip_prefix("/usr/bin/perl")
                        .is_some_and(|suffix| {
                            suffix.is_empty()
                                || (suffix.as_bytes().first().is_some_and(u8::is_ascii_digit)
                                    && suffix
                                        .bytes()
                                        .all(|byte| byte.is_ascii_digit() || byte == b'.'))
                        }),
                "ExifTool requires a fixed protected system Perl"
            );
            if bytes.starts_with(b"#!/bin/sh\n") {
                ensure!(
                    bytes == exiftool_wrapper(interpreter).as_bytes(),
                    "unrecognized private ExifTool wrapper"
                );
            }
            self.system.insert(PathBuf::from(interpreter));
            ensure!(
                modules.join("Image/ExifTool.pm").is_file(),
                "ExifTool private modules are missing"
            );
            let base = PathBuf::from("scripts").join(name);
            self.data.insert(
                base.join("exiftool.pl"),
                (script.clone(), digest(&script_bytes)),
            );
            for entry in walkdir::WalkDir::new(&modules).follow_links(false) {
                let entry = entry?;
                ensure!(
                    !entry.file_type().is_symlink(),
                    "ExifTool module tree contains an unsupported symbolic link"
                );
                if entry.file_type().is_file() {
                    let source = entry.path().canonicalize()?;
                    self.data.insert(
                        base.join("lib").join(entry.path().strip_prefix(&modules)?),
                        (source.clone(), digest(&read_regular(&source)?)),
                    );
                } else {
                    ensure!(
                        entry.file_type().is_dir(),
                        "ExifTool module tree contains a special file"
                    );
                }
            }
            let config = script
                .parent()
                .context("ExifTool script has no parent")?
                .join(".ExifTool_config");
            if config.try_exists()? {
                bail!(
                    "ExifTool executable-local config needs an explicit runtime adapter; user HOME/EXIFTOOL_HOME config remains an external input"
                );
            }
            self.external.insert("ExifTool HOME/EXIFTOOL_HOME user config and OS Perl modules retain their normal semantics and are not frozen".into());
            let wrapper = exiftool_wrapper(interpreter);
            let target = base.join("exiftool");
            self.wrappers.insert(target.clone(), wrapper);
            self.tools.insert(name.into(), target);
            Ok(())
        }
    }

    fn exiftool_wrapper(interpreter: &str) -> String {
        format!(
            "#!/bin/sh\nbase=${{0%/*}}\n[ \"$base\" != \"$0\" ] || exit 1\nPERL5LIB=\"$base/lib\" exec '{interpreter}' \"$base/exiftool.pl\" \"$@\"\n"
        )
    }

    fn command(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
        let output = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("start {program} for private tool runtime"))?;
        ensure!(
            output.status.success(),
            "{program} failed for private tool runtime: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }

    fn copy_verified(source: &Path, destination: &Path, expected: &str) -> Result<()> {
        ensure!(
            digest(&read_regular(source)?) == expected,
            "tool runtime source changed before copying: {}",
            source.display()
        );
        fs::create_dir_all(
            destination
                .parent()
                .context("private runtime destination has no parent")?,
        )?;
        fs::copy(source, destination)?;
        ensure!(
            digest(&read_regular(destination)?) == expected
                && digest(&read_regular(source)?) == expected,
            "tool runtime source changed during copying: {}",
            source.display()
        );
        Ok(())
    }

    fn private_import(target: &Path) -> Result<String> {
        ensure!(!target.is_absolute(), "private dependency must be relative");
        let suffix = target.strip_prefix("files")?;
        Ok(format!(
            "@loader_path/../{}",
            suffix
                .to_str()
                .context("runtime dependency path is not UTF-8")?
        ))
    }

    fn probe_tools(staging: &Path, tools: &BTreeMap<String, PathBuf>) -> Result<()> {
        for (name, target) in tools {
            let program = if target.is_absolute() {
                target.clone()
            } else {
                staging.join(target)
            };
            crate::common_utils::probe_tool_runtime(&program, name)
                .with_context(|| format!("confirm private runtime {name}"))?;
        }
        Ok(())
    }

    fn relocate(staging: &Path, graph: &Graph) -> Result<()> {
        for node in graph.files.values() {
            let path = staging.join(&node.relative);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            for (import, target) in &node.imports {
                if target.is_absolute() {
                    continue;
                }
                let replacement = private_import(target)?;
                command(
                    "/usr/bin/install_name_tool",
                    &[
                        "-change".as_ref(),
                        import.as_ref(),
                        replacement.as_ref(),
                        path.as_os_str(),
                    ],
                )?;
            }
            if node.dylib {
                let id = private_import(&node.relative)?;
                command(
                    "/usr/bin/install_name_tool",
                    &["-id".as_ref(), id.as_ref(), path.as_os_str()],
                )?;
            }
            for rpath in &node.rpaths {
                command(
                    "/usr/bin/install_name_tool",
                    &["-delete_rpath".as_ref(), rpath.as_ref(), path.as_os_str()],
                )?;
            }
            command(
                "/usr/bin/codesign",
                &[
                    "--force".as_ref(),
                    "--sign".as_ref(),
                    "-".as_ref(),
                    path.as_os_str(),
                ],
            )?;
            command(
                "/usr/bin/codesign",
                &["--verify".as_ref(), "--strict".as_ref(), path.as_os_str()],
            )?;
            for slice in slices(&read_regular(&path)?)? {
                ensure!(
                    slice.rpaths.is_empty(),
                    "private runtime retained external rpaths"
                );
                for import in slice.imports {
                    if system_library(Path::new(&import)) {
                        continue;
                    }
                    let target = expand(
                        &import,
                        path.parent().context("runtime path has no parent")?,
                        path.parent().context("runtime path has no parent")?,
                    )?;
                    ensure!(
                        target.canonicalize()?.starts_with(staging),
                        "private runtime import escaped its closure"
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn prepare(root: &Path, tools: &BTreeMap<String, PathBuf>) -> Result<Snapshot> {
        ensure!(!tools.is_empty(), "cannot prepare an empty tool runtime");
        guard_loader_environment()?;
        let mut graph = Graph {
            system_identity: system_identity()?,
            ..Graph::default()
        };
        for (name, selected) in tools {
            crate::runtime_config::parse_tool_override(&format!("{name}={}", selected.display()))
                .map_err(anyhow::Error::msg)?;
            let requested = selected.clone();
            let selected = selected
                .canonicalize()
                .with_context(|| format!("selected tool {name} is unavailable"))?;
            let bytes = read_regular(&selected)?;
            graph.selections.insert(
                name.clone(),
                Selection {
                    requested,
                    canonical: selected.clone(),
                    blake3: digest(&bytes),
                },
            );
            if system_program(&selected) {
                graph.system.insert(selected.clone());
                graph.tools.insert(name.clone(), selected);
                continue;
            }
            ensure!(
                !matches!(name.as_str(), "magick" | "convert" | "osxphotos"),
                "tool {name} needs an adapter for runtime plugins/delegates/interpreter data; no incomplete snapshot is published"
            );
            if bytes.starts_with(b"#!") {
                graph.exiftool(name, &selected, &bytes)?;
            } else {
                let executable = selected.parent().context("selected tool has no parent")?;
                let target = graph.collect(&selected, executable, &[], &mut BTreeSet::new())?;
                graph.tools.insert(name.clone(), target);
            }
        }
        let storage = root.join("runtime/toolchains");
        fs::create_dir_all(&storage)?;
        ensure!(
            fs::symlink_metadata(&storage)?.is_dir(),
            "runtime storage must not be a symlink"
        );
        fs::set_permissions(&storage, fs::Permissions::from_mode(0o700))?;
        let storage = storage.canonicalize()?;
        let source_identity = digest(&serde_json::to_vec(&(1_u8, &graph))?);
        let index_directory = storage.join("sources");
        fs::create_dir_all(&index_directory)?;
        ensure!(
            fs::symlink_metadata(&index_directory)?.is_dir(),
            "runtime index must not be a symlink"
        );
        let index = index_directory.join(&source_identity);
        if index.try_exists()? {
            let bytes = read_regular(&index)?;
            let identity = std::str::from_utf8(&bytes)?;
            ensure!(
                identity.len() == 64 && identity.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid runtime source index"
            );
            let manifest = storage.join(identity).join("manifest.json");
            let recorded: Manifest = serde_json::from_slice(&read_regular(&manifest)?)?;
            ensure!(
                recorded.source_identity == source_identity,
                "runtime source index identity mismatch"
            );
            return load_and_verify(&manifest);
        }
        let staging = tempfile::Builder::new()
            .prefix(".preparing-")
            .tempdir_in(&storage)?;
        for (source, node) in &graph.files {
            copy_verified(
                source,
                &staging.path().join(&node.relative),
                &node.source_hash,
            )?;
        }
        for (target, (source, hash)) in &graph.data {
            copy_verified(source, &staging.path().join(target), hash)?;
        }
        for (target, wrapper) in &graph.wrappers {
            let target = staging.path().join(target);
            fs::write(&target, wrapper)?;
            fs::set_permissions(target, fs::Permissions::from_mode(0o755))?;
        }
        relocate(staging.path(), &graph)?;
        probe_tools(staging.path(), &graph.tools)?;
        for selection in graph.selections.values() {
            ensure!(
                digest(&read_regular(&selection.canonical)?) == selection.blake3,
                "selected tool changed before runtime publication"
            );
        }
        let mut manifest = Manifest {
            schema_version: 1,
            kind: "mfb_tool_snapshot".into(),
            source_identity,
            system_identity: graph.system_identity,
            selections: graph.selections,
            tools: graph.tools,
            members: BTreeMap::new(),
            system_dependencies: graph.system,
            external_inputs: graph.external,
        };
        for (source, node) in &graph.files {
            ensure!(
                digest(&read_regular(source)?) == node.source_hash,
                "runtime installation changed before publication"
            );
            let bytes = read_regular(&staging.path().join(&node.relative))?;
            manifest.members.insert(
                node.relative.clone(),
                Member {
                    source: Some(source.clone()),
                    source_blake3: Some(node.source_hash.clone()),
                    blake3: digest(&bytes),
                    bytes: u64::try_from(bytes.len())?,
                    executable: true,
                },
            );
        }
        for (target, (source, source_hash)) in &graph.data {
            ensure!(
                digest(&read_regular(source)?) == *source_hash,
                "ExifTool runtime changed before publication"
            );
            let bytes = read_regular(&staging.path().join(target))?;
            manifest.members.insert(
                target.clone(),
                Member {
                    source: Some(source.clone()),
                    source_blake3: Some(source_hash.clone()),
                    blake3: digest(&bytes),
                    bytes: u64::try_from(bytes.len())?,
                    executable: false,
                },
            );
        }
        for (target, wrapper) in &graph.wrappers {
            manifest.members.insert(
                target.clone(),
                Member {
                    source: None,
                    source_blake3: None,
                    blake3: digest(wrapper.as_bytes()),
                    bytes: u64::try_from(wrapper.len())?,
                    executable: true,
                },
            );
        }
        for member in manifest.members.keys() {
            fs::File::open(staging.path().join(member))?.sync_all()?;
        }
        for entry in walkdir::WalkDir::new(staging.path()).contents_first(true) {
            let entry = entry?;
            if entry.file_type().is_dir() {
                fs::File::open(entry.path())?.sync_all()?;
            }
        }
        let identity = manifest_identity(&manifest)?;
        let destination = storage.join(&identity);
        let manifest_path = destination.join("manifest.json");
        let data = serde_json::to_vec(&manifest)?;
        let target = staging.path().join("manifest.json");
        fs::write(&target, data)?;
        fs::File::open(&target)?.sync_all()?;
        fs::File::open(staging.path())?.sync_all()?;
        if !destination.try_exists()? {
            match fs::rename(staging.path(), &destination) {
                Ok(()) => {}
                Err(error) if destination.try_exists()? => {
                    load_and_verify(&manifest_path).context(error)?;
                }
                Err(error) => return Err(error.into()),
            }
            fs::File::open(&storage)?.sync_all()?;
        }
        let snapshot = load_and_verify(&manifest_path)?;
        let mut pending_index = tempfile::NamedTempFile::new_in(&index_directory)?;
        pending_index.write_all(identity.as_bytes())?;
        pending_index.as_file().sync_all()?;
        pending_index.persist(&index).map_err(|error| error.error)?;
        fs::File::open(&index_directory)?.sync_all()?;
        Ok(snapshot)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::process::Command;

    // Cargo injects a loader search path for its test binaries. Isolate that
    // harness environment without relaxing the production loader guard or
    // changing the environment shared by other concurrently running tests.
    fn isolated_test(name: &str) -> Result<bool> {
        const CHILD: &str = "MFB_TOOL_SNAPSHOT_TEST_CHILD";
        let receipt = format!("MFB tool snapshot isolated test: {name}");
        if std::env::var(CHILD).ok().as_deref() == Some(name) {
            println!("{receipt}");
            return Ok(false);
        }
        let test = format!("infra::tool_snapshot::tests::{name}");
        let output = Command::new(std::env::current_exe()?)
            .args(["--exact", &test, "--nocapture", "--include-ignored"])
            .env(CHILD, name)
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .output()?;
        ensure!(
            output.status.success() && String::from_utf8_lossy(&output.stdout).contains(&receipt),
            "isolated test {test} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8_lossy(&output.stdout));
        Ok(true)
    }

    fn run(program: &Path, args: &[&OsStr]) -> Result<Vec<u8>> {
        let output = Command::new(program).args(args).output()?;
        ensure!(
            output.status.success(),
            "{}: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output.stdout)
    }

    fn clang(source: &Path, output: &Path, libraries: &[&Path], flags: &[&str]) -> Result<()> {
        let mut arguments = vec![
            source.as_os_str(),
            "-o".as_ref(),
            output.as_os_str(),
            "-Wl,-headerpad_max_install_names".as_ref(),
        ];
        arguments.extend(libraries.iter().map(|path| path.as_os_str()));
        arguments.extend(flags.iter().map(OsStr::new));
        run(Path::new("/usr/bin/clang"), &arguments)?;
        Ok(())
    }

    #[test]
    fn native_runtime_survives_source_upgrade_removal_and_separates_same_named_libraries()
    -> Result<()> {
        if isolated_test(
            "native_runtime_survives_source_upgrade_removal_and_separates_same_named_libraries",
        )? {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        let a = source.join("a");
        let b = source.join("b");
        fs::create_dir_all(&a)?;
        fs::create_dir_all(&b)?;
        let a_c = a.join("a.c");
        let b_c = b.join("b.c");
        let helper_c = b.join("helper.c");
        fs::write(&a_c, "int a(void) { return 7; }\n")?;
        fs::write(&helper_c, "int helper(void) { return 9; }\n")?;
        fs::write(
            &b_c,
            "extern int helper(void); int b(void) { return helper(); }\n",
        )?;
        let a_lib = a.join("libsame.dylib");
        let b_lib = b.join("libsame.dylib");
        let helper = b.join("helper.dylib");
        clang(
            &a_c,
            &a_lib,
            &[],
            &["-dynamiclib", "-Wl,-install_name,@rpath/libsame.dylib"],
        )?;
        clang(
            &helper_c,
            &helper,
            &[],
            &["-dynamiclib", "-Wl,-install_name,@loader_path/helper.dylib"],
        )?;
        let b_id = format!("-Wl,-install_name,{}", b_lib.display());
        clang(&b_c, &b_lib, &[&helper], &["-dynamiclib", &b_id])?;
        let main_c = source.join("main.c");
        fs::write(
            &main_c,
            "#include <stdio.h>\nextern int a(void); extern int b(void); int main(void) { printf(\"%d\\n\", a()+b()); return 0; }\n",
        )?;
        let tool = source.join("probe");
        clang(
            &main_c,
            &tool,
            &[&a_lib, &b_lib],
            &["-Wl,-rpath,@executable_path/a"],
        )?;
        assert_eq!(run(&tool, &[])?, b"16\n");
        let tools = BTreeMap::from([("probe".into(), tool.clone())]);
        let root = temp.path().join("state");
        let unavailable = b.join("helper.saved");
        fs::rename(&helper, &unavailable)?;
        assert!(prepare(&root, &tools).is_err());
        assert!(!root.join("runtime/toolchains").exists());
        fs::rename(unavailable, &helper)?;
        let snapshot = prepare(&root, &tools)?;
        let first_mtime = fs::metadata(&snapshot.manifest_path)?.modified()?;
        let reused = prepare(&root, &tools)?;
        assert_eq!(snapshot.identity, reused.identity);
        assert_eq!(
            fs::metadata(&snapshot.manifest_path)?.modified()?,
            first_mtime
        );
        let manifest: Manifest = serde_json::from_slice(&fs::read(&snapshot.manifest_path)?)?;
        assert_eq!(
            manifest
                .members
                .keys()
                .filter(|path| path.file_name().is_some_and(|name| name == "libsame.dylib"))
                .count(),
            2
        );
        fs::write(&tool, b"updated installation")?;
        fs::remove_dir_all(source)?;
        let restored = load_and_verify(&snapshot.manifest_path)?;
        restored.verify_selection("probe", &tool)?;
        assert!(
            restored
                .verify_selection("probe", Path::new("/different/tool"))
                .is_err()
        );
        assert!(restored.verify_selection("absent", &tool).is_err());
        assert_eq!(run(&restored.tools["probe"], &[])?, b"16\n");
        fs::write(&restored.tools["probe"], b"corrupted private snapshot")?;
        assert!(restored.verify().is_err());
        Ok(())
    }

    #[test]
    fn unknown_script_and_invalid_binary_publish_no_runtime() -> Result<()> {
        if isolated_test("unknown_script_and_invalid_binary_publish_no_runtime")? {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let tool = temp.path().join("unknown");
        fs::write(&tool, b"#!/bin/sh\nexit 0\n")?;
        let root = temp.path().join("state");
        assert!(prepare(&root, &BTreeMap::from([("unknown".into(), tool)])).is_err());
        assert!(!root.join("runtime/toolchains").exists());
        assert!(macos::slices(b"malformed executable").is_err());
        Ok(())
    }

    #[test]
    fn universal_runtime_checks_every_architecture() -> Result<()> {
        if isolated_test("universal_runtime_checks_every_architecture")? {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("probe.c");
        fs::write(
            &source,
            "#include <stdio.h>\nint main(void) { puts(\"universal\"); return 0; }\n",
        )?;
        let arm = temp.path().join("arm");
        let x86 = temp.path().join("x86");
        clang(&source, &arm, &[], &["-arch", "arm64"])?;
        clang(&source, &x86, &[], &["-arch", "x86_64"])?;
        let fat = temp.path().join("fat");
        run(
            Path::new("/usr/bin/lipo"),
            &[
                "-create".as_ref(),
                arm.as_os_str(),
                x86.as_os_str(),
                "-output".as_ref(),
                fat.as_os_str(),
            ],
        )?;
        assert_eq!(macos::slices(&fs::read(&fat)?)?.len(), 2);
        let snapshot = prepare(temp.path(), &BTreeMap::from([("probe".into(), fat)]))?;
        assert_eq!(run(&snapshot.tools["probe"], &[])?, b"universal\n");
        snapshot.verify()?;
        Ok(())
    }

    #[test]
    #[ignore = "explicit local installed-tool acceptance; no production media"]
    fn installed_jpeg_tools_and_exiftool_are_independent_of_the_installation() -> Result<()> {
        if isolated_test("installed_jpeg_tools_and_exiftool_are_independent_of_the_installation")? {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let tools = ["cjxl", "djxl", "jxlinfo", "exiftool"]
            .into_iter()
            .map(|name| {
                Ok((
                    name.to_owned(),
                    crate::common_utils::resolve_tool_path(name)
                        .with_context(|| format!("required local tool {name}"))?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let baseline = prepare(&temp.path().join("baseline"), &tools)?;
        let installation = temp.path().join("temporary-installation");
        let baseline_root = baseline
            .manifest_path
            .parent()
            .context("baseline snapshot root")?;
        for entry in walkdir::WalkDir::new(baseline_root) {
            let entry = entry?;
            let target = installation.join(entry.path().strip_prefix(baseline_root)?);
            if entry.file_type().is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                fs::copy(entry.path(), &target)?;
            }
        }
        let copied_tools = baseline
            .tools
            .iter()
            .map(|(name, path)| {
                Ok((
                    name.clone(),
                    installation.join(path.strip_prefix(baseline_root)?),
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let root = temp.path().join("acceptance");
        let began = std::time::Instant::now();
        let snapshot = prepare(&root, &copied_tools)?;
        let cold = began.elapsed();
        let mtime = fs::metadata(&snapshot.manifest_path)?.modified()?;
        let began = std::time::Instant::now();
        let cached = prepare(&root, &copied_tools)?;
        let hot = began.elapsed();
        assert_eq!(cached.identity, snapshot.identity);
        assert_eq!(fs::metadata(&cached.manifest_path)?.modified()?, mtime);
        fs::remove_dir_all(&installation)?;
        snapshot.verify()?;
        let manifest: Manifest = serde_json::from_slice(&fs::read(&snapshot.manifest_path)?)?;
        println!(
            "private tool runtime acceptance: members={} cold_ms={} hot_ms={} identity_reused=true manifest_mtime_unchanged=true temporary_installation_removed=true",
            manifest.members.len(),
            cold.as_millis(),
            hot.as_millis()
        );
        for (name, original) in &tools {
            let flag = if name == "exiftool" {
                "-ver"
            } else if name == "jxlinfo" {
                "--help"
            } else {
                "--version"
            };
            let original_output = Command::new(original).arg(flag).output()?;
            let private_output = Command::new(&snapshot.tools[name]).arg(flag).output()?;
            assert_eq!(original_output.status.code(), private_output.status.code());
            assert_eq!(original_output.stdout, private_output.stdout);
        }
        let jpeg = temp.path().join("synthetic.jpg");
        image::RgbImage::from_pixel(3, 2, image::Rgb([11, 92, 203]))
            .save_with_format(&jpeg, image::ImageFormat::Jpeg)?;
        let jxl = temp.path().join("synthetic.jxl");
        let reconstructed = temp.path().join("reconstructed.jpg");
        run(
            &snapshot.tools["cjxl"],
            &[
                jpeg.as_os_str(),
                jxl.as_os_str(),
                "--lossless_jpeg=1".as_ref(),
                "--effort=1".as_ref(),
            ],
        )?;
        run(
            &snapshot.tools["djxl"],
            &[jxl.as_os_str(), reconstructed.as_os_str()],
        )?;
        assert_eq!(fs::read(&jpeg)?, fs::read(&reconstructed)?);
        snapshot.verify()?;
        Ok(())
    }
}

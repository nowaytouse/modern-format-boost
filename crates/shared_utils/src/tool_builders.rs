//! Type-safe builders for non-imaging media tools (VMAF, Exiv2, JXL-info, Dovi, HDR10+, x265).

use std::path::{Path, PathBuf};
use std::process::Command;

// Re-export other builders for architectural consistency and easier access.
pub use crate::ffmpeg_builder::{
    FfmpegBuilder, FfprobeBuilder, PixFmt, StreamType, VideoCodec, VideoProfile,
};
pub use crate::image_builders::{
    AvifencBuilder, DwebpBuilder, ExiftoolBuilder, GifskiBuilder, MagickBuilder, SipsBuilder,
    WebpmuxBuilder,
};
pub use crate::jxl_builder::{CjxlBuilder, DjxlBuilder};

/// Builder for constructing `vmaf` commands.
#[derive(Debug, Default)]
pub struct VmafBuilder {
    reference: Option<PathBuf>,
    distorted: Option<PathBuf>,
    output: Option<PathBuf>,
    features: Vec<String>,
    json: bool,
    threads: Option<usize>,
    model: Option<String>,
    extra_args: Vec<String>,
}

impl VmafBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reference<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.reference = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn distorted<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.distorted = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn feature<S: AsRef<str>>(&mut self, feature: S) -> &mut Self {
        self.features.push(feature.as_ref().to_string());
        self
    }

    pub const fn json(&mut self, enabled: bool) -> &mut Self {
        self.json = enabled;
        self
    }

    /// Sets the number of threads for VMAF calculation.
    pub const fn threads(&mut self, count: usize) -> &mut Self {
        self.threads = Some(count);
        self
    }

    /// Sets the VMAF model file path.
    pub fn model<S: AsRef<str>>(&mut self, path: S) -> &mut Self {
        self.model = Some(path.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("vmaf");

        if let Some(reference) = &self.reference {
            cmd.arg("--reference")
                .arg(crate::safe_path_arg(reference).as_ref());
        }

        if let Some(distorted) = &self.distorted {
            cmd.arg("--distorted")
                .arg(crate::safe_path_arg(distorted).as_ref());
        }

        for feature in &self.features {
            cmd.arg("--feature").arg(feature);
        }

        if self.json {
            cmd.arg("--json");
        }

        if let Some(threads) = self.threads {
            cmd.args(["--thread", &threads.to_string()]);
        }

        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }

        if let Some(output) = &self.output {
            cmd.arg("--output")
                .arg(crate::safe_path_arg(output).as_ref());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("vmaf")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `exiv2` commands.
#[derive(Debug, Default)]
pub struct Exiv2Builder {
    input: Option<PathBuf>,
    args: Vec<String>,
}

impl Exiv2Builder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.as_ref().to_string());
        self
    }

    /// Adds -pa (print all metadata).
    pub fn print_all(&mut self) -> &mut Self {
        self.args.push("-pa".to_string());
        self
    }

    /// Adds -ps (print metadata summary).
    pub fn print_summary(&mut self) -> &mut Self {
        self.args.push("-ps".to_string());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("exiv2");

        for arg in &self.args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("exiv2")
            .arg("-V")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `jxlinfo` commands.
#[derive(Debug, Default)]
pub struct JxlinfoBuilder {
    input: Option<PathBuf>,
}

impl JxlinfoBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("jxlinfo");

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("jxlinfo")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `dovi_tool` commands.
#[derive(Debug, Default)]
pub struct DoviBuilder {
    mode: Option<String>,
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    extra_args: Vec<String>,
}

impl DoviBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode<S: AsRef<str>>(&mut self, mode: S) -> &mut Self {
        self.mode = Some(mode.as_ref().to_string());
        self
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("dovi_tool");

        if let Some(mode) = &self.mode {
            cmd.arg(mode);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg("-i").arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg("-o").arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("dovi_tool")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `hdr10plus_tool` commands.
#[derive(Debug, Default)]
pub struct Hdr10PlusBuilder {
    mode: Option<String>,
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    skip_validation: bool,
}

impl Hdr10PlusBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mode<S: AsRef<str>>(&mut self, mode: S) -> &mut Self {
        self.mode = Some(mode.as_ref().to_string());
        self
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub const fn skip_validation(&mut self, enabled: bool) -> &mut Self {
        self.skip_validation = enabled;
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("hdr10plus_tool");

        if let Some(mode) = &self.mode {
            cmd.arg(mode);
        }

        if self.skip_validation {
            cmd.arg("--skip-validation");
        }

        if let Some(input) = &self.input {
            cmd.arg("-i").arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg("-o").arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("hdr10plus_tool")
            .arg("--help")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `x265` commands.
#[derive(Debug, Default)]
pub struct X265Builder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    y4m: bool,
    crf: Option<f32>,
    lossless: bool,
    preset: Option<String>,
    pools: Option<String>,
    log_level: Option<String>,
    hdr10_opt: bool,
    repeat_headers: bool,
    colorprim: Option<String>,
    transfer: Option<String>,
    colormatrix: Option<String>,
    master_display: Option<String>,
    max_cll: Option<String>,
    extra_args: Vec<String>,
}

fn x265_io_arg(path: &Path) -> std::borrow::Cow<'_, str> {
    // x265 uses a bare "-" to mean stdin/stdout. Do not armor it into "./-",
    // otherwise x265 tries to open a literal file named "-" and the pipe path breaks.
    if path == Path::new("-") {
        std::borrow::Cow::Borrowed("-")
    } else {
        crate::safe_path_arg(path)
    }
}

impl X265Builder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn output<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.output = Some(path.as_ref().to_path_buf());
        self
    }

    pub const fn y4m(&mut self, enabled: bool) -> &mut Self {
        self.y4m = enabled;
        self
    }

    pub const fn crf(&mut self, crf: f32) -> &mut Self {
        self.crf = Some(crf);
        self
    }

    pub const fn lossless(&mut self, enabled: bool) -> &mut Self {
        self.lossless = enabled;
        self
    }

    pub fn preset<S: AsRef<str>>(&mut self, preset: S) -> &mut Self {
        self.preset =
            Some(crate::types::preset::sanitize_hevc_preset_name(preset.as_ref()).to_string());
        self
    }

    pub fn pools<S: AsRef<str>>(&mut self, pools: S) -> &mut Self {
        self.pools = Some(pools.as_ref().to_string());
        self
    }

    pub fn log_level<S: AsRef<str>>(&mut self, level: S) -> &mut Self {
        self.log_level = Some(level.as_ref().to_string());
        self
    }

    pub const fn hdr10_opt(&mut self, enabled: bool) -> &mut Self {
        self.hdr10_opt = enabled;
        self
    }

    pub const fn repeat_headers(&mut self, enabled: bool) -> &mut Self {
        self.repeat_headers = enabled;
        self
    }

    pub fn colorprim<S: AsRef<str>>(&mut self, prim: S) -> &mut Self {
        self.colorprim = Some(prim.as_ref().to_string());
        self
    }

    pub fn transfer<S: AsRef<str>>(&mut self, trc: S) -> &mut Self {
        self.transfer = Some(trc.as_ref().to_string());
        self
    }

    pub fn colormatrix<S: AsRef<str>>(&mut self, matrix: S) -> &mut Self {
        self.colormatrix = Some(matrix.as_ref().to_string());
        self
    }

    pub fn master_display<S: AsRef<str>>(&mut self, master: S) -> &mut Self {
        self.master_display = Some(master.as_ref().to_string());
        self
    }

    pub fn max_cll<S: AsRef<str>>(&mut self, cll: S) -> &mut Self {
        self.max_cll = Some(cll.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(crate::constants::TOOL_X265);

        if self.y4m {
            cmd.arg("--y4m");
        }

        if let Some(crf) = self.crf {
            cmd.arg("--crf").arg(format!("{crf:.1}"));
        }

        if self.lossless {
            cmd.arg("--lossless");
        }

        if let Some(preset) = &self.preset {
            cmd.arg("--preset").arg(preset);
        }

        if let Some(pools) = &self.pools {
            cmd.arg("--pools").arg(pools);
        }

        if let Some(level) = &self.log_level {
            cmd.arg("--log-level").arg(level);
        }

        if self.hdr10_opt {
            cmd.arg("--hdr10-opt");
        }

        if self.repeat_headers {
            cmd.arg("--repeat-headers");
        }

        if let Some(cp) = &self.colorprim {
            cmd.arg("--colorprim").arg(cp);
        }

        if let Some(trc) = &self.transfer {
            cmd.arg("--transfer").arg(trc);
        }

        if let Some(matrix) = &self.colormatrix {
            cmd.arg("--colormatrix").arg(matrix);
        }

        if let Some(md) = &self.master_display {
            cmd.arg("--master-display").arg(md);
        }

        if let Some(cll) = &self.max_cll {
            cmd.arg("--max-cll").arg(cll);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg("--input").arg(x265_io_arg(input).as_ref());
        }

        if let Some(output) = &self.output {
            cmd.arg("--output").arg(x265_io_arg(output).as_ref());
        }

        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new(crate::constants::TOOL_X265)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::X265Builder;
    use std::path::Path;

    #[test]
    fn x265_builder_preserves_stdio_dash_input() {
        let args: Vec<String> = X265Builder::new()
            .input(Path::new("-"))
            .output(Path::new("out.hevc"))
            .build()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        let input_idx = args
            .iter()
            .position(|arg| arg == "--input")
            .expect("x265 input arg should exist");
        assert_eq!(args[input_idx + 1], "-");
    }

    #[test]
    fn x265_builder_still_arms_dash_prefixed_real_paths() {
        let args: Vec<String> = X265Builder::new()
            .input(Path::new("-clip.y4m"))
            .output(Path::new("out.hevc"))
            .build()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        let input_idx = args
            .iter()
            .position(|arg| arg == "--input")
            .expect("x265 input arg should exist");
        assert_eq!(args[input_idx + 1], "./-clip.y4m");
    }
}

/// Builder for constructing `osascript` (macOS) commands.
#[derive(Debug, Default)]
pub struct OsascriptBuilder {
    script: Option<String>,
    args: Vec<String>,
}

impl OsascriptBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub fn script<S: Into<String>>(&mut self, script: S) -> &mut Self {
        self.script = Some(script.into());
        self
    }

    pub fn arg<S: Into<String>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("osascript");
        if let Some(script) = &self.script {
            cmd.arg("-e").arg(script);
        }
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            Command::new("osascript")
                .arg("-e")
                .arg("return")
                .output()
                .is_ok_and(|o| o.status.success())
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

/// Builder for constructing `powershell` (Windows) commands.
#[derive(Debug, Default)]
pub struct PowershellBuilder {
    command: Option<String>,
    args: Vec<String>,
}

impl PowershellBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub fn command<S: Into<String>>(&mut self, command: S) -> &mut Self {
        self.command = Some(command.into());
        self
    }

    pub fn arg<S: Into<String>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("powershell");
        cmd.arg("-NoProfile").arg("-NonInteractive");
        if let Some(command) = &self.command {
            cmd.arg("-Command").arg(command);
        }
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }

    #[must_use]
    pub const fn check_available() -> bool {
        #[cfg(target_os = "windows")]
        {
            Command::new("powershell")
                .arg("-Command")
                .arg("Get-Date")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }
}

/// Builder for constructing `getfacl`/`setfacl` (Linux ACL) commands.
#[derive(Debug, Default)]
pub struct AclBuilder {
    tool: String, // "getfacl" or "setfacl"
    args: Vec<String>,
    input: Option<PathBuf>,
}

impl AclBuilder {
    #[must_use] 
    pub fn getfacl() -> Self {
        Self {
            tool: "getfacl".to_string(),
            ..Default::default()
        }
    }

    /// Configures setfacl to restore ACLs from stdin (-R --restore=-).
    #[must_use] 
    pub fn restore() -> Self {
        let mut builder = Self::setfacl();
        builder.arg("--restore=-");
        builder
    }

    #[must_use] 
    pub fn setfacl() -> Self {
        Self {
            tool: "setfacl".to_string(),
            ..Default::default()
        }
    }

    pub fn arg<S: Into<String>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(&self.tool);
        for arg in &self.args {
            cmd.arg(arg);
        }
        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }
        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("getfacl")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `sysctl` (macOS/Linux) commands.
#[derive(Debug, Default)]
pub struct SysctlBuilder {
    args: Vec<String>,
}

impl SysctlBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub fn arg<S: Into<String>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("sysctl");
        for arg in &self.args {
            cmd.arg(arg);
        }
        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("sysctl")
            .arg("-a")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `vm_stat` (macOS) commands.
#[derive(Debug, Default)]
pub struct VmstatBuilder {}

impl VmstatBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn build(&self) -> Command {
        Command::new("vm_stat")
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("vm_stat")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `attrib` (Windows) commands.
#[derive(Debug, Default)]
pub struct AttribBuilder {
    args: Vec<String>,
    input: Option<PathBuf>,
}

impl AttribBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub fn arg<S: Into<String>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("attrib");
        for arg in &self.args {
            cmd.arg(arg);
        }
        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }
        cmd
    }

    #[must_use]
    pub fn check_available() -> bool {
        Command::new("attrib")
            .arg("/?")
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

/// Builder for constructing `rsync` commands.
#[derive(Debug, Default)]
pub struct RsyncBuilder {
    executable: Option<String>,
    args: Vec<String>,
    sources: Vec<PathBuf>,
    destination: Option<PathBuf>,
}

impl RsyncBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub fn executable<S: Into<String>>(&mut self, path: S) -> &mut Self {
        self.executable = Some(path.into());
        self
    }

    pub fn arg<S: Into<String>>(&mut self, arg: S) -> &mut Self {
        self.args.push(arg.into());
        self
    }

    pub fn add_source<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.sources.push(path.as_ref().to_path_buf());
        self
    }

    pub fn destination<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.destination = Some(path.as_ref().to_path_buf());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let exe = self.executable.as_deref().unwrap_or("rsync");
        let mut cmd = Command::new(exe);

        // Protect args against shell/remote interpretation (requires rsync 3.0.0+)
        cmd.arg("--protect-args");

        for arg in &self.args {
            cmd.arg(arg);
        }
        for src in &self.sources {
            cmd.arg(crate::safe_path_arg(src).as_ref());
        }
        if let Some(dest) = &self.destination {
            cmd.arg(crate::safe_path_arg(dest).as_ref());
        }
        cmd
    }
}

/// Builder for constructing `ps` commands (Unix).
#[derive(Debug, Default)]
pub struct PsBuilder {
    pid: Option<u32>,
    output_fields: Vec<String>,
}

impl PsBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn pid(&mut self, pid: u32) -> &mut Self {
        self.pid = Some(pid);
        self
    }

    pub fn output_field<S: Into<String>>(&mut self, field: S) -> &mut Self {
        self.output_fields.push(field.into());
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("ps");
        if let Some(pid) = self.pid {
            cmd.args(["-p", &pid.to_string()]);
        }
        if !self.output_fields.is_empty() {
            cmd.arg("-o");
            cmd.arg(format!("{}=", self.output_fields.join(",")));
        }
        cmd
    }
}

/// Builder for constructing `kill` commands (Unix).
#[derive(Debug, Default)]
pub struct KillBuilder {
    signal: Option<String>,
    pid: Option<u32>,
}

impl KillBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub fn signal<S: Into<String>>(&mut self, sig: S) -> &mut Self {
        self.signal = Some(sig.into());
        self
    }

    pub const fn pid(&mut self, pid: u32) -> &mut Self {
        self.pid = Some(pid);
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("kill");
        if let Some(sig) = &self.signal {
            cmd.arg(sig);
        }
        if let Some(pid) = self.pid {
            cmd.arg(pid.to_string());
        }
        cmd
    }
}

/// Builder for constructing `hostname` commands.
#[derive(Debug, Default)]
pub struct HostnameBuilder {}

impl HostnameBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn build(&self) -> Command {
        Command::new("hostname")
    }
}

/// Builder for constructing `taskkill` commands (Windows).
#[derive(Debug, Default)]
pub struct TaskkillBuilder {
    pid: Option<u32>,
    force: bool,
}

impl TaskkillBuilder {
    #[must_use] 
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn pid(&mut self, pid: u32) -> &mut Self {
        self.pid = Some(pid);
        self
    }

    pub const fn force(&mut self) -> &mut Self {
        self.force = true;
        self
    }

    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new("taskkill");
        if let Some(pid) = self.pid {
            cmd.args(["/PID", &pid.to_string()]);
        }
        if self.force {
            cmd.arg("/F");
        }
        cmd
    }
}

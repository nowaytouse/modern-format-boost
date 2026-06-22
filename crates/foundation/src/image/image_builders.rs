//! Type-safe builders for imaging tools (`ImageMagick`, `webpmux`, `gifski`, `avifenc`, `sips`, `exiftool`).

use crate::builder_base::ToolBuilder;
use crate::constants;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Builder for constructing `magick` (`ImageMagick`) commands.
#[derive(Debug, Default)]
pub struct MagickBuilder {
    base: crate::builder_base::BaseBuilder,
    strip: bool,
    depth: Option<u8>,
    colorspace: Option<String>,
    defines: Vec<(String, String)>,
    sets: Vec<(String, String)>,
    use_stdout: bool,
    format: Option<String>,
}

impl MagickBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub const fn strip(&mut self, enabled: bool) -> &mut Self {
        self.strip = enabled;
        self
    }

    pub const fn depth(&mut self, depth: u8) -> &mut Self {
        self.depth = Some(depth);
        self
    }

    pub fn colorspace<S: AsRef<str>>(&mut self, colorspace: S) -> &mut Self {
        self.colorspace = Some(colorspace.as_ref().to_string());
        self
    }

    pub fn define<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.defines
            .push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    pub fn set<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<str>,
        V: AsRef<str>,
    {
        self.sets
            .push((key.as_ref().to_string(), value.as_ref().to_string()));
        self
    }

    pub const fn use_stdout(&mut self, enabled: bool) -> &mut Self {
        self.use_stdout = enabled;
        self
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }
}

crate::impl_base_builder_accessors_full!(MagickBuilder);

impl ToolBuilder for MagickBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_MAGICK
    }

    fn get_resolved_command(&self) -> Command {
        let path = crate::media_conversion_gate::delivery_imagemagick_cli_path_or_default();
        Command::new(path)
    }

    fn check_available(&self) -> bool {
        crate::common_utils::resolve_imagemagick_cli().is_some()
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();
        let uses_magick7 = crate::common_utils::imagemagick_uses_magick7();

        if let Some(input) = self.base.inputs.first() {
            let safe_input = crate::path_safety::magick_safe_path(input);
            if uses_magick7 {
                cmd.arg("--").arg(safe_input.as_ref());
            } else {
                cmd.arg(safe_input.as_ref());
            }
        }

        self.base.apply_to_command(&mut cmd);

        if self.strip {
            cmd.arg(constants::MAGICK_ARG_STRIP);
        }

        for (k, v) in &self.defines {
            cmd.arg(constants::MAGICK_ARG_DEFINE)
                .arg(format!("{k}={v}"));
        }

        for (k, v) in &self.sets {
            cmd.arg(constants::MAGICK_ARG_SET).arg(k).arg(v);
        }

        if let Some(cs) = &self.colorspace {
            cmd.arg(constants::MAGICK_ARG_COLORSPACE).arg(cs);
        }

        if let Some(d) = self.depth {
            cmd.arg(constants::MAGICK_ARG_DEPTH).arg(d.to_string());
        }

        if let Some(fmt) = &self.format {
            cmd.arg(constants::MAGICK_ARG_FORMAT).arg(fmt);
        }

        if self.use_stdout {
            if !self.base.extra_args.iter().any(|a| a.ends_with(":-")) {
                cmd.arg("png:-");
            }
            cmd.stdin(Stdio::null()); // Default for IM piped
            cmd.stdout(Stdio::piped());
        } else {
            self.base.apply_output(&mut cmd, None);
        }

        cmd
    }
}

/// Builder for constructing `identify` (`ImageMagick`) commands.
#[derive(Debug, Default)]
pub struct IdentifyBuilder {
    base: crate::builder_base::BaseBuilder,
    format: Option<String>,
    use_magick: bool, // toggle between 'magick identify' and 'identify'
    verbose: bool,
}

impl IdentifyBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }

    pub const fn use_magick(&mut self, enabled: bool) -> &mut Self {
        self.use_magick = enabled;
        self
    }

    /// Enables verbose output.
    pub const fn verbose(&mut self, enabled: bool) -> &mut Self {
        self.verbose = enabled;
        self
    }
}

crate::impl_base_builder_accessors!(IdentifyBuilder);

impl ToolBuilder for IdentifyBuilder {
    fn get_command_name(&self) -> &str {
        if self.use_magick {
            constants::TOOL_MAGICK
        } else {
            constants::TOOL_IDENTIFY
        }
    }

    fn get_check_args(&self) -> &[&str] {
        if self.use_magick {
            &[constants::MAGICK_ARG_IDENTIFY, constants::ARG_VERSION]
        } else {
            &[constants::ARG_VERSION]
        }
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();
        if self.use_magick {
            cmd.arg(constants::MAGICK_ARG_IDENTIFY);
        }

        if let Some(fmt) = &self.format {
            cmd.arg(constants::MAGICK_ARG_FORMAT).arg(fmt);
        }

        if self.verbose {
            cmd.arg(constants::FFMPEG_ARG_VERBOSE);
        }

        self.base.apply_to_command(&mut cmd);
        self.base.apply_first_input(&mut cmd, None);

        cmd
    }

    fn check_available(&self) -> bool {
        let mut cmd = self.get_resolved_command();
        if self.use_magick {
            cmd.arg(constants::MAGICK_ARG_IDENTIFY);
        }
        cmd.arg(constants::ARG_VERSION);

        match cmd.output() {
            Ok(o) if o.status.success() => true,
            Ok(_) | Err(_) => {
                if self.use_magick {
                    false
                } else {
                    let path =
                        crate::common_utils::resolve_tool_path_or_audit(constants::TOOL_MAGICK);
                    let mut fallback = Command::new(path);
                    fallback
                        .arg(constants::MAGICK_ARG_IDENTIFY)
                        .arg(constants::ARG_VERSION);
                    match fallback.output() {
                        Ok(output) => output.status.success(),
                        Err(err) => {
                            crate::media_conversion_gate::delivery_jxl_batch_audit(
                                "magick_availability_probe",
                                format!("failed to run ImageMagick availability probe: {err}"),
                            );
                            false
                        }
                    }
                }
            }
        }
    }
}

/// Builder for constructing `webpmux` commands.
#[derive(Debug, Default)]
pub struct WebpmuxBuilder {
    base: crate::builder_base::BaseBuilder,
    frames: Vec<(PathBuf, u32, i32, i32, bool)>, // (path, duration, x, y, blend)
    loop_count: Option<u32>,
    bgcolor: Option<String>,
    info: bool,
    get_frame: Option<u32>,
}

impl WebpmuxBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub const fn info(&mut self, enabled: bool) -> &mut Self {
        self.info = enabled;
        self
    }

    pub const fn get_frame(&mut self, index: u32) -> &mut Self {
        self.get_frame = Some(index);
        self
    }

    pub fn add_frame<P: AsRef<Path>>(
        &mut self,
        path: P,
        duration: u32,
        x: i32,
        y: i32,
        blend: bool,
    ) -> &mut Self {
        self.frames
            .push((path.as_ref().to_path_buf(), duration, x, y, blend));
        self
    }

    pub const fn loop_count(&mut self, count: u32) -> &mut Self {
        self.loop_count = Some(count);
        self
    }

    pub fn bgcolor<S: AsRef<str>>(&mut self, color: S) -> &mut Self {
        self.bgcolor = Some(color.as_ref().to_string());
        self
    }
}

crate::impl_base_builder_accessors_full!(WebpmuxBuilder);

impl ToolBuilder for WebpmuxBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_WEBPMUX
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if self.info {
            cmd.arg(constants::WEBPMUX_ARG_INFO);
        }

        if let Some(i) = self.get_frame {
            cmd.arg(constants::WEBPMUX_ARG_GET)
                .arg(constants::WEBPMUX_ARG_FRAME)
                .arg(i.to_string());
        }

        self.base.apply_to_command(&mut cmd);
        self.base.apply_first_input(&mut cmd, None);

        for (path, dur, x, y, blend) in &self.frames {
            let blend_str = if *blend { "+b" } else { "-b" };
            cmd.arg("-frame")
                .arg(crate::safe_path_arg(path).as_ref())
                .arg(format!("+{dur}+{x}+{y}+{blend_str}"));
        }

        if let Some(count) = self.loop_count {
            cmd.arg(constants::WEBPMUX_ARG_LOOP).arg(count.to_string());
        }

        if let Some(bg) = &self.bgcolor {
            cmd.arg(constants::WEBPMUX_ARG_BGCOLOR).arg(bg);
        }

        self.base
            .apply_output(&mut cmd, Some(constants::WEBPMUX_ARG_OUTPUT));

        cmd
    }
}

/// Builder for constructing `gifski` commands.
#[derive(Debug, Default)]
pub struct GifskiBuilder {
    base: crate::builder_base::BaseBuilder,
    fps: Option<f32>,
    quality: Option<u8>,
    motion_quality: Option<u8>,
    lossy_quality: Option<u8>,
    width: Option<u32>,
    height: Option<u32>,
    repeat: Option<u32>,
    fast: bool,
}

impl GifskiBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub const fn fps(&mut self, fps: f32) -> &mut Self {
        self.fps = Some(fps);
        self
    }

    pub const fn quality(&mut self, quality: u8) -> &mut Self {
        self.quality = Some(quality);
        self
    }

    pub const fn motion_quality(&mut self, quality: u8) -> &mut Self {
        self.motion_quality = Some(quality);
        self
    }

    pub const fn lossy_quality(&mut self, quality: u8) -> &mut Self {
        self.lossy_quality = Some(quality);
        self
    }

    pub const fn dimensions(&mut self, width: u32, height: u32) -> &mut Self {
        self.width = Some(width);
        self.height = Some(height);
        self
    }

    pub const fn repeat(&mut self, repeat: u32) -> &mut Self {
        self.repeat = Some(repeat);
        self
    }

    pub const fn fast(&mut self, enabled: bool) -> &mut Self {
        self.fast = enabled;
        self
    }
}

crate::impl_base_builder_accessors_full!(GifskiBuilder);

impl ToolBuilder for GifskiBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_GIFSKI
    }

    /// Construct the `std::process::Command`.
    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if let Some(fps) = self.fps {
            cmd.arg(constants::GIFSKI_ARG_FPS).arg(format!("{fps:.3}"));
        }

        if let Some(q) = self.quality {
            cmd.arg(constants::GIFSKI_ARG_QUALITY).arg(q.to_string());
        }

        if let Some(q) = self.motion_quality {
            cmd.arg(constants::GIFSKI_ARG_MOTION_QUALITY)
                .arg(q.to_string());
        }

        if let Some(q) = self.lossy_quality {
            cmd.arg(constants::GIFSKI_ARG_LOSSY_QUALITY)
                .arg(q.to_string());
        }

        if let (Some(w), Some(h)) = (self.width, self.height) {
            cmd.arg(constants::GIFSKI_ARG_WIDTH)
                .arg(w.to_string())
                .arg(constants::GIFSKI_ARG_HEIGHT)
                .arg(h.to_string());
        }

        if let Some(r) = self.repeat {
            cmd.arg(constants::GIFSKI_ARG_REPEAT).arg(r.to_string());
        }

        if self.fast {
            cmd.arg(constants::GIFSKI_ARG_FAST);
        }

        self.base.apply_to_command(&mut cmd);
        self.base
            .apply_output(&mut cmd, Some(constants::GIFSKI_ARG_OUTPUT));
        self.base.apply_all_inputs(&mut cmd, None);

        cmd
    }
}

/// Builder for constructing `avifenc` commands.
#[derive(Debug, Default)]
pub struct AvifencBuilder {
    base: crate::builder_base::BaseBuilder,
    min_quality: Option<u8>,
    max_quality: Option<u8>,
    lossless: bool,
    speed: Option<u8>,
    threads: Option<String>,
    depth: Option<u8>,
    yuv: Option<String>,
}

impl AvifencBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub const fn speed(&mut self, speed: u8) -> &mut Self {
        self.speed = Some(speed);
        self
    }

    pub const fn quality(&mut self, min: u8, max: u8) -> &mut Self {
        self.min_quality = Some(min);
        self.max_quality = Some(max);
        self
    }

    pub const fn lossless(&mut self, enabled: bool) -> &mut Self {
        self.lossless = enabled;
        self
    }

    pub fn jobs(&mut self, jobs: &str) -> &mut Self {
        self.threads = Some(jobs.to_string());
        self
    }

    pub const fn depth(&mut self, depth: u8) -> &mut Self {
        self.depth = Some(depth);
        self
    }

    pub fn yuv<S: AsRef<str>>(&mut self, yuv: S) -> &mut Self {
        self.yuv = Some(yuv.as_ref().to_string());
        self
    }
}

crate::impl_base_builder_accessors_full!(AvifencBuilder);

impl ToolBuilder for AvifencBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_AVIFENC
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if self.lossless {
            cmd.arg(constants::AVIFENC_ARG_LOSSLESS);
        }

        if let Some(s) = self.speed {
            cmd.arg(constants::AVIFENC_ARG_SPEED).arg(s.to_string());
        }

        if let Some(j) = &self.threads {
            cmd.arg(constants::AVIFENC_ARG_JOBS).arg(j);
        }

        if let Some(q) = self.max_quality {
            cmd.arg(constants::AVIFENC_ARG_MAX).arg(q.to_string());
        }

        if let Some(q) = self.min_quality {
            cmd.arg(constants::AVIFENC_ARG_MIN).arg(q.to_string());
        }

        if let Some(d) = self.depth {
            cmd.arg(constants::AVIFENC_ARG_DEPTH).arg(d.to_string());
        }

        if let Some(yuv) = &self.yuv {
            cmd.arg(constants::AVIFENC_ARG_YUV).arg(yuv);
        }

        self.base.apply_to_command(&mut cmd);
        self.base.apply_first_input(&mut cmd, None);
        self.base.apply_output(&mut cmd, None);

        cmd
    }
}

/// Builder for constructing `sips` commands (macOS only).
#[derive(Debug, Default)]
pub struct SipsBuilder {
    base: crate::builder_base::BaseBuilder,
    format: Option<String>,
    quality: Option<u32>,
}

impl SipsBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }
    /// Sets the output image quality (1-100).
    /// Values outside this range will be clamped to prevent sips errors.
    pub fn quality(&mut self, q: u32) -> &mut Self {
        self.quality = Some(q.clamp(1, 100));
        self
    }
}

crate::impl_base_builder_accessors_full!(SipsBuilder);

impl ToolBuilder for SipsBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_SIPS
    }

    fn get_check_args(&self) -> &[&str] {
        &[constants::ARG_V]
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        if let Some(fmt) = &self.format {
            cmd.arg(constants::SIPS_ARG_S)
                .arg(constants::SIPS_ARG_FORMAT)
                .arg(fmt);
        }

        if let Some(q) = self.quality {
            cmd.arg(constants::SIPS_ARG_S)
                .arg(constants::SIPS_ARG_FORMAT_OPTIONS)
                .arg(q.to_string());
        }

        self.base.apply_to_command(&mut cmd);
        self.base.apply_first_input(&mut cmd, None);
        self.base
            .apply_output(&mut cmd, Some(constants::SIPS_ARG_OUT));

        cmd
    }

    fn check_available(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            match self.get_resolved_command().arg(constants::ARG_V).output() {
                Ok(output) => output.status.success(),
                Err(err) => {
                    crate::media_conversion_gate::delivery_jxl_batch_audit(
                        "sips_availability_probe",
                        format!("failed to run sips availability probe: {err}"),
                    );
                    false
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }
}

/// Builder for constructing `exiftool` commands.
#[derive(Debug, Default)]
pub struct ExiftoolBuilder {
    base: crate::builder_base::BaseBuilder,
    overwrite_original: bool,
    use_stdin: bool,
}

impl ExiftoolBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }

    pub fn tags_from_file<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_TAGS_FROM_FILE);
        // ExifTool format-interprets % in tagsfromfile argument, so we must double them.
        self.arg(crate::path_safety::property_safe_path(path.as_ref()));
        self
    }

    pub const fn overwrite_original(&mut self) -> &mut Self {
        self.overwrite_original = true;
        self
    }

    pub fn extract_icc_profile(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_ICC_PROFILE)
            .arg(constants::EXIFTOOL_ARG_B);
        self
    }

    pub const fn use_stdin(&mut self) -> &mut Self {
        self.use_stdin = true;
        self
    }

    /// Strips all metadata tags from the file.
    /// Equivalent to `exiftool -all=`.
    pub fn strip_all(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_ALL);
        self
    }

    /// Ignores minor errors and warnings.
    /// Equivalent to `exiftool -m`.
    pub fn ignore_minor(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_M);
        self
    }

    /// Suppresses warnings and logs.
    /// Equivalent to `exiftool -q`. Call twice for absolute silence.
    pub fn quiet(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_Q);
        self
    }

    /// Preserves file modification date/time.
    /// Equivalent to `exiftool -P`.
    pub fn preserve_date(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_P);
        self
    }

    /// Allows processing of 'unsafe' tags.
    /// Equivalent to `exiftool -unsafe`.
    pub fn unsafe_tags(&mut self) -> &mut Self {
        self.arg(constants::EXIFTOOL_ARG_UNSAFE);
        self
    }
}

crate::impl_base_builder_accessors!(ExiftoolBuilder);

impl ToolBuilder for ExiftoolBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_EXIFTOOL
    }

    fn get_check_args(&self) -> &[&str] {
        &[constants::ARG_VER]
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();
        if self.overwrite_original {
            cmd.arg(constants::EXIFTOOL_ARG_OVERWRITE_ORIGINAL);
        }

        self.base.apply_to_command(&mut cmd);
        self.base.apply_first_input(&mut cmd, None);

        if self.use_stdin {
            cmd.stdin(std::process::Stdio::piped());
        }

        cmd
    }
}

/// Builder for constructing `dwebp` commands.
#[derive(Debug, Default)]
pub struct DwebpBuilder {
    base: crate::builder_base::BaseBuilder,
}

impl DwebpBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn check_available() -> bool {
        Self::default().check_available()
    }
}

crate::impl_base_builder_accessors_full!(DwebpBuilder);

impl ToolBuilder for DwebpBuilder {
    fn get_command_name(&self) -> &str {
        constants::TOOL_DWEBP
    }

    fn build(&self) -> Command {
        let mut cmd = self.get_resolved_command();

        self.base.apply_first_input(&mut cmd, None);
        self.base.apply_to_command(&mut cmd);
        self.base
            .apply_output(&mut cmd, Some(constants::WEBPMUX_ARG_OUTPUT));

        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_magick_builder() {
        let mut builder = MagickBuilder::new();
        builder
            .input(Path::new("in.png"))
            .output(Path::new("out.jpg"))
            .strip(true)
            .depth(8)
            .colorspace("sRGB")
            .define("jpeg:extent", "500kb")
            .set("comment", "test");

        let cmd = builder.build();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();

        if crate::common_utils::imagemagick_uses_magick7() {
            assert!(
                args.contains(&"--"),
                "IM7 magick uses -- before input paths"
            );
        } else {
            assert!(
                !args.contains(&"--"),
                "IM6 convert must not use magick-style -- separator"
            );
        }
        assert!(args.contains(&"-strip"));
        assert!(args.contains(&"-depth"));
        assert!(args.contains(&"8"));
        assert!(args.contains(&"colorspace"));
        assert!(args.contains(&"sRGB"));
        assert!(args.contains(&"-define"));
        assert!(args.contains(&"jpeg:extent=500kb"));
        assert!(args.contains(&"-set"));
        assert!(args.contains(&"comment"));
        assert!(args.contains(&"test"));
    }

    #[test]
    fn test_webpmux_builder() {
        let mut builder = WebpmuxBuilder::new();
        builder
            .input(Path::new("in.webp"))
            .output(Path::new("out.webp"))
            .add_frame(Path::new("f1.webp"), 100, 0, 0, true)
            .loop_count(0)
            .bgcolor("white")
            .info(true);

        let cmd = builder.build();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();

        assert!(args.contains(&"-info"));
        assert!(args.contains(&"-frame"));
        assert!(args.contains(&"f1.webp") || args.iter().any(|a| a.contains("f1.webp")));
        assert!(args.contains(&"-loop"));
        assert!(args.contains(&"0"));
        assert!(args.contains(&"-bgcolor"));
        assert!(args.contains(&"white"));
        assert!(args.contains(&"-o"));
    }

    #[test]
    fn test_gifski_builder() {
        let mut builder = GifskiBuilder::new();
        builder
            .input(Path::new("frame*.png"))
            .output(Path::new("out.gif"))
            .fps(24.0)
            .quality(90)
            .dimensions(1280, 720)
            .repeat(0)
            .fast(true);

        let cmd = builder.build();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();

        assert!(args.contains(&"--fps"));
        assert!(args.contains(&"24.000"));
        assert!(args.contains(&"--quality"));
        assert!(args.contains(&"90"));
        assert!(args.contains(&"--width"));
        assert!(args.contains(&"1280"));
        assert!(args.contains(&"--height"));
        assert!(args.contains(&"720"));
        assert!(args.contains(&"--repeat"));
        assert!(args.contains(&"0"));
        assert!(args.contains(&"--fast"));
        assert!(args.contains(&"--output"));
    }

    #[test]
    fn test_avifenc_builder() {
        let mut builder = AvifencBuilder::new();
        builder
            .input(Path::new("in.png"))
            .output(Path::new("out.avif"))
            .lossless(true)
            .speed(6)
            .jobs("all")
            .quality(60, 80)
            .depth(10);

        let cmd = builder.build();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();

        assert!(args.contains(&"--lossless"));
        assert!(args.contains(&"--speed"));
        assert!(args.contains(&"6"));
        assert!(args.contains(&"-j"));
        assert!(args.contains(&"all"));
        assert!(args.contains(&"--min"));
        assert!(args.contains(&"60"));
        assert!(args.contains(&"--max"));
        assert!(args.contains(&"80"));
        assert!(args.contains(&"--depth"));
        assert!(args.contains(&"10"));
    }

    #[test]
    fn test_exiftool_builder() {
        let mut builder = ExiftoolBuilder::new();
        builder
            .input(Path::new("img.jpg"))
            .tags_from_file(Path::new("src.jpg"))
            .overwrite_original()
            .ignore_minor()
            .quiet()
            .preserve_date();

        let cmd = builder.build();
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();

        assert!(args.contains(&"-tagsfromfile"));
        assert!(args.contains(&"-overwrite_original"));
        assert!(args.contains(&"-m"));
        assert!(args.contains(&"-q"));
        assert!(args.contains(&"-P"));
    }
}

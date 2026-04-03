//! Type-safe builder for constructing `ffmpeg` and `ffprobe` commands.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use crate::constants;
use crate::ffmpeg_process::FfmpegProcess;
pub use crate::types::EncoderPreset;

/// Common video codecs supported by the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
    Png,
    Apng,
    Copy,
    None,
}

#[derive(Debug, Default, Clone)]
struct FfmpegParams {
    x265: Option<String>,
    tag_video: Option<String>,
}

impl VideoCodec {
    #[must_use]
    pub fn ffmpeg_name(&self, is_gpu: bool) -> &'static str {
        match self {
            Self::H264 => if is_gpu { "h264_videotoolbox" } else { constants::FFMPEG_ENCODER_X264 },
            Self::Hevc => if is_gpu { "hevc_videotoolbox" } else { constants::FFMPEG_ENCODER_X265 },
            Self::Av1 => constants::FFMPEG_ENCODER_SVTAV1,
            Self::Vp9 => "libvpx-vp9",
            Self::Png => "png",
            Self::Apng => "apng",
            Self::Copy => "copy",
            Self::None => "none",
        }
    }
}

impl FromStr for VideoCodec {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "libx264" | "h264" => Ok(Self::H264),
            "libx265" | "x265" | "hevc" => Ok(Self::Hevc),
            "libsvtav1" | "av1" => Ok(Self::Av1),
            "libvpx-vp9" | "vp9" => Ok(Self::Vp9),
            "png" => Ok(Self::Png),
            "apng" => Ok(Self::Apng),
            "copy" => Ok(Self::Copy),
            "" => Ok(Self::None),
            _ => Err(()),
        }
    }
}

/// FFmpeg video profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProfile {
    Main,
    Main10,
    High,
    High10,
}

impl VideoProfile {
    #[must_use]
    pub const fn ffmpeg_name(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Main10 => "main10",
            Self::High => "high",
            Self::High10 => "high10",
        }
    }
}

impl FromStr for VideoProfile {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "main" => Ok(Self::Main),
            "main10" => Ok(Self::Main10),
            "high" => Ok(Self::High),
            "high10" => Ok(Self::High10),
            _ => Err(()),
        }
    }
}

/// Common pixel formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixFmt {
    Yuv420p,
    Yuv420p10le,
    Yuv444p,
    Yuv444p10le,
    Rgb24,
    Rgb48le,
    Gray,
}

impl PixFmt {
    #[must_use]
    pub const fn ffmpeg_name(&self) -> &'static str {
        match self {
            Self::Yuv420p => "yuv420p",
            Self::Yuv420p10le => "yuv420p10le",
            Self::Yuv444p => "yuv444p",
            Self::Yuv444p10le => "yuv444p10le",
            Self::Rgb24 => "rgb24",
            Self::Rgb48le => "rgb48le",
            Self::Gray => "gray",
        }
    }
}

impl FromStr for PixFmt {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "yuv420p" => Ok(Self::Yuv420p),
            "yuv420p10le" => Ok(Self::Yuv420p10le),
            "yuv444p" => Ok(Self::Yuv444p),
            "yuv444p10le" => Ok(Self::Yuv444p10le),
            "rgb24" => Ok(Self::Rgb24),
            "rgb48le" => Ok(Self::Rgb48le),
            "gray" => Ok(Self::Gray),
            _ => Err(()),
        }
    }
}

/// Builder for constructing `ffmpeg` commands.
#[derive(Debug, Default, Clone)]
pub struct FfmpegBuilder {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    vcodec: Option<VideoCodec>,
    format: Option<String>,
    frames_v: Option<u32>,
    crf: Option<f32>,
    preset: Option<EncoderPreset>,
    profile: Option<VideoProfile>,
    pix_fmt: Option<PixFmt>,
    threads: Option<usize>,
    overwrite: bool,
    map: Vec<String>,
    extra_args: Vec<String>,
    is_gpu: bool,
    params: FfmpegParams,
}

impl FfmpegBuilder {
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

    pub fn vcodec(&mut self, codec: VideoCodec) -> &mut Self {
        self.vcodec = Some(codec);
        self
    }

    pub fn format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.format = Some(format.as_ref().to_string());
        self
    }

    pub fn frames_v(&mut self, count: u32) -> &mut Self {
        self.frames_v = Some(count);
        self
    }

    pub fn crf(&mut self, crf: f32) -> &mut Self {
        self.crf = Some(crf);
        self
    }

    pub fn preset(&mut self, preset: EncoderPreset) -> &mut Self {
        self.preset = Some(preset);
        self
    }

    pub fn profile(&mut self, profile: VideoProfile) -> &mut Self {
        self.profile = Some(profile);
        self
    }

    pub fn pix_fmt(&mut self, pix_fmt: PixFmt) -> &mut Self {
        self.pix_fmt = Some(pix_fmt);
        self
    }

    pub fn threads(&mut self, threads: usize) -> &mut Self {
        self.threads = Some(threads);
        self
    }

    pub fn overwrite(&mut self, overwrite: bool) -> &mut Self {
        self.overwrite = overwrite;
        self
    }

    pub fn map<S: AsRef<str>>(&mut self, map: S) -> &mut Self {
        self.map.push(map.as_ref().to_string());
        self
    }

    pub fn use_gpu(&mut self, use_gpu: bool) -> &mut Self {
        self.is_gpu = use_gpu;
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    pub fn x265_params<S: AsRef<str>>(&mut self, params: S) -> &mut Self {
        self.params.x265 = Some(params.as_ref().to_string());
        self
    }

    pub fn tag_video<S: AsRef<str>>(&mut self, tag: S) -> &mut Self {
        self.params.tag_video = Some(tag.as_ref().to_string());
        self
    }

    pub fn args<I, S>(&mut self, args: I) -> &mut Self 
    where 
        I: IntoIterator<Item = S>,
        S: AsRef<str>
    {
        for arg in args {
            self.extra_args.push(arg.as_ref().to_string());
        }
        self
    }

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_FFMPEG);

        if self.overwrite {
            cmd.arg(constants::FFMPEG_ARG_OVERWRITE);
        }

        if let Some(threads) = self.threads {
            cmd.arg(constants::FFMPEG_ARG_THREADS).arg(threads.to_string());
        }

        for map in &self.map {
            cmd.arg(constants::FFMPEG_ARG_MAP).arg(map);
        }

        if let Some(input) = &self.input {
            cmd.arg(constants::FFMPEG_ARG_INPUT).arg(crate::safe_path_arg(input).as_ref());
        }

        if let Some(format) = &self.format {
            cmd.arg(constants::FFMPEG_ARG_FORMAT).arg(format);
        }

        if let Some(vcodec) = self.vcodec {
            cmd.arg(constants::FFMPEG_ARG_CODEC_VIDEO).arg(vcodec.ffmpeg_name(self.is_gpu));
        }

        if let Some(frames) = self.frames_v {
            cmd.arg(constants::FFMPEG_ARG_FRAMES_VIDEO).arg(frames.to_string());
        }

        if let Some(pix_fmt) = self.pix_fmt {
            cmd.arg(constants::FFMPEG_ARG_PIX_FMT).arg(pix_fmt.ffmpeg_name());
        }

        if let Some(preset) = self.preset {
            cmd.arg(constants::FFMPEG_ARG_PRESET).arg(preset.ffmpeg_name());
        }

        if let Some(profile) = self.profile {
            cmd.arg(constants::FFMPEG_ARG_PROFILE_VIDEO).arg(profile.ffmpeg_name());
        }

        if let Some(tag) = &self.params.tag_video {
            cmd.arg(constants::FFMPEG_ARG_TAG_VIDEO).arg(tag);
        }

        if let Some(x265) = &self.params.x265 {
            cmd.arg(constants::FFMPEG_ARG_X265_PARAMS).arg(x265);
        }

        if let Some(crf) = self.crf {
            cmd.arg(constants::FFMPEG_ARG_CRF).arg(crf.to_string());
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(output) = &self.output {
            cmd.arg(crate::safe_path_arg(output).as_ref());
        }

        cmd
    }

    /// Spawn the command and return an `FfmpegProcess` for monitoring.
    pub fn spawn(&self) -> anyhow::Result<FfmpegProcess> {
        let mut cmd = self.build();
        FfmpegProcess::spawn(&mut cmd)
    }
}

/// Builder for constructing `ffprobe` commands.
#[derive(Debug, Default)]
pub struct FfprobeBuilder {
    input: Option<PathBuf>,
    show_streams: bool,
    show_format: bool,
    show_entries: Option<String>,
    select_streams: Option<String>,
    print_format: Option<String>,
    extra_args: Vec<String>,
}

impl FfprobeBuilder {
    #[must_use]
    pub fn new() -> self::FfprobeBuilder {
        Self::default()
    }

    pub fn input<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.input = Some(path.as_ref().to_path_buf());
        self
    }

    pub fn show_streams(&mut self) -> &mut Self {
        self.show_streams = true;
        self
    }

    pub fn show_format(&mut self) -> &mut Self {
        self.show_format = true;
        self
    }

    pub fn show_entries<S: AsRef<str>>(&mut self, entries: S) -> &mut Self {
        self.show_entries = Some(entries.as_ref().to_string());
        self
    }

    pub fn select_streams<S: AsRef<str>>(&mut self, select: S) -> &mut Self {
        self.select_streams = Some(select.as_ref().to_string());
        self
    }

    pub fn print_format<S: AsRef<str>>(&mut self, format: S) -> &mut Self {
        self.print_format = Some(format.as_ref().to_string());
        self
    }

    pub fn arg<S: AsRef<str>>(&mut self, arg: S) -> &mut Self {
        self.extra_args.push(arg.as_ref().to_string());
        self
    }

    /// Construct the `std::process::Command`.
    #[must_use]
    pub fn build(&self) -> Command {
        let mut cmd = Command::new(constants::TOOL_FFPROBE);

        if self.show_streams {
            cmd.arg("-show_streams");
        }

        if self.show_format {
            cmd.arg("-show_format");
        }

        if let Some(entries) = &self.show_entries {
            cmd.arg("-show_entries").arg(entries);
        }

        if let Some(select) = &self.select_streams {
            cmd.arg("-select_streams").arg(select);
        }

        if let Some(format) = &self.print_format {
            cmd.arg("-print_format").arg(format);
        }

        for arg in &self.extra_args {
            cmd.arg(arg);
        }

        if let Some(input) = &self.input {
            cmd.arg(crate::safe_path_arg(input).as_ref());
        }

        cmd
    }
}

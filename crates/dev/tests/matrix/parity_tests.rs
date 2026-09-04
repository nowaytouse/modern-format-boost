use foundation::ToolBuilder;
use foundation::ffmpeg_builder::{EncoderPreset, FfmpegBuilder, VideoCodec};
use foundation::image_builders::{
    AvifencBuilder, DwebpBuilder, ExiftoolBuilder, GifskiBuilder, IdentifyBuilder, MagickBuilder,
    SipsBuilder, WebpmuxBuilder,
};
use foundation::jxl_builder::{CjxlBuilder, DjxlBuilder};
use foundation::tool_builders::{
    AclBuilder, AttribBuilder, DoviBuilder, Exiv2Builder, Hdr10PlusBuilder, HostnameBuilder,
    JxlinfoBuilder, KillBuilder, OsascriptBuilder, PowershellBuilder, PsBuilder, RsyncBuilder,
    SysctlBuilder, TaskkillBuilder, VmafBuilder, VmstatBuilder, X265Builder,
};

use std::path::Path;

#[test]
fn builder_argument_parity_suite() {
    test_ffmpeg_flag_order_parity();
    test_ffprobe_flag_order_parity();
    test_cjxl_flag_order_parity();
    test_djxl_flag_order_parity();
    test_jxlinfo_flag_order_parity();
    test_magick_flag_order_parity();
    test_identify_flag_order_parity();
    test_sips_flag_order_parity();
    test_webpmux_flag_order_parity();
    test_gifski_flag_order_parity();
    test_avifenc_flag_order_parity();
    test_dwebp_flag_order_parity();
    test_exiftool_flag_order_parity();
    test_exiv2_flag_order_parity();
    test_vmaf_flag_order_parity();
    test_x265_flag_order_parity();
    test_ffmpeg_hevc_preset_is_sanitized();
    test_x265_preset_is_sanitized();
    test_dovi_flag_order_parity();
    test_hdr10plus_flag_order_parity();
    test_osascript_flag_order_parity();
    test_powershell_flag_order_parity();
    test_sysctl_flag_order_parity();
    test_vm_stat_flag_order_parity();
    test_acl_getfacl_flag_order_parity();
    test_acl_setfacl_flag_order_parity();
    test_attrib_flag_order_parity();
    test_rsync_flag_order_parity();
    test_ps_flag_order_parity();
    test_kill_flag_order_parity();
    test_hostname_flag_order_parity();
    test_taskkill_flag_order_parity();
    test_ffprobe_pattern_safety_hardening();
    test_ffmpeg_odd_dim_correction_hardening();
    test_magick_path_armor_hardening();
    test_exiftool_nuclear_strip_hardening();
    test_ffmpeg_global_flag_priority_parity();
    test_sips_quality_clamping_hardening();
    test_vmaf_comprehensive_hardening();
    test_identify_verbose_hardening();
    test_gifski_performance_controls();
}

fn get_arg(args: &[String], idx: usize) -> &str {
    args.get(idx)
        .map_or_else(|| panic!("missing arg at index {idx}"), String::as_str)
}

fn test_ffmpeg_flag_order_parity() {
    let cmd = FfmpegBuilder::new()
        .overwrite()
        .threads(4)
        .input(Path::new("in.mp4"))
        .vcodec(VideoCodec::Hevc)
        .crf(18.0)
        .preset(EncoderPreset::Slower)
        .output(Path::new("out.mp4"))
        .build();

    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted order from commit 73edfa6: -y -threads [N] -i [in] [opts] [out]
    assert_eq!(get_arg(&args, 0), "-y");
    assert_eq!(get_arg(&args, 1), "-threads");
    assert_eq!(get_arg(&args, 2), "4");
    assert_eq!(get_arg(&args, 3), "-i");
    assert!(get_arg(&args, 4).contains("in.mp4"));
    assert_eq!(get_arg(&args, 5), "-c:v");
    assert!(get_arg(&args, 6).contains("libx265") || get_arg(&args, 6).contains("hevc"));
    assert_eq!(get_arg(&args, 7), "-crf");
    assert_eq!(get_arg(&args, 9), "-preset");
}

fn test_ffprobe_flag_order_parity() {
    let cmd = foundation::ffmpeg_builder::FfprobeBuilder::new()
        .arg("-v")
        .arg("error")
        .arg("-show_entries")
        .arg("format=duration")
        .input(Path::new("in.mp4"))
        .build();

    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: -v error -show_entries ... -- [path]
    assert_eq!(get_arg(&args, 0), "-v");
    assert_eq!(get_arg(&args, 1), "error");
    assert_eq!(get_arg(&args, 2), "-show_entries");
    assert_eq!(get_arg(&args, 3), "format=duration");
    assert_eq!(get_arg(&args, 4), "--");
    assert!(get_arg(&args, 5).contains("in.mp4"));
}

fn test_cjxl_flag_order_parity() {
    let cmd = CjxlBuilder::new()
        .input(Path::new("in.png"))
        .output(Path::new("out.jxl"))
        .distance(0.5)
        .effort(7)
        .build();

    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Archival JXL output requires an explicit container before codec flags.
    assert!(get_arg(&args, 0).contains("in.png"));
    assert!(get_arg(&args, 1).contains("out.jxl"));
    assert_eq!(get_arg(&args, 2), "--container=1");
    assert_eq!(get_arg(&args, 3), "--progressive_dc=0");
    assert_eq!(get_arg(&args, 4), "--responsive=0");
    assert_eq!(get_arg(&args, 5), "--noise=0");
    assert_eq!(get_arg(&args, 6), "--photon_noise_iso=0");
    assert_eq!(get_arg(&args, 7), "-d");
    assert_eq!(get_arg(&args, 8), "0.5");
    assert_eq!(get_arg(&args, 9), "-e");
    assert_eq!(get_arg(&args, 10), "7");
}

fn test_djxl_flag_order_parity() {
    let cmd = DjxlBuilder::new()
        .input(Path::new("in.jxl"))
        .output(Path::new("out.png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [in] [out]
    assert!(get_arg(&args, 0).contains("in.jxl"));
    assert!(get_arg(&args, 1).contains("out.png"));
}

fn test_jxlinfo_flag_order_parity() {
    let cmd = JxlinfoBuilder::new().input(Path::new("in.jxl")).build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [in]
    assert!(get_arg(&args, 0).contains("in.jxl"));
}

fn test_magick_flag_order_parity() {
    let cmd = MagickBuilder::new()
        .input(Path::new("in.jpg"))
        .arg("-quality")
        .arg("85")
        .output(Path::new("out.png"))
        .build();

    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // ImageMagick 7 uses `magick -- [in] ...`; ImageMagick 6 uses `[in] ...`.
    let has_sentinel = args.first().is_some_and(|arg| arg == "--");
    let input_idx = usize::from(has_sentinel);
    let quality_idx = input_idx + 1;
    assert!(get_arg(&args, input_idx).contains("in.jpg"));
    assert_eq!(get_arg(&args, quality_idx), "-quality");
    assert_eq!(get_arg(&args, quality_idx + 1), "85");
    assert!(get_arg(&args, quality_idx + 2).contains("out.png"));
}

fn test_identify_flag_order_parity() {
    let cmd = IdentifyBuilder::new()
        .arg("-verbose")
        .input(Path::new("in.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in]
    assert_eq!(get_arg(&args, 0), "-verbose");
    assert!(get_arg(&args, 1).contains("in.jpg"));
}

fn test_sips_flag_order_parity() {
    let cmd = SipsBuilder::new()
        .arg("-s")
        .arg("format")
        .arg("jpeg")
        .input(Path::new("in.png"))
        .output(Path::new("out.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in] --out [out]
    assert_eq!(get_arg(&args, 0), "-s");
    assert_eq!(get_arg(&args, 2), "jpeg");
    assert!(get_arg(&args, 3).contains("in.png"));
    assert_eq!(get_arg(&args, 4), "--out");
    assert!(get_arg(&args, 5).contains("out.jpg"));
}

fn test_webpmux_flag_order_parity() {
    let cmd = WebpmuxBuilder::new()
        .arg("-get")
        .arg("icc")
        .input(Path::new("in.webp"))
        .output(Path::new("out.icc"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in] -o [out]
    assert_eq!(get_arg(&args, 0), "-get");
    assert_eq!(get_arg(&args, 1), "icc");
    assert!(get_arg(&args, 2).contains("in.webp"));
    assert!(
        get_arg(&args, 3) == "-o"
            || get_arg(&args, 3) == "--output"
            || get_arg(&args, 3) == "--out"
    );
    assert!(get_arg(&args, 4).contains("out.icc"));
}

fn test_gifski_flag_order_parity() {
    let cmd = GifskiBuilder::new()
        .arg("--quality")
        .arg("90")
        .output(Path::new("out.gif"))
        .input(Path::new("frame.png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] -o [out] [in]
    assert_eq!(get_arg(&args, 0), "--quality");
    assert!(
        get_arg(&args, 2) == "-o"
            || get_arg(&args, 2) == "--output"
            || get_arg(&args, 2) == "--out"
    );
    assert!(get_arg(&args, 3).contains("out.gif"));
    assert!(get_arg(&args, 4).contains("frame.png"));
}

fn test_avifenc_flag_order_parity() {
    let cmd = AvifencBuilder::new()
        .arg("--speed")
        .arg("6")
        .input(Path::new("in.png"))
        .output(Path::new("out.avif"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in] [out]
    assert_eq!(get_arg(&args, 0), "--speed");
    assert!(get_arg(&args, 2).contains("in.png"));
    assert!(get_arg(&args, 3).contains("out.avif"));
}

fn test_dwebp_flag_order_parity() {
    let cmd = DwebpBuilder::new()
        .input(Path::new("in.webp"))
        .arg("-lossless")
        .output(Path::new("out.png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [in] [flags] -o [out]
    assert!(get_arg(&args, 0).contains("in.webp"));
    assert_eq!(get_arg(&args, 1), "-lossless");
    assert!(
        get_arg(&args, 2) == "-o"
            || get_arg(&args, 2) == "--output"
            || get_arg(&args, 2) == "--out"
    );
    assert!(get_arg(&args, 3).contains("out.png"));
}

fn test_exiftool_flag_order_parity() {
    let cmd = ExiftoolBuilder::new()
        .arg("-icc_profile")
        .arg("-b")
        .input(Path::new("in.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in]
    assert_eq!(get_arg(&args, 0), "-icc_profile");
    assert_eq!(get_arg(&args, 1), "-b");
    assert!(get_arg(&args, 2).contains("in.jpg"));
}

fn test_exiv2_flag_order_parity() {
    let cmd = Exiv2Builder::new()
        .arg("-pt")
        .input(Path::new("in.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in]
    assert_eq!(get_arg(&args, 0), "-pt");
    assert!(get_arg(&args, 1).contains("in.jpg"));
}

fn test_vmaf_flag_order_parity() {
    let cmd = VmafBuilder::new()
        .reference(Path::new("ref.mp4"))
        .distorted(Path::new("dist.mp4"))
        .output(Path::new("out.json"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: --reference [ref] --distorted [dist] --output [out]
    assert_eq!(get_arg(&args, 0), "--reference");
    assert!(get_arg(&args, 1).contains("ref.mp4"));
    assert_eq!(get_arg(&args, 2), "--distorted");
    assert!(get_arg(&args, 3).contains("dist.mp4"));
    assert_eq!(get_arg(&args, 4), "--output");
    assert!(get_arg(&args, 5).contains("out.json"));
}

fn test_x265_flag_order_parity() {
    let cmd = X265Builder::new()
        .crf(18.0)
        .preset("slower")
        .input(Path::new("in.y4m"))
        .output(Path::new("out.hevc"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] --input [in] --output [out]
    assert_eq!(get_arg(&args, 0), "--crf");
    assert_eq!(get_arg(&args, 2), "--preset");
    assert_eq!(get_arg(&args, 4), "--input");
    assert!(get_arg(&args, 5).contains("in.y4m"));
    assert_eq!(get_arg(&args, 6), "--output");
    assert!(get_arg(&args, 7).contains("out.hevc"));
}

fn test_ffmpeg_hevc_preset_is_sanitized() {
    let cmd = FfmpegBuilder::new()
        .input(Path::new("in.mp4"))
        .vcodec(VideoCodec::Hevc)
        .preset(EncoderPreset::Fast)
        .output(Path::new("out.mp4"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    let preset_idx = args
        .iter()
        .position(|arg| arg == "-preset")
        .unwrap_or_else(|| panic!("preset arg should exist"));
    assert_eq!(get_arg(&args, preset_idx + 1), "medium");
}

fn test_x265_preset_is_sanitized() {
    let cmd = X265Builder::new()
        .crf(18.0)
        .preset("veryslow")
        .input(Path::new("in.y4m"))
        .output(Path::new("out.hevc"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    let preset_idx = args
        .iter()
        .position(|arg| arg == "--preset")
        .unwrap_or_else(|| panic!("preset arg should exist"));
    assert_eq!(get_arg(&args, preset_idx + 1), "slower");
}

fn test_dovi_flag_order_parity() {
    let cmd = DoviBuilder::new()
        .mode("demux")
        .input(Path::new("in.hevc"))
        .output(Path::new("out.rpu"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [mode] -i [in] -o [out]
    assert_eq!(get_arg(&args, 0), "demux");
    assert_eq!(get_arg(&args, 1), "-i");
    assert!(get_arg(&args, 2).contains("in.hevc"));
    assert!(
        get_arg(&args, 3) == "-o"
            || get_arg(&args, 3) == "--output"
            || get_arg(&args, 3) == "--out"
    );
    assert!(get_arg(&args, 4).contains("out.rpu"));
}

fn test_hdr10plus_flag_order_parity() {
    let cmd = Hdr10PlusBuilder::new()
        .mode("extract")
        .input(Path::new("in.hevc"))
        .output(Path::new("out.json"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [mode] -i [in] -o [out]
    assert_eq!(get_arg(&args, 0), "extract");
    assert_eq!(get_arg(&args, 1), "-i");
    assert!(get_arg(&args, 2).contains("in.hevc"));
    assert!(
        get_arg(&args, 3) == "-o"
            || get_arg(&args, 3) == "--output"
            || get_arg(&args, 3) == "--out"
    );
    assert!(get_arg(&args, 4).contains("out.json"));
}

fn test_osascript_flag_order_parity() {
    let cmd = OsascriptBuilder::new().script("return 1").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: -e [script]
    assert_eq!(get_arg(&args, 0), "-e");
    assert_eq!(get_arg(&args, 1), "return 1");
}

fn test_powershell_flag_order_parity() {
    let cmd = PowershellBuilder::new().command("Get-Date").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: -NoProfile -NonInteractive -Command [command]
    assert_eq!(get_arg(&args, 0), "-NoProfile");
    assert_eq!(get_arg(&args, 1), "-NonInteractive");
    assert_eq!(get_arg(&args, 2), "-Command");
    assert_eq!(get_arg(&args, 3), "Get-Date");
}

fn test_sysctl_flag_order_parity() {
    let cmd = SysctlBuilder::new().arg("-n").arg("hw.ncpu").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [key]
    assert_eq!(get_arg(&args, 0), "-n");
    assert_eq!(get_arg(&args, 1), "hw.ncpu");
}

fn test_vm_stat_flag_order_parity() {
    let cmd = VmstatBuilder::new().build();
    let args_count = cmd.get_args().count();
    // Snapshotted: (no args)
    assert_eq!(args_count, 0);
}

fn test_acl_getfacl_flag_order_parity() {
    let cmd = AclBuilder::getfacl().input(Path::new("test.txt")).build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [in]
    assert!(get_arg(&args, 0).contains("test.txt"));
}

fn test_acl_setfacl_flag_order_parity() {
    let cmd = AclBuilder::setfacl()
        .arg("-m")
        .arg("u:user:rwx")
        .input(Path::new("test.txt"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in]
    assert_eq!(get_arg(&args, 0), "-m");
    assert_eq!(get_arg(&args, 1), "u:user:rwx");
    assert!(get_arg(&args, 2).contains("test.txt"));
}

fn test_attrib_flag_order_parity() {
    let cmd = AttribBuilder::new()
        .arg("+R")
        .input(Path::new("test.txt"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [in]
    assert_eq!(get_arg(&args, 0), "+R");
    assert!(get_arg(&args, 1).contains("test.txt"));
}

fn test_rsync_flag_order_parity() {
    let cmd = RsyncBuilder::new()
        .arg("-av")
        .input(Path::new("src"))
        .output(Path::new("dest"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [src] [dest]
    assert_eq!(get_arg(&args, 0), "--protect-args");
    assert_eq!(get_arg(&args, 1), "-av");
    assert!(get_arg(&args, 2).contains("src"));
    assert!(get_arg(&args, 3).contains("dest"));
}

fn test_ps_flag_order_parity() {
    let cmd = PsBuilder::new().pid(1234).output_field("pid").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: -p [pid] -o [fields]=
    assert_eq!(get_arg(&args, 0), "-p");
    assert_eq!(get_arg(&args, 1), "1234");
    assert!(
        get_arg(&args, 2) == "-o"
            || get_arg(&args, 2) == "--output"
            || get_arg(&args, 2) == "--out"
    );
    assert_eq!(get_arg(&args, 3), "pid=");
}

fn test_kill_flag_order_parity() {
    let cmd = KillBuilder::new().signal("-9").pid(1234).build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [sig] [pid]
    assert_eq!(get_arg(&args, 0), "-9");
    assert_eq!(get_arg(&args, 1), "1234");
}

fn test_hostname_flag_order_parity() {
    let cmd = HostnameBuilder::new().build();
    let args_count = cmd.get_args().count();
    // Snapshotted: (no args)
    assert_eq!(args_count, 0);
}

fn test_taskkill_flag_order_parity() {
    let cmd = TaskkillBuilder::new().pid(1234).force(true).build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: /PID [pid] /F
    assert_eq!(get_arg(&args, 0), "/PID");
    assert_eq!(get_arg(&args, 1), "1234");
    assert_eq!(get_arg(&args, 2), "/F");
}

fn test_ffprobe_pattern_safety_hardening() {
    // Filename with brackets SHOULD trigger -pattern_type none automatically
    let cmd = foundation::ffmpeg_builder::FfprobeBuilder::new()
        .input(Path::new("frame[01].png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    assert_eq!(get_arg(&args, 0), "-pattern_type");
    assert_eq!(get_arg(&args, 1), "none");
    assert_eq!(get_arg(&args, 2), "--");
    assert!(get_arg(&args, 3).contains("frame[01].png"));
}

fn test_ffmpeg_odd_dim_correction_hardening() {
    let cmd = FfmpegBuilder::new()
        .with_odd_dim_correction() // Force correction
        .filter_complex("ssim")
        .input(Path::new("in.mp4"))
        .output(Path::new("out.mp4"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    let filter_idx = args
        .iter()
        .position(|r| r == "-filter_complex")
        .unwrap_or_else(|| panic!("-filter_complex not found"));
    // Should prepend scaling: scale=trunc(iw/2)*2:trunc(ih/2)*2,ssim
    assert!(get_arg(&args, filter_idx + 1).starts_with("scale=trunc(iw/2)*2:trunc(ih/2)*2,"));
    assert!(get_arg(&args, filter_idx + 1).contains("ssim"));
}

fn test_magick_path_armor_hardening() {
    let cmd = MagickBuilder::new()
        .input(Path::new("img%1.jpg"))
        .output(Path::new("out.png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    // Should have ./ (protocol-less relative) and %% escaping.
    // ImageMagick 7 uses `magick -- [in] ...`; ImageMagick 6 uses `[in] ...`.
    let input_idx = usize::from(args.first().is_some_and(|arg| arg == "--"));
    assert_eq!(get_arg(&args, input_idx), "./img%%1.jpg");
}

fn test_exiftool_nuclear_strip_hardening() {
    let cmd = ExiftoolBuilder::new()
        .strip_all()
        .input(Path::new("in.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    assert!(args.contains(&"-all=".to_string()));
}

fn test_ffmpeg_global_flag_priority_parity() {
    let cmd = FfmpegBuilder::new()
        .overwrite()
        .hide_banner()
        .loglevel("error")
        .input(Path::new("in.mp4"))
        .output(Path::new("out.mp4"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    // Priority check: -y and -hide_banner MUST come before -i
    let y_idx = args
        .iter()
        .position(|r| r == "-y")
        .unwrap_or_else(|| panic!("-y not found"));
    let hb_idx = args
        .iter()
        .position(|r| r == "-hide_banner")
        .unwrap_or_else(|| panic!("-hide_banner not found"));
    let i_idx = args
        .iter()
        .position(|r| r == "-i")
        .unwrap_or_else(|| panic!("-i not found"));

    assert!(y_idx < i_idx);
    assert!(hb_idx < i_idx);
}

fn test_sips_quality_clamping_hardening() {
    let cmd = foundation::image_builders::SipsBuilder::new()
        .quality(150) // Should clamp to 100
        .format("jpeg")
        .input(Path::new("in.png"))
        .output(Path::new("out.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    // Match: -s format jpeg -s formatOptions 100 in.png --out out.jpg
    assert_eq!(get_arg(&args, 0), "-s");
    assert_eq!(get_arg(&args, 1), "format");
    assert_eq!(get_arg(&args, 2), "jpeg");
    assert_eq!(get_arg(&args, 3), "-s");
    assert_eq!(get_arg(&args, 4), "formatOptions");
    assert_eq!(get_arg(&args, 5), "100");
}

fn test_vmaf_comprehensive_hardening() {
    let cmd = foundation::tool_builders::VmafBuilder::new()
        .threads(8)
        .model("vmaf_v0.6.1.json")
        .reference(Path::new("ref.mp4"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    assert!(args.contains(&"--thread".to_string()));
    assert!(args.contains(&"8".to_string()));
    assert!(args.contains(&"--model".to_string()));
    assert!(args.contains(&"vmaf_v0.6.1.json".to_string()));
}

fn test_identify_verbose_hardening() {
    let cmd = foundation::image_builders::IdentifyBuilder::new()
        .verbose(true)
        .format("%w %h")
        .input(Path::new("in.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    assert!(args.contains(&"-verbose".to_string()));
    assert!(args.contains(&"-format".to_string()));
    assert!(args.contains(&"%w %h".to_string()));
}

fn test_gifski_performance_controls() {
    let cmd = foundation::image_builders::GifskiBuilder::new()
        .fast(true)
        .quality(85)
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    assert!(args.contains(&"--fast".to_string()));
    assert!(args.contains(&"--quality".to_string()));
    assert!(args.contains(&"85".to_string()));
}

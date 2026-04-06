// parity_tests.rs

use crate::ffmpeg_builder::{EncoderPreset, FfmpegBuilder, VideoCodec};
use crate::image_builders::*;
use crate::jxl_builder::{CjxlBuilder, DjxlBuilder};
use crate::tool_builders::*;
use std::path::Path;

#[test]
fn test_ffmpeg_flag_order_parity() {
    let cmd = FfmpegBuilder::new()
        .overwrite()
        .threads(4)
        .input(Path::new("in.mp4"))
        .vcodec(VideoCodec::Hevc)
        .crf(18.0)
        .preset(EncoderPreset::Slower)
        .build();

    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted order from commit 73edfa6: -y -threads [N] -i [in] [opts] [out]
    assert_eq!(args[0], "-y");
    assert_eq!(args[1], "-threads");
    assert_eq!(args[2], "4");
    assert_eq!(args[3], "-i");
    assert!(args[4].contains("in.mp4"));
    assert_eq!(args[5], "-c:v");
    assert!(args[6].contains("libx265") || args[6].contains("hevc"));
    assert_eq!(args[7], "-crf");
    assert_eq!(args[9], "-preset");
}

#[test]
fn test_ffprobe_flag_order_parity() {
    let cmd = crate::ffmpeg_builder::FfprobeBuilder::new()
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
    assert_eq!(args[0], "-v");
    assert_eq!(args[1], "error");
    assert_eq!(args[2], "-show_entries");
    assert_eq!(args[3], "format=duration");
    assert_eq!(args[4], "--");
    assert!(args[5].contains("in.mp4"));
}

#[test]
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
    // Snapshotted order from commit 73edfa6: [in] [out] [flags]
    assert!(args[0].contains("in.png"));
    assert!(args[1].contains("out.jxl"));
    assert_eq!(args[2], "-d");
    assert_eq!(args[3], "0.5");
    assert_eq!(args[4], "-e");
    assert_eq!(args[5], "7");
}

#[test]
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
    assert!(args[0].contains("in.jxl"));
    assert!(args[1].contains("out.png"));
}

#[test]
fn test_jxlinfo_flag_order_parity() {
    let cmd = JxlinfoBuilder::new().input(Path::new("in.jxl")).build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [in]
    assert!(args[0].contains("in.jxl"));
}

#[test]
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
    // Snapshotted: -- [in] [flags] [out]
    assert_eq!(args[0], "--");
    assert!(args[1].contains("in.jpg"));
    assert_eq!(args[2], "-quality");
    assert_eq!(args[3], "85");
    assert!(args[4].contains("out.png"));
}

#[test]
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
    assert_eq!(args[0], "-verbose");
    assert!(args[1].contains("in.jpg"));
}

#[test]
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
    assert_eq!(args[0], "-s");
    assert_eq!(args[2], "jpeg");
    assert!(args[3].contains("in.png"));
    assert_eq!(args[4], "--out");
    assert!(args[5].contains("out.jpg"));
}

#[test]
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
    assert_eq!(args[0], "-get");
    assert_eq!(args[1], "icc");
    assert!(args[2].contains("in.webp"));
    assert_eq!(args[3], "-o");
    assert!(args[4].contains("out.icc"));
}

#[test]
fn test_gifski_flag_order_parity() {
    let cmd = GifskiBuilder::new()
        .arg("--quality")
        .arg("90")
        .output(Path::new("out.gif"))
        .add_input(Path::new("frame.png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] -o [out] [in]
    assert_eq!(args[0], "--quality");
    assert_eq!(args[2], "-o");
    assert!(args[3].contains("out.gif"));
    assert!(args[4].contains("frame.png"));
}

#[test]
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
    assert_eq!(args[0], "--speed");
    assert!(args[2].contains("in.png"));
    assert!(args[3].contains("out.avif"));
}

#[test]
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
    assert!(args[0].contains("in.webp"));
    assert_eq!(args[1], "-lossless");
    assert_eq!(args[2], "-o");
    assert!(args[3].contains("out.png"));
}

#[test]
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
    assert_eq!(args[0], "-icc_profile");
    assert_eq!(args[1], "-b");
    assert!(args[2].contains("in.jpg"));
}

#[test]
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
    assert_eq!(args[0], "-pt");
    assert!(args[1].contains("in.jpg"));
}

#[test]
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
    assert_eq!(args[0], "--reference");
    assert!(args[1].contains("ref.mp4"));
    assert_eq!(args[2], "--distorted");
    assert!(args[3].contains("dist.mp4"));
    assert_eq!(args[4], "--output");
    assert!(args[5].contains("out.json"));
}

#[test]
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
    assert_eq!(args[0], "--crf");
    assert_eq!(args[2], "--preset");
    assert_eq!(args[4], "--input");
    assert!(args[5].contains("in.y4m"));
    assert_eq!(args[6], "--output");
    assert!(args[7].contains("out.hevc"));
}

#[test]
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
    assert_eq!(args[0], "demux");
    assert_eq!(args[1], "-i");
    assert!(args[2].contains("in.hevc"));
    assert_eq!(args[3], "-o");
    assert!(args[4].contains("out.rpu"));
}

#[test]
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
    assert_eq!(args[0], "extract");
    assert_eq!(args[1], "-i");
    assert!(args[2].contains("in.hevc"));
    assert_eq!(args[3], "-o");
    assert!(args[4].contains("out.json"));
}

#[test]
fn test_osascript_flag_order_parity() {
    let cmd = OsascriptBuilder::new().script("return 1").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: -e [script]
    assert_eq!(args[0], "-e");
    assert_eq!(args[1], "return 1");
}

#[test]
fn test_powershell_flag_order_parity() {
    let cmd = PowershellBuilder::new().command("Get-Date").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: -NoProfile -NonInteractive -Command [command]
    assert_eq!(args[0], "-NoProfile");
    assert_eq!(args[1], "-NonInteractive");
    assert_eq!(args[2], "-Command");
    assert_eq!(args[3], "Get-Date");
}

#[test]
fn test_sysctl_flag_order_parity() {
    let cmd = SysctlBuilder::new().arg("-n").arg("hw.ncpu").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [key]
    assert_eq!(args[0], "-n");
    assert_eq!(args[1], "hw.ncpu");
}

#[test]
fn test_vm_stat_flag_order_parity() {
    let cmd = VmstatBuilder::new().build();
    let args_count = cmd.get_args().count();
    // Snapshotted: (no args)
    assert_eq!(args_count, 0);
}

#[test]
fn test_acl_getfacl_flag_order_parity() {
    let cmd = AclBuilder::getfacl().input(Path::new("test.txt")).build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [in]
    assert!(args[0].contains("test.txt"));
}

#[test]
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
    assert_eq!(args[0], "-m");
    assert_eq!(args[1], "u:user:rwx");
    assert!(args[2].contains("test.txt"));
}

#[test]
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
    assert_eq!(args[0], "+R");
    assert!(args[1].contains("test.txt"));
}

#[test]
fn test_rsync_flag_order_parity() {
    let cmd = RsyncBuilder::new()
        .arg("-av")
        .add_source(Path::new("src"))
        .destination(Path::new("dest"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [flags] [src] [dest]
    assert_eq!(args[0], "--protect-args");
    assert_eq!(args[1], "-av");
    assert!(args[2].contains("src"));
    assert!(args[3].contains("dest"));
}

#[test]
fn test_ps_flag_order_parity() {
    let cmd = PsBuilder::new().pid(1234).output_field("pid").build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: -p [pid] -o [fields]=
    assert_eq!(args[0], "-p");
    assert_eq!(args[1], "1234");
    assert_eq!(args[2], "-o");
    assert_eq!(args[3], "pid=");
}

#[test]
fn test_kill_flag_order_parity() {
    let cmd = KillBuilder::new().signal("-9").pid(1234).build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: [sig] [pid]
    assert_eq!(args[0], "-9");
    assert_eq!(args[1], "1234");
}

#[test]
fn test_hostname_flag_order_parity() {
    let cmd = HostnameBuilder::new().build();
    let args_count = cmd.get_args().count();
    // Snapshotted: (no args)
    assert_eq!(args_count, 0);
}

#[test]
fn test_taskkill_flag_order_parity() {
    let cmd = TaskkillBuilder::new().pid(1234).force().build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    // Snapshotted: /PID [pid] /F
    assert_eq!(args[0], "/PID");
    assert_eq!(args[1], "1234");
    assert_eq!(args[2], "/F");
}

#[test]
fn test_ffprobe_pattern_safety_hardening() {
    // Filename with brackets SHOULD trigger -pattern_type none automatically
    let cmd = crate::ffmpeg_builder::FfprobeBuilder::new()
        .input(Path::new("frame[01].png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    assert_eq!(args[0], "-pattern_type");
    assert_eq!(args[1], "none");
    assert_eq!(args[2], "--");
    assert!(args[3].contains("frame[01].png"));
}

#[test]
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

    let filter_idx = args.iter().position(|r| r == "-filter_complex").unwrap();
    // Should prepend scaling: scale=trunc(iw/2)*2:trunc(ih/2)*2,ssim
    assert!(args[filter_idx + 1].starts_with("scale=trunc(iw/2)*2:trunc(ih/2)*2,"));
    assert!(args[filter_idx + 1].contains("ssim"));
}

#[test]
fn test_magick_path_armor_hardening() {
    let cmd = MagickBuilder::new()
        .input(Path::new("img%1.jpg"))
        .output(Path::new("out.png"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    // Should have ./ (protocol-less relative) and %% escaping
    assert_eq!(args[0], "--");
    assert_eq!(args[1], "./img%%1.jpg");
}

#[test]
fn test_exiftool_nuclear_strip_hardening() {
    let cmd = ExiftoolBuilder::new()
        .strip_all()
        .ignore_minor()
        .input(Path::new("in.jpg"))
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();

    assert!(args.contains(&"-all=".to_string()));
    assert!(args.contains(&"-m".to_string()));
}

#[test]
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
    let y_idx = args.iter().position(|r| r == "-y").unwrap();
    let hb_idx = args.iter().position(|r| r == "-hide_banner").unwrap();
    let i_idx = args.iter().position(|r| r == "-i").unwrap();

    assert!(y_idx < i_idx);
    assert!(hb_idx < i_idx);
}

#[test]
fn test_sips_quality_clamping_hardening() {
    let cmd = crate::image_builders::SipsBuilder::new()
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
    assert_eq!(args[0], "-s");
    assert_eq!(args[1], "format");
    assert_eq!(args[2], "jpeg");
    assert_eq!(args[3], "-s");
    assert_eq!(args[4], "formatOptions");
    assert_eq!(args[5], "100");
}

#[test]
fn test_vmaf_comprehensive_hardening() {
    let cmd = crate::tool_builders::VmafBuilder::new()
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

#[test]
fn test_identify_verbose_hardening() {
    let cmd = crate::image_builders::IdentifyBuilder::new()
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

#[test]
fn test_gifski_performance_controls() {
    let cmd = crate::image_builders::GifskiBuilder::new()
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

#[test]
fn test_avifenc_quality_refinement() {
    let cmd = crate::image_builders::AvifencBuilder::new()
        .quality(15, 35)
        .speed(8)
        .build();
    let args: Vec<String> = cmd
        .get_args()
        .map(|s: &std::ffi::OsStr| s.to_string_lossy().to_string())
        .collect();
    assert!(args.contains(&"--min".to_string()));
    assert!(args.contains(&"15".to_string()));
    assert!(args.contains(&"--max".to_string()));
    assert!(args.contains(&"35".to_string()));
    assert!(args.contains(&"--speed".to_string()));
    assert!(args.contains(&"8".to_string()));
}

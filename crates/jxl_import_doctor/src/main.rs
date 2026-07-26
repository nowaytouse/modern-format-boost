use clap::{Parser, ValueEnum};
use jxl_oxide::{JpegReconstructionStatus, JxlImage};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use walkdir::WalkDir;

const EXIT_BOUNDARY: i32 = 2;
const EXIT_COUNT_GATE: i32 = 3;
const DEFAULT_EXPECTED_MIN: usize = 1400;
const DEFAULT_EXPECTED_MAX: usize = 1499;
const JXL_SIGNATURE_BOX: &[u8; 12] = b"\0\0\0\x0cJXL \r\n\x87\n";
const REPAIR_MANIFEST: &str = "MFB_JXL_REPAIR_MANIFEST.json";

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Probe {
    Status,
    Reconstruct,
    Render,
    Full,
    Djxl,
    Correlate,
}

#[derive(Debug, Parser)]
#[command(about = "Read-only JPEG XL import diagnostics with a mandatory affected-count gate")]
struct Args {
    /// Local folder containing the JPEG XL files to inspect.
    input_dir: PathBuf,

    /// Lowest accepted affected-file count (inclusive).
    #[arg(long, default_value_t = DEFAULT_EXPECTED_MIN)]
    expected_min: usize,

    /// Highest accepted affected-file count (inclusive).
    #[arg(long, default_value_t = DEFAULT_EXPECTED_MAX)]
    expected_max: usize,

    /// How deeply to validate each JPEG XL file.
    #[arg(long, value_enum, default_value_t = Probe::Status)]
    probe: Probe,

    /// Explicit local djxl binary used only by the djxl probe.
    #[arg(long, value_name = "PATH")]
    djxl: Option<PathBuf>,

    /// Write repaired copies into this new direct child of `INPUT_DIR`.
    /// Originals are never modified. Requires an accepted 14xx count and djxl probe.
    #[arg(long, value_name = "NEW_FOLDER")]
    repair_output: Option<PathBuf>,
}

#[derive(Debug)]
struct Finding {
    path: PathBuf,
    reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BoxSpan {
    start: usize,
    end: usize,
    kind: [u8; 4],
}

#[derive(Debug)]
struct RepairPlan {
    source: PathBuf,
    relative: PathBuf,
    source_len: usize,
    source_blake3: String,
    jbrd: BoxSpan,
}

#[derive(Debug, Serialize)]
struct RepairManifest {
    format_version: u32,
    source_root: String,
    output_root: String,
    approved_affected_count: usize,
    repair: &'static str,
    guarantees: RepairGuarantees,
    files: Vec<RepairRecord>,
}

#[derive(Debug, Serialize)]
#[allow(clippy::struct_excessive_bools)]
struct RepairGuarantees {
    originals_untouched: bool,
    retained_bytes_exact: bool,
    jpeg_xl_pixel_decode_verified: bool,
    local_apple_imageio_verified: bool,
    photos_library_accessed: bool,
    media_uploaded: bool,
}

#[derive(Debug, Serialize)]
struct RepairRecord {
    relative_path: String,
    source_blake3: String,
    repaired_blake3: String,
    source_bytes: usize,
    repaired_bytes: usize,
    removed_jbrd_bytes: usize,
}

fn main() {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => {}
        Err((code, message)) => {
            eprintln!("{message}");
            std::process::exit(code);
        }
    }
}

fn run(args: &Args) -> Result<(), (i32, String)> {
    validate_count_gate(args.expected_min, args.expected_max)
        .map_err(|message| (EXIT_BOUNDARY, message))?;
    let input_dir =
        canonical_safe_input_dir(&args.input_dir).map_err(|message| (EXIT_BOUNDARY, message))?;
    let repair_output = args
        .repair_output
        .as_deref()
        .map(|path| validate_repair_output(&input_dir, path, args))
        .transpose()
        .map_err(|message| (EXIT_BOUNDARY, message))?;
    let files = collect_jxl_files(&input_dir).map_err(|message| (EXIT_BOUNDARY, message))?;
    let djxl = resolve_djxl(args.probe, args.djxl.as_deref())
        .map_err(|message| (EXIT_BOUNDARY, message))?;

    let previous_panic_hook = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let findings: Vec<Finding> = files
        .par_iter()
        .filter_map(|path| inspect(path, args.probe, djxl.as_deref()))
        .collect();
    panic::set_hook(previous_panic_hook);

    let mut reasons = BTreeMap::<&str, usize>::new();
    for finding in &findings {
        *reasons.entry(finding.reason.as_str()).or_default() += 1;
    }

    if repair_output.is_some() {
        println!("mode=preflight_then_new_folder_repair");
        println!("originals=untouched");
    } else {
        println!("mode=read-only");
    }
    println!("input={}", input_dir.display());
    println!("probe={:?}", args.probe);
    println!("affected={}", findings.len());
    println!("count_policy=required_14xx");
    for (reason, count) in reasons {
        println!("reason.{reason}={count}");
    }
    for finding in findings.iter().take(20) {
        println!("sample={}\t{}", finding.reason, finding.path.display());
    }

    if !count_is_accepted(findings.len(), args.expected_min, args.expected_max) {
        return Err((
            EXIT_COUNT_GATE,
            format!(
                "count_gate=REJECTED: affected count does not match the required {}..={} boundary; no files were written",
                args.expected_min, args.expected_max
            ),
        ));
    }

    println!("count_gate=ACCEPTED");

    if let Some(output) = repair_output {
        let djxl = djxl.as_deref().ok_or_else(|| {
            (
                EXIT_BOUNDARY,
                "repair requires an explicitly resolved local djxl binary".to_owned(),
            )
        })?;
        let records = repair_findings(&input_dir, &output, &findings, djxl)
            .map_err(|message| (EXIT_BOUNDARY, message))?;
        println!("repair_output={}", output.display());
        println!("repaired={}", records.len());
        println!("repair_manifest={}", output.join(REPAIR_MANIFEST).display());
    }
    Ok(())
}

fn validate_count_gate(expected_min: usize, expected_max: usize) -> Result<(), String> {
    if expected_min < DEFAULT_EXPECTED_MIN
        || expected_max > DEFAULT_EXPECTED_MAX
        || expected_min > expected_max
    {
        return Err("the accepted affected-count gate must stay within 1400..=1499".to_owned());
    }
    Ok(())
}

fn count_is_accepted(affected: usize, expected_min: usize, expected_max: usize) -> bool {
    (expected_min..=expected_max).contains(&affected)
}

fn canonical_safe_input_dir(path: &Path) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve input folder {}: {error}", path.display()))?;
    if !canonical.is_dir() {
        return Err(format!("input is not a folder: {}", canonical.display()));
    }
    if canonical.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".photoslibrary")
    }) {
        return Err("Photos libraries are forbidden inputs".to_owned());
    }
    Ok(canonical)
}

fn validate_repair_output(
    input_dir: &Path,
    requested: &Path,
    args: &Args,
) -> Result<PathBuf, String> {
    if !matches!(args.probe, Probe::Djxl) {
        return Err("repair requires --probe djxl".to_owned());
    }
    if args.djxl.is_none() {
        return Err("repair requires an explicit local --djxl PATH".to_owned());
    }
    if requested.exists() {
        return Err(format!(
            "repair output must be a new folder and will never be overwritten: {}",
            requested.display()
        ));
    }
    if requested.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".photoslibrary")
    }) {
        return Err("Photos libraries are forbidden repair outputs".to_owned());
    }

    let name = requested
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "repair output must have a folder name".to_owned())?;
    let parent = requested
        .parent()
        .ok_or_else(|| "repair output must have a parent folder".to_owned())?
        .canonicalize()
        .map_err(|error| {
            format!(
                "cannot resolve repair-output parent {}: {error}",
                requested.display()
            )
        })?;
    if parent != input_dir {
        return Err(format!(
            "repair output must be a new direct child of the approved input folder {}",
            input_dir.display()
        ));
    }

    let output = input_dir.join(name);
    if output.exists() {
        return Err(format!(
            "repair output must not already exist: {}",
            output.display()
        ));
    }
    Ok(output)
}

fn collect_jxl_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| format!("failed to walk {}: {error}", root.display()))?;
        if entry.file_type().is_symlink() {
            return Err(format!(
                "symbolic links are forbidden inside the input folder: {}",
                entry.path().display()
            ));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let is_jxl = entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jxl"));
        if is_jxl {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn repair_findings(
    input_dir: &Path,
    output_dir: &Path,
    findings: &[Finding],
    djxl: &Path,
) -> Result<Vec<RepairRecord>, String> {
    let approved_count = findings.len();
    if !(DEFAULT_EXPECTED_MIN..=DEFAULT_EXPECTED_MAX).contains(&approved_count) {
        return Err(
            "repair refused: preflight affected count must stay within 1400..=1499".to_owned(),
        );
    }
    if findings
        .iter()
        .any(|finding| finding.reason != "djxl_jpeg_reconstruction_failed")
    {
        return Err(
            "repair refused: every affected file must be an isolated djxl JPEG-reconstruction failure"
                .to_owned(),
        );
    }

    // Complete every structural and source-hash check before creating the output folder.
    let mut plans = findings
        .iter()
        .map(|finding| plan_jbrd_removal(input_dir, &finding.path))
        .collect::<Result<Vec<_>, _>>()?;
    plans.sort_by(|left, right| left.relative.cmp(&right.relative));
    if plans.len() != approved_count {
        return Err("repair refused: structural preflight count changed".to_owned());
    }

    fs::create_dir(output_dir).map_err(|error| {
        format!(
            "cannot create new repair output folder {}: {error}",
            output_dir.display()
        )
    })?;

    let mut records = Vec::with_capacity(plans.len());
    for plan in &plans {
        records.push(write_repaired_copy(output_dir, plan, djxl)?);
    }

    if records.len() != approved_count {
        return Err("repair output count changed before manifest creation".to_owned());
    }

    let manifest = RepairManifest {
        format_version: 1,
        source_root: input_dir.display().to_string(),
        output_root: output_dir.display().to_string(),
        approved_affected_count: approved_count,
        repair: "removed exactly one invalid top-level jbrd box; all other bytes retained in order",
        guarantees: RepairGuarantees {
            originals_untouched: true,
            retained_bytes_exact: true,
            jpeg_xl_pixel_decode_verified: true,
            local_apple_imageio_verified: true,
            photos_library_accessed: false,
            media_uploaded: false,
        },
        files: records,
    };
    write_manifest(output_dir, &manifest)?;
    Ok(manifest.files)
}

fn plan_jbrd_removal(input_dir: &Path, source: &Path) -> Result<RepairPlan, String> {
    let relative = source.strip_prefix(input_dir).map_err(|_| {
        format!(
            "repair source escaped the approved input folder: {}",
            source.display()
        )
    })?;
    let bytes = fs::read(source)
        .map_err(|error| format!("cannot read repair source {}: {error}", source.display()))?;
    let (repaired, jbrd) = remove_exactly_one_jbrd(&bytes)
        .map_err(|error| format!("{}: {error}", source.display()))?;
    verify_repaired_bytes(&repaired).map_err(|error| format!("{}: {error}", source.display()))?;

    Ok(RepairPlan {
        source: source.to_owned(),
        relative: relative.to_owned(),
        source_len: bytes.len(),
        source_blake3: blake3::hash(&bytes).to_hex().to_string(),
        jbrd,
    })
}

fn write_repaired_copy(
    output_dir: &Path,
    plan: &RepairPlan,
    djxl: &Path,
) -> Result<RepairRecord, String> {
    let source_bytes = fs::read(&plan.source).map_err(|error| {
        format!(
            "cannot reread repair source {}: {error}",
            plan.source.display()
        )
    })?;
    let current_hash = blake3::hash(&source_bytes).to_hex().to_string();
    if source_bytes.len() != plan.source_len || current_hash != plan.source_blake3 {
        return Err(format!(
            "repair stopped because a source changed after preflight: {}",
            plan.source.display()
        ));
    }

    let (repaired_bytes, removed) = remove_exactly_one_jbrd(&source_bytes)
        .map_err(|error| format!("{}: {error}", plan.source.display()))?;
    if removed != plan.jbrd {
        return Err(format!(
            "repair stopped because the jbrd layout changed after preflight: {}",
            plan.source.display()
        ));
    }
    if repaired_bytes.len() + (removed.end - removed.start) != source_bytes.len() {
        return Err(format!(
            "repair byte-accounting mismatch: {}",
            plan.source.display()
        ));
    }

    let destination = output_dir.join(&plan.relative);
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "repair destination has no parent: {}",
            destination.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "cannot create repair subfolder {}: {error}",
            parent.display()
        )
    })?;

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            format!(
                "repair destination already exists or cannot be created {}: {error}",
                destination.display()
            )
        })?;
    let write_result = (|| -> Result<(), String> {
        let mut writer = BufWriter::new(file);
        writer.write_all(&repaired_bytes).map_err(|error| {
            format!(
                "cannot write repaired copy {}: {error}",
                destination.display()
            )
        })?;
        writer.flush().map_err(|error| {
            format!(
                "cannot flush repaired copy {}: {error}",
                destination.display()
            )
        })?;
        writer.get_ref().sync_all().map_err(|error| {
            format!(
                "cannot sync repaired copy {}: {error}",
                destination.display()
            )
        })?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }

    let permissions_result = fs::metadata(&plan.source)
        .and_then(|metadata| fs::set_permissions(&destination, metadata.permissions()));
    if let Err(error) = permissions_result {
        let _ = fs::remove_file(&destination);
        return Err(format!(
            "cannot preserve permissions for repaired copy {}: {error}",
            destination.display()
        ));
    }
    if let Err(error) = verify_repaired_copy(&destination, djxl) {
        let _ = fs::remove_file(&destination);
        return Err(error);
    }

    Ok(RepairRecord {
        relative_path: plan.relative.to_string_lossy().into_owned(),
        source_blake3: plan.source_blake3.clone(),
        repaired_blake3: blake3::hash(&repaired_bytes).to_hex().to_string(),
        source_bytes: source_bytes.len(),
        repaired_bytes: repaired_bytes.len(),
        removed_jbrd_bytes: removed.end - removed.start,
    })
}

fn verify_repaired_copy(path: &Path, djxl: &Path) -> Result<(), String> {
    let parsed = panic::catch_unwind(AssertUnwindSafe(|| JxlImage::builder().open(path)))
        .map_err(|_| format!("independent JPEG XL parser panicked for {}", path.display()))?
        .map_err(|error| {
            format!(
                "independent JPEG XL parser rejected {}: {error}",
                path.display()
            )
        })?;
    if parsed.jpeg_reconstruction_status() != JpegReconstructionStatus::Unavailable {
        return Err(format!(
            "repaired copy still advertises JPEG reconstruction data: {}",
            path.display()
        ));
    }

    let pixel_decode_ok = Command::new(djxl)
        .arg(path)
        .arg("-")
        .args(["--output_format", "png", "--quiet", "--num_threads=0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("cannot launch local djxl for {}: {error}", path.display()))?
        .success();
    if !pixel_decode_ok {
        return Err(format!(
            "local djxl pixel decode rejected repaired copy {}",
            path.display()
        ));
    }

    let imageio_ok = Command::new("/usr/bin/sips")
        .args(["-g", "pixelWidth", "-g", "pixelHeight"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| {
            format!(
                "cannot launch local Apple ImageIO check for {}: {error}",
                path.display()
            )
        })?
        .success();
    if !imageio_ok {
        return Err(format!(
            "local Apple ImageIO rejected repaired copy {}",
            path.display()
        ));
    }
    Ok(())
}

fn verify_repaired_bytes(bytes: &[u8]) -> Result<(), String> {
    let parsed = panic::catch_unwind(AssertUnwindSafe(|| JxlImage::builder().read(bytes)))
        .map_err(|_| "independent JPEG XL parser panicked after jbrd removal".to_owned())?
        .map_err(|error| {
            format!("independent JPEG XL parser rejected bytes after jbrd removal: {error}")
        })?;
    if parsed.jpeg_reconstruction_status() != JpegReconstructionStatus::Unavailable {
        return Err("bytes still advertise JPEG reconstruction after jbrd removal".to_owned());
    }
    Ok(())
}

fn write_manifest(output_dir: &Path, manifest: &RepairManifest) -> Result<(), String> {
    let path = output_dir.join(REPAIR_MANIFEST);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("cannot create repair manifest {}: {error}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, manifest)
        .map_err(|error| format!("cannot serialize repair manifest: {error}"))?;
    writer
        .write_all(b"\n")
        .and_then(|()| writer.flush())
        .map_err(|error| format!("cannot write repair manifest {}: {error}", path.display()))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| format!("cannot sync repair manifest {}: {error}", path.display()))?;
    Ok(())
}

fn remove_exactly_one_jbrd(bytes: &[u8]) -> Result<(Vec<u8>, BoxSpan), String> {
    let spans = parse_top_level_boxes(bytes)?;
    let jbrd = exactly_one_jbrd(&spans)?;
    let mut repaired = Vec::with_capacity(bytes.len() - (jbrd.end - jbrd.start));
    repaired.extend_from_slice(&bytes[..jbrd.start]);
    repaired.extend_from_slice(&bytes[jbrd.end..]);

    let repaired_spans = parse_top_level_boxes(&repaired)?;
    if repaired_spans.iter().any(|span| span.kind == *b"jbrd") {
        return Err("jbrd removal verification failed".to_owned());
    }
    if repaired != [&bytes[..jbrd.start], &bytes[jbrd.end..]].concat() {
        return Err("retained-byte verification failed".to_owned());
    }
    Ok((repaired, jbrd))
}

fn exactly_one_jbrd(spans: &[BoxSpan]) -> Result<BoxSpan, String> {
    let mut matches = spans.iter().copied().filter(|span| span.kind == *b"jbrd");
    let jbrd = matches
        .next()
        .ok_or_else(|| "expected exactly one top-level jbrd box, found none".to_owned())?;
    if matches.next().is_some() {
        return Err("expected exactly one top-level jbrd box, found multiple".to_owned());
    }
    Ok(jbrd)
}

fn parse_top_level_boxes(bytes: &[u8]) -> Result<Vec<BoxSpan>, String> {
    if bytes.len() < JXL_SIGNATURE_BOX.len()
        || &bytes[..JXL_SIGNATURE_BOX.len()] != JXL_SIGNATURE_BOX
    {
        return Err("not a JPEG XL container signature".to_owned());
    }

    let mut spans = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = bytes.len() - offset;
        if remaining < 8 {
            return Err(format!("truncated box header at byte {offset}"));
        }
        let size32 = u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]);
        let kind = [
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ];
        let (header_len, box_len) = match size32 {
            0 => (8usize, remaining),
            1 => {
                if remaining < 16 {
                    return Err(format!("truncated extended box header at byte {offset}"));
                }
                let size64 = u64::from_be_bytes([
                    bytes[offset + 8],
                    bytes[offset + 9],
                    bytes[offset + 10],
                    bytes[offset + 11],
                    bytes[offset + 12],
                    bytes[offset + 13],
                    bytes[offset + 14],
                    bytes[offset + 15],
                ]);
                let size = usize::try_from(size64)
                    .map_err(|_| format!("box at byte {offset} is too large"))?;
                (16usize, size)
            }
            size => (8usize, size as usize),
        };
        if box_len < header_len {
            return Err(format!("invalid box size {box_len} at byte {offset}"));
        }
        let end = offset
            .checked_add(box_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format!("box at byte {offset} exceeds file length"))?;
        spans.push(BoxSpan {
            start: offset,
            end,
            kind,
        });
        offset = end;
        if size32 == 0 && offset != bytes.len() {
            return Err("zero-sized box was not final".to_owned());
        }
    }
    Ok(spans)
}

fn inspect(path: &Path, probe: Probe, djxl: Option<&Path>) -> Option<Finding> {
    match panic::catch_unwind(AssertUnwindSafe(|| inspect_inner(path, probe, djxl))) {
        Ok(finding) => finding,
        Err(_) => Some(finding(path, "decoder_panicked")),
    }
}

fn inspect_inner(path: &Path, probe: Probe, djxl: Option<&Path>) -> Option<Finding> {
    if matches!(probe, Probe::Djxl) {
        let Some(djxl) = djxl else {
            return Some(finding(path, "djxl_not_configured"));
        };
        return inspect_with_djxl(path, djxl);
    }
    if matches!(probe, Probe::Correlate) {
        let Some(djxl) = djxl else {
            return Some(finding(path, "djxl_not_configured"));
        };
        match djxl_reconstructs(path, djxl) {
            Ok(true) => return None,
            Ok(false) => {}
            Err(()) => return Some(finding(path, "djxl_launch_failed")),
        }
    }

    let image = match JxlImage::builder().open(path) {
        Ok(image) => image,
        Err(_) => return Some(finding(path, "parse_failed")),
    };

    match image.jpeg_reconstruction_status() {
        JpegReconstructionStatus::Available => {}
        JpegReconstructionStatus::Invalid => {
            return Some(finding(path, "reconstruction_invalid"));
        }
        JpegReconstructionStatus::NeedMoreData => {
            return Some(finding(path, "reconstruction_incomplete"));
        }
        JpegReconstructionStatus::Unavailable => {
            return Some(finding(path, "reconstruction_unavailable"));
        }
    }

    if matches!(probe, Probe::Reconstruct | Probe::Full | Probe::Correlate)
        && image.reconstruct_jpeg(io::sink()).is_err()
    {
        return Some(finding(path, "reconstruction_failed"));
    }

    if matches!(probe, Probe::Render | Probe::Full) && image.render_frame(0).is_err() {
        return Some(finding(path, "render_failed"));
    }

    None
}

fn inspect_with_djxl(path: &Path, djxl: &Path) -> Option<Finding> {
    match djxl_reconstructs(path, djxl) {
        Ok(true) => None,
        Ok(false) => Some(finding(path, "djxl_jpeg_reconstruction_failed")),
        Err(()) => Some(finding(path, "djxl_launch_failed")),
    }
}

fn djxl_reconstructs(path: &Path, djxl: &Path) -> Result<bool, ()> {
    Command::new(djxl)
        .arg("-J")
        .arg(path)
        .arg("-")
        .args(["--output_format", "jpg", "--quiet", "--num_threads=0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|_| ())
}

fn resolve_djxl(probe: Probe, path: Option<&Path>) -> Result<Option<PathBuf>, String> {
    if !matches!(probe, Probe::Djxl | Probe::Correlate) {
        return Ok(None);
    }
    let path = path.ok_or_else(|| "the djxl probe requires --djxl PATH".to_owned())?;
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve djxl binary {}: {error}", path.display()))?;
    if !canonical.is_file() {
        return Err(format!("djxl path is not a file: {}", canonical.display()));
    }
    Ok(Some(canonical))
}

fn finding(path: &Path, reason: &str) -> Finding {
    Finding {
        path: path.to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_args(input_dir: PathBuf) -> Args {
        Args {
            input_dir,
            expected_min: DEFAULT_EXPECTED_MIN,
            expected_max: DEFAULT_EXPECTED_MAX,
            probe: Probe::Djxl,
            djxl: Some(PathBuf::from("/local/djxl")),
            repair_output: None,
        }
    }

    fn iso_box(kind: [u8; 4], payload: &[u8]) -> Result<Vec<u8>, std::num::TryFromIntError> {
        let size = u32::try_from(8 + payload.len())?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&size.to_be_bytes());
        bytes.extend_from_slice(&kind);
        bytes.extend_from_slice(payload);
        Ok(bytes)
    }

    #[test]
    fn count_gate_cannot_escape_required_14xx_boundary() {
        assert!(validate_count_gate(1400, 1499).is_ok());
        assert!(validate_count_gate(1399, 1499).is_err());
        assert!(validate_count_gate(1400, 1500).is_err());
        assert!(validate_count_gate(1499, 1400).is_err());
        assert!(count_is_accepted(1400, 1400, 1499));
        assert!(count_is_accepted(1499, 1400, 1499));
        assert!(!count_is_accepted(1399, 1400, 1499));
        assert!(!count_is_accepted(1500, 1400, 1499));
        assert!(!count_is_accepted(1569, 1400, 1499));
    }

    #[test]
    fn cli_rejects_all_temporary_count_exceptions() {
        assert!(
            Args::try_parse_from([
                "jxl_import_doctor",
                "local-folder",
                "--temporary-approved-count",
                "1569",
            ])
            .is_err()
        );
    }

    #[test]
    fn jbrd_removal_preserves_every_other_byte() -> Result<(), Box<dyn std::error::Error>> {
        let mut input = JXL_SIGNATURE_BOX.to_vec();
        let before = iso_box(*b"jxlp", b"before")?;
        let jbrd = iso_box(*b"jbrd", b"invalid reconstruction data")?;
        let after = iso_box(*b"Exif", b"after")?;
        input.extend_from_slice(&before);
        input.extend_from_slice(&jbrd);
        input.extend_from_slice(&after);

        let (repaired, removed) = remove_exactly_one_jbrd(&input).map_err(std::io::Error::other)?;
        let mut expected = JXL_SIGNATURE_BOX.to_vec();
        expected.extend_from_slice(&before);
        expected.extend_from_slice(&after);

        assert_eq!(repaired, expected);
        assert_eq!(&input[removed.start..removed.end], jbrd);
        assert_eq!(removed.kind, *b"jbrd");
        Ok(())
    }

    #[test]
    fn jbrd_removal_rejects_missing_multiple_and_malformed_boxes()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut missing = JXL_SIGNATURE_BOX.to_vec();
        missing.extend_from_slice(&iso_box(*b"jxlp", b"pixels")?);
        assert!(remove_exactly_one_jbrd(&missing).is_err());

        let mut multiple = JXL_SIGNATURE_BOX.to_vec();
        multiple.extend_from_slice(&iso_box(*b"jbrd", b"one")?);
        multiple.extend_from_slice(&iso_box(*b"jbrd", b"two")?);
        assert!(remove_exactly_one_jbrd(&multiple).is_err());

        let mut malformed = JXL_SIGNATURE_BOX.to_vec();
        malformed.extend_from_slice(&100u32.to_be_bytes());
        malformed.extend_from_slice(b"jbrd");
        assert!(remove_exactly_one_jbrd(&malformed).is_err());
        Ok(())
    }

    #[test]
    fn repair_output_must_be_a_new_direct_child_of_input() -> Result<(), Box<dyn std::error::Error>>
    {
        let input = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let input_path = input.path().canonicalize()?;
        let args = test_args(input_path.clone());

        let allowed = input_path.join("MFB_Repaired_14xx");
        assert_eq!(
            validate_repair_output(&input_path, &allowed, &args).map_err(std::io::Error::other)?,
            allowed
        );
        assert!(
            validate_repair_output(
                &input_path,
                &outside.path().join("MFB_Repaired_14xx"),
                &args
            )
            .is_err()
        );

        let existing = input_path.join("already_exists");
        fs::create_dir(&existing)?;
        assert!(validate_repair_output(&input_path, &existing, &args).is_err());
        Ok(())
    }

    #[test]
    fn photos_library_input_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let photos_library = temp.path().join("Example.photoslibrary");
        std::fs::create_dir(&photos_library)?;
        let Err(error) = canonical_safe_input_dir(&photos_library) else {
            return Err("Photos library input must be rejected".into());
        };
        assert_eq!(error, "Photos libraries are forbidden inputs");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_rejected_without_following_them() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        symlink(outside.path(), temp.path().join("escape"))?;

        let Err(error) = collect_jxl_files(temp.path()) else {
            return Err("symlink must be rejected".into());
        };
        assert!(error.contains("symbolic links are forbidden"));
        Ok(())
    }
}

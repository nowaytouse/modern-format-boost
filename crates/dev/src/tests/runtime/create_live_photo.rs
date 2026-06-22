use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::tempdir;

#[test]
fn creates_jpg_live_photo_with_standard_commands() -> Result<()> {
    let fixture = Fixture::new()?;

    let out = run_live_photo(
        &fixture,
        &[fixture.input.to_str().context("utf8 input")?, "--no-meta"],
    )?;
    assert!(out.status.success(), "stdout:\n{}", stdout(&out));

    let commands = fixture.commands()?;
    assert!(
        commands
            .iter()
            .any(|line| line.contains("ffprobe -v error -show_entries format=duration"))
    );
    assert!(
        commands
            .iter()
            .any(|line| line.contains("ffprobe -v error -select_streams v:0"))
    );
    assert!(commands.iter().any(|line| {
        line.contains("ffmpeg -y -i")
            && line.contains("-vframes 1")
            && line.contains("-q:v 2")
            && line.contains(".JPG")
    }));
    assert!(commands.iter().any(|line| {
        line.contains("ffmpeg -y -i")
            && line.contains("-t 3.0")
            && line.contains("-c:v h264")
            && line.contains("-q:v 2")
            && line.contains(".MOV")
    }));
    assert!(!commands.iter().any(|line| line.starts_with("makelive ")));

    Ok(())
}

#[test]
fn creates_heic_hq_live_photo_with_metadata() -> Result<()> {
    let fixture = Fixture::new()?;
    let out_dir = fixture.temp.path().join("out");

    let out = run_live_photo(
        &fixture,
        &[
            fixture.input.to_str().context("utf8 input")?,
            "--format",
            "heic",
            "--hq",
            "--output",
            out_dir.to_str().context("utf8 output")?,
        ],
    )?;
    assert!(out.status.success(), "stdout:\n{}", stdout(&out));

    let commands = fixture.commands()?;
    assert!(commands.iter().any(|line| {
        line.contains("ffmpeg -y -i")
            && line.contains("-pix_fmt rgb24")
            && line.contains("_temp.png")
    }));
    assert!(commands.iter().any(|line| {
        line.starts_with("heif-enc --lossless -o")
            && line.contains(".HEIC")
            && line.contains("_temp.png")
    }));
    assert!(commands.iter().any(|line| {
        line.contains("ffmpeg -y -i")
            && line.contains("-t 3.0")
            && line.contains("-crf 18")
            && line.contains("-preset slow")
            && line.contains(".MOV")
    }));
    assert!(commands.iter().any(|line| {
        line.starts_with("makelive -p -v") && line.contains(".HEIC") && line.contains(".MOV")
    }));

    Ok(())
}

#[test]
fn fails_when_required_dependency_is_missing() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::remove_file(fixture.bin_dir.join("makelive"))?;

    let out = run_live_photo(&fixture, &[fixture.input.to_str().context("utf8 input")?])?;
    assert!(!out.status.success());
    assert!(stdout(&out).contains("Missing required dependencies: makelive"));

    Ok(())
}

#[test]
fn expands_tilde_in_input_path_like_python_path_expanduser() -> Result<()> {
    let fixture = Fixture::new()?;
    let home = fixture.temp.path().join("home");
    fs::create_dir(&home)?;
    fs::write(home.join("sample.mov"), b"fake video")?;

    let out = run_live_photo_with_env(
        &fixture,
        &["~/sample.mov", "--no-meta"],
        &[("HOME", home.to_str().context("utf8 home")?)],
    )?;
    assert!(out.status.success(), "stdout:\n{}", stdout(&out));

    Ok(())
}

struct Fixture {
    temp: tempfile::TempDir,
    bin_dir: PathBuf,
    log: PathBuf,
    input: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let temp = tempdir()?;
        let bin_dir = temp.path().join("bin");
        fs::create_dir(&bin_dir)?;
        let log = temp.path().join("commands.log");
        let input = temp.path().join("sample.mov");
        fs::write(&input, b"fake video")?;
        write_tool(
            &bin_dir.join("ffprobe"),
            &format!(
                r#"#!/bin/sh
tool=${{0##*/}}
printf '%s %s\n' "$tool" "$*" >> {}
case "$*" in
  *format=duration*) printf '5.25\n' ;;
  *stream=width,height*) printf '1920,1080\n' ;;
esac
"#,
                sh_quote(&log)
            ),
        )?;
        write_tool(
            &bin_dir.join("ffmpeg"),
            &format!(
                r#"#!/bin/sh
tool=${{0##*/}}
printf '%s %s\n' "$tool" "$*" >> {}
for out do :; done
: > "$out"
"#,
                sh_quote(&log)
            ),
        )?;
        write_tool(
            &bin_dir.join("heif-enc"),
            &format!(
                r#"#!/bin/sh
tool=${{0##*/}}
printf '%s %s\n' "$tool" "$*" >> {}
while [ "$1" != "" ]; do
  if [ "$1" = "-o" ]; then
    shift
    : > "$1"
    exit 0
  fi
  shift
done
"#,
                sh_quote(&log)
            ),
        )?;
        write_tool(
            &bin_dir.join("makelive"),
            &format!(
                r#"#!/bin/sh
tool=${{0##*/}}
printf '%s %s\n' "$tool" "$*" >> {}
exit 0
"#,
                sh_quote(&log)
            ),
        )?;
        Ok(Self {
            temp,
            bin_dir,
            log,
            input,
        })
    }

    fn commands(&self) -> Result<Vec<String>> {
        Ok(fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect())
    }
}

fn run_live_photo(fixture: &Fixture, args: &[&str]) -> Result<std::process::Output> {
    run_live_photo_with_env(fixture, args, &[])
}

fn run_live_photo_with_env(
    fixture: &Fixture,
    args: &[&str],
    env: &[(&str, &str)],
) -> Result<std::process::Output> {
    let os_args = args.iter().map(OsStr::new).collect::<Vec<_>>();
    run_live_photo_os(fixture, &os_args, env)
}

fn run_live_photo_os(
    fixture: &Fixture,
    args: &[&OsStr],
    env: &[(&str, &str)],
) -> Result<std::process::Output> {
    let bin = match std::env::var("CARGO_BIN_EXE_create_live_photo") {
        Ok(value) => value,
        Err(err) => bail!("missing CARGO_BIN_EXE_create_live_photo: {err}"),
    };
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env("PATH", &fixture.bin_dir)
        .env("MODERN_FORMAT_PLAIN_UI", "1")
        .current_dir(fixture.temp.path());
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().context("run create_live_photo")
}

fn write_tool(path: &Path, text: &str) -> Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(text.as_bytes())?;
    let mut perms = file.metadata()?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

fn sh_quote(path: &Path) -> String {
    let s = path.as_os_str().to_string_lossy();
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

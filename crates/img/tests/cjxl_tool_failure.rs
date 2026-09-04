use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use tempfile::tempdir;

// Run the CLI in a child process with an explicit failing `cjxl` override. This
// avoids process-global PATH mutation and ensures the valid JPEG reaches cjxl.

#[test]
fn cjxl_failure_marks_conversion_error() -> anyhow::Result<()> {
    let td = tempdir()?;
    let bin_dir = td.path().join("bin");
    fs::create_dir_all(&bin_dir)?;

    // Create a fake cjxl that passes tool discovery but fails real encoding.
    #[cfg(unix)]
    let cjxl_path = {
        let p = bin_dir.join("cjxl");
        fs::write(
            &p,
            "#!/bin/sh\ncase \"$1\" in --version|-version|--help|-h) echo 'JPEG XL encoder v0.13.0'; exit 0;; esac\necho 'Error while decoding the JPEG image' >&2\nexit 1\n",
        )?;
        let mut perms = fs::metadata(&p)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms)?;
        p
    };

    #[cfg(windows)]
    let cjxl_path = {
        let p = bin_dir.join("cjxl.bat");
        fs::write(
            &p,
            "@if \"%1\"==\"--version\" (echo JPEG XL encoder v0.13.0 & exit /b 0)\n@echo Error while decoding the JPEG image 1>&2\n@exit /b 1\n",
        )?;
        p
    };

    // Keep this test scoped to cjxl. A broken host ffmpeg installation must
    // not make the CLI fail during the required-tool preflight before the
    // deliberately failing encoder is reached.
    #[cfg(unix)]
    let ffmpeg_path = {
        let p = bin_dir.join("ffmpeg");
        fs::write(
            &p,
            "#!/bin/sh\ncase \"$1\" in --version|-version|--help|-h) echo 'ffmpeg version 9.0.1'; exit 0;; esac\necho 'unexpected ffmpeg invocation in cjxl failure test' >&2\nexit 1\n",
        )?;
        let mut perms = fs::metadata(&p)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&p, perms)?;
        p
    };

    #[cfg(windows)]
    let ffmpeg_path = {
        let p = bin_dir.join("ffmpeg.bat");
        fs::write(
            &p,
            "@if \"%1\"==\"--version\" (echo ffmpeg version 9.0.1 & exit /b 0)\n@if \"%1\"==\"-version\" (echo ffmpeg version 9.0.1 & exit /b 0)\n@echo unexpected ffmpeg invocation in cjxl failure test 1>&2\n@exit /b 1\n",
        )?;
        p
    };

    let src_dir = td.path().join("src");
    let output_dir = td.path().join("output");
    fs::create_dir_all(&src_dir)?;
    fs::create_dir_all(&output_dir)?;
    let input_path = src_dir.join("valid.jpg");
    image::RgbImage::from_pixel(16, 16, image::Rgb([12, 34, 56]))
        .save_with_format(&input_path, image::ImageFormat::Jpeg)?;

    let output = Command::new(env!("CARGO_BIN_EXE_img"))
        .arg("run")
        .arg(&input_path)
        .arg("--output")
        .arg(&output_dir)
        .arg("--force")
        .env("MFB_TOOL_CJXL", &cjxl_path)
        .env("MFB_TOOL_FFMPEG", &ffmpeg_path)
        .env("MFB_INVOKER", "test-harness")
        .output()?;
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "conversion unexpectedly succeeded"
    );
    assert!(
        diagnostics.contains("cjxl") || diagnostics.contains("Error while decoding"),
        "unexpected error message: {diagnostics}"
    );
    assert!(
        input_path.exists(),
        "failed conversion must retain its source"
    );
    assert!(!output_dir.join("valid.JXL").exists());
    Ok(())
}

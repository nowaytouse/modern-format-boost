use std::env;
use std::fs;
use tempfile::tempdir;

// This integration test simulates a failing `cjxl` binary by putting a small wrapper
// script on PATH that prints a typical cjxl stderr message and exits non-zero.
// It then runs the lossless converter on a tiny truncated JPEG and asserts the
// converter fails (records a conversion error) rather than crashing or succeeding.

#[test]
fn cjxl_failure_marks_conversion_error() -> anyhow::Result<()> {
    let td = tempdir()?;
    let bin_dir = td.path().join("bin");
    fs::create_dir_all(&bin_dir)?;

    // Create a fake cjxl that prints the stderr message and exits 1
    #[cfg(unix)]
    let _cjxl_path = {
        let p = bin_dir.join("cjxl");
        fs::write(
            &p,
            "#!/bin/sh\necho 'JPEG XL encoder v0.11.2' >&2\necho 'Encoding [JPEG, lossless transcode, effort: 11]' >&2\necho 'Error while decoding the JPEG image. It may be corrupt (e.g. truncated) or of an unsupported type (e.g. CMYK).' >&2\nexit 1\n",
        )?;
        // make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&p)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms)?;
        }
        p
    };

    #[cfg(windows)]
    let _cjxl_path = {
        let p = bin_dir.join("cjxl.bat");
        fs::write(&p, "@echo JPEG XL encoder v0.11.2 && exit /b 1")?;
        p
    };

    // Prepend our fake bin to PATH
    let old_path = env::var_os("PATH").unwrap_or_default();
    let mut new_path = bin_dir.into_os_string();
    new_path.push(":");
    new_path.push(&old_path);
    // tests in this repo sometimes set env in unsafe blocks for CI consistency
    unsafe { std::env::set_var("PATH", new_path) };

    // Prepare a truncated JPEG input
    let src_dir = td.path().join("src");
    fs::create_dir_all(&src_dir)?;
    let input_path = src_dir.join("truncated.jpg");
    fs::write(&input_path, vec![0xFFu8, 0xD8, 0xFF, 0xE0])?;

    // Run convert_jpeg_to_jxl which should attempt cjxl and fail
    use img::lossless_converter::convert_jpeg_to_jxl;
    let options = img::ConvertOptions::default();

    let res = convert_jpeg_to_jxl(&input_path, &options, None);

    // Expect an error and that the message references cjxl / EncodeImageJXL
    let Err(e) = res else {
        anyhow::bail!("conversion unexpectedly succeeded");
    };
    let e = e.to_string();
    assert!(
        e.contains("EncodeImageJXL") || e.contains("cjxl") || e.contains("Error while decoding"),
        "unexpected error message: {e}"
    );
    Ok(())
}

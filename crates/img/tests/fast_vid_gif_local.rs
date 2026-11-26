//! The read-only GIF route must never open a Photos task, even for a mixed input.
use std::{fs, path::Path, process::Command};

#[test]
fn local_gif_selection_reports_exact_dispositions_without_writing_state() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let state = root.join("unavailable-state");
    fs::write(&state, b"closed")?;
    let source = root.join("selection");
    fs::create_dir(&source)?;
    let gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x44\x01\x00;";
    let frame = b",\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02\x4c\x01\x00";
    let padded = [&gif[..gif.len() - 1], frame.as_slice(), b";\0\r\n"].concat();
    let inputs: [(&str, &[u8]); 5] = [
        ("selected", &padded),
        ("static.gif", gif),
        ("selected.xmp", b"sidecar"),
        ("pretend.gif", b"unrelated file"),
        ("damaged.gif", &gif[..gif.len() - 1]),
    ];
    for (name, bytes) in inputs {
        fs::write(source.join(name), bytes)?;
    }
    let run = |input: &Path, flags: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_img"));
        command
            .env_clear()
            .current_dir(root)
            .env("HOME", root)
            .env("MFB_INVOKER", "test-harness")
            .env("MFB_RUN_ID", "synthetic-gif")
            .env("MFB_INVOCATION_ID", "local-verification")
            .env("MFB_HOME_ROOT", &state);
        for key in ["SystemRoot", "DYLD_LIBRARY_PATH", "LD_LIBRARY_PATH"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .args(["fast-img", "--strategy", "gif", "--no-config"])
            .args(flags)
            .arg(input)
            .output()
    };
    for flags in [&[][..], &["--dry-run"][..]] {
        let output = run(&source, flags)?;
        assert_eq!(output.status.code(), Some(1));
        let text = String::from_utf8(output.stdout)?;
        assert!(
            text.contains("Succeeded: 0\nSkipped: 1\nFailed: 1\nIgnored: 3\nUnprocessed: 0"),
            "{text}"
        );
        assert!(text.contains("[FAIL    ]   damaged.gif:"));
        let static_entry = text
            .lines()
            .filter_map(|line| line.strip_prefix("MFB_FAST_IMG_IGNORED="))
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|entry| entry["source_path"] == "static.gif")
            .ok_or_else(|| anyhow::anyhow!("missing ignored receipt for static GIF"))?;
        assert_eq!(static_entry["reason_code"], "static_gif");
        #[cfg(unix)]
        {
            let failures: Vec<_> = text
                .lines()
                .filter_map(|line| line.strip_prefix("MFB_FILE_FAILURE="))
                .collect();
            assert_eq!(failures.len(), 1);
            let failure: serde_json::Value = serde_json::from_str(failures[0])?;
            assert_eq!(failure["invocation_id"], "local-verification");
            assert_eq!(failure["failure_phase"], "admission");
            assert!(
                failure["source_path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("/damaged.gif"))
            );
        }
        assert_eq!(text.contains("[DRY RUN ]"), !flags.is_empty());
        assert!(!text.contains("MFB_FAST_IMG_RESULT="));
    }
    let output = run(&source.join("selected"), &[])?;
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8(output.stdout)?.contains("Skipped: 1\nFailed: 0\nIgnored: 0"));
    for (name, bytes) in inputs {
        assert_eq!(fs::read(source.join(name))?, bytes);
    }
    assert_eq!(fs::read_dir(&source)?.count(), inputs.len());
    assert_eq!(fs::read_dir(root)?.count(), 2);
    assert_eq!(fs::read(state)?, b"closed");
    Ok(())
}

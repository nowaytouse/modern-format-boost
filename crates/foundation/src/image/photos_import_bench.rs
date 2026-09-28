//! Explicitly opted-in, synthetic-only benchmark for the debug system library.
use super::*;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

#[test]
#[ignore = "writes synthetic assets to an explicitly selected debug system Photos library"]
fn photos_native_debug_benchmark() -> anyhow::Result<()> {
    let library =
        PathBuf::from(std::env::var("MFB_LIVE_PHOTOS_SMOKE_DEBUG_LIBRARY")?).canonicalize()?;
    anyhow::ensure!(library.file_name() == Some(std::ffi::OsStr::new("debug.photoslibrary")));
    let helper = PathBuf::from(std::env::var("MFB_PHOTOS_NATIVE_HELPER_APP")?).canonicalize()?;
    let witness = std::env::var("MFB_PHOTOS_NATIVE_WITNESS")?;
    anyhow::ensure!(!witness.is_empty());
    let count: usize = std::env::var("MFB_PHOTOS_BENCH_COUNT")?.parse()?;
    let batch_size: usize = std::env::var("MFB_PHOTOS_BENCH_BATCH_SIZE")?.parse()?;
    anyhow::ensure!((1..=100_000).contains(&count));
    anyhow::ensure!([1, 2, 50, 100, 250, 500, 1000].contains(&batch_size));
    let _lock = acquire_photos_import_lock()?;
    require_active_photos_library(Some(&library))?;
    // Keep inputs and journals on every failure, including an unknown commit.
    let scratch = tempfile::Builder::new()
        .prefix("mfb-native-bench-")
        .tempdir()?
        .keep();
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
    eprintln!("Native benchmark evidence: {}", scratch.display());
    let jpeg = scratch.join("original.jpg");
    image::RgbImage::from_fn(96, 64, |x, y| {
        image::Rgb([
            u8::try_from(x * 2).unwrap(),
            u8::try_from(y * 3).unwrap(),
            u8::try_from(x + y).unwrap(),
        ])
    })
    .save_with_format(&jpeg, image::ImageFormat::Jpeg)?;
    let jxl = scratch.join("original.jxl");
    let encoded = run_fast_img_command_with_timeout(
        std::process::Command::new(
            crate::common_utils::resolve_tool_path("cjxl")
                .ok_or_else(|| anyhow::anyhow!("cjxl required"))?,
        )
        .arg(&jpeg)
        .arg(&jxl)
        .args(["--distance=0", "--effort=1"]),
        Duration::from_secs(60),
        "native benchmark JXL fixture",
    )?;
    anyhow::ensure!(encoded.status.success(), "JXL fixture encoding failed");
    verify_jxl_roundtrip_integrity(&jpeg, &jxl)?;
    let hash = crate::common_utils::calculate_blake3_hash(&jxl)?;
    let name = scratch.file_name().unwrap().to_str().unwrap();
    let requests_path = scratch.join("requests.jsonl");
    let results_path = scratch.join("results.jsonl");
    let mut requests = std::fs::File::create_new(&requests_path)?;
    writeln!(
        requests,
        "{}",
        serde_json::json!({"version":1,"operation":"probe","batchID":"probe","witnessIdentifiers":[witness]})
    )?;
    for start in (0..count).step_by(batch_size) {
        let assets = (start..count.min(start + batch_size)).map(|index| {
            // Identical bytes are intentional: distinct input entries must remain distinct assets.
            serde_json::json!({"entryID":index.to_string(),"resources":[{
                "path":jxl,"kind":"photo","originalFilename":format!("{name}-{index}.jxl"),"blake3":hash
            }]})
        }).collect::<Vec<_>>();
        writeln!(
            requests,
            "{}",
            serde_json::json!({"version":1,"operation":"import",
            "batchID":start.to_string(),"witnessIdentifiers":[witness],"assets":assets,
            "journalPath":scratch.join(format!("batch-{start}.json"))})
        )?;
    }
    requests.sync_all()?;
    let started = std::time::Instant::now();
    let state = crate::process_lock::get_mfb_root()?.join("photos-native");
    let mut client = super::super::photos_native::Client::start(&helper, &state, &[witness])?;
    let mut results = std::fs::File::create_new(&results_path)?;
    writeln!(
        results,
        "{}",
        serde_json::json!({"version":1,"batchID":"probe","state":"ready"})
    )?;
    for line in std::fs::read_to_string(&requests_path)?.lines().skip(1) {
        let request = serde_json::from_str(line)?;
        require_active_photos_library(Some(&library))?;
        let reply = client.request(&request)?;
        super::super::photos_native::committed_pairs(&request, &reply)?;
        writeln!(results, "{reply}")?;
        results.sync_all()?;
    }
    drop(client);
    require_active_photos_library(Some(&library))?;
    let import_seconds = started.elapsed().as_secs_f64();
    let records = std::fs::read_to_string(&results_path)?
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    anyhow::ensure!(
        records.len() == count.div_ceil(batch_size) + 1,
        "worker replies incomplete; reconcile journals, never blindly replay"
    );
    anyhow::ensure!(
        records[0]["state"] == "ready",
        "PhotoKit target/authorization probe failed: {}",
        records[0]
    );
    let mut entries = BTreeSet::new();
    let mut uuids = BTreeSet::new();
    let mut transaction_seconds = Vec::new();
    for record in &records[1..] {
        anyhow::ensure!(record["state"] == "committed", "uncertain batch: {record}");
        transaction_seconds.push(
            record["transactionSeconds"]
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("missing timing"))?,
        );
        for identity in record["identities"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing identities"))?
        {
            anyhow::ensure!(
                entries.insert(
                    identity["entryID"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing entry ID"))?
                        .to_owned()
                )
            );
            let id = identity["localIdentifier"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("missing local ID"))?;
            anyhow::ensure!(
                uuids.insert(osxphotos_uuid_from_photos_import_identifier(id)?.to_owned()),
                "native importer merged duplicate content entries"
            );
        }
    }
    anyhow::ensure!(entries == (0..count).map(|i| i.to_string()).collect());
    anyhow::ensure!(uuids.len() == count);
    let verify_started = std::time::Instant::now();
    let mut verified = BTreeSet::new();
    let uuids = uuids.into_iter().collect::<Vec<_>>();
    for chunk in uuids.chunks(250) {
        let mut last_error = None;
        for attempt in 0..10 {
            match query_osxphotos_asset_probes_from_library(chunk, &library) {
                Ok(probes)
                    if probes.len() == chunk.len()
                        && probes.iter().all(|p| !p.ismissing && p.path.is_file()) =>
                {
                    for probe in probes {
                        anyhow::ensure!(chunk.contains(&probe.uuid), "unexpected original UUID");
                        anyhow::ensure!(
                            crate::common_utils::calculate_blake3_hash(&probe.path)? == hash,
                            "Photos original payload mismatch"
                        );
                        anyhow::ensure!(verified.insert(probe.uuid), "duplicate UUID proof");
                    }
                    last_error = None;
                    break;
                }
                Ok(_) => last_error = Some("original resources not yet visible".to_owned()),
                Err(error) => last_error = Some(error.to_string()),
            }
            if attempt < 9 {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
        anyhow::ensure!(
            last_error.is_none(),
            "original verification failed: {last_error:?}"
        );
    }
    anyhow::ensure!(verified.len() == count);
    require_active_photos_library(Some(&library))?;
    let report = serde_json::json!({"schema_version":1,"backend":"photokit","synthetic":true,
        "assets":count,"batch_size":batch_size,"transactions":transaction_seconds.len(),
        "transaction_seconds":transaction_seconds,"import_seconds":import_seconds,
        "verification_seconds":verify_started.elapsed().as_secs_f64(),
        "total_seconds":started.elapsed().as_secs_f64(),"original_byte_hashes_verified":verified.len(),
        "duplicate_content_entries_preserved":true,"icloud_tested":false});
    let mut file = std::fs::File::create_new(scratch.join("report.json"))?;
    serde_json::to_writer_pretty(&mut file, &report)?;
    file.sync_all()?;
    eprintln!("Native Photos benchmark: {report}");
    Ok(())
}

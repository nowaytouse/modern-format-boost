//! Explicitly opted-in, synthetic-only benchmark for the debug system library.
use super::*;
use std::io::{BufRead, BufReader, Write};
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
    let scratch = crate::media_conversion_gate::delivery_temp_dir_in_scratch_or_err(
        "native Photos benchmark evidence",
        "mfb-native-bench-",
    )?
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
    for line in BufReader::new(std::fs::File::open(&requests_path)?)
        .lines()
        .skip(1)
    {
        let request = serde_json::from_str(&line?)?;
        require_active_photos_library(Some(&library))?;
        let reply = client.request(&request)?;
        super::super::photos_native::committed_pairs(&request, &reply)?;
        writeln!(results, "{reply}")?;
        results.sync_all()?;
    }
    drop(client);
    require_active_photos_library(Some(&library))?;
    let import_seconds = started.elapsed().as_secs_f64();
    let mut records = BufReader::new(std::fs::File::open(&results_path)?).lines();
    let ready: serde_json::Value = serde_json::from_str(
        &records
            .next()
            .ok_or_else(|| anyhow::anyhow!("missing PhotoKit target probe"))??,
    )?;
    anyhow::ensure!(
        ready["state"] == "ready",
        "PhotoKit target/authorization probe failed: {ready}"
    );
    let mut entries = BTreeSet::new();
    let mut uuids = BTreeSet::new();
    let mut transaction_seconds = Vec::new();
    let mut transaction_count = 0;
    for record in records {
        let record: serde_json::Value = serde_json::from_str(&record?)?;
        transaction_count += 1;
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
    anyhow::ensure!(
        transaction_count == count.div_ceil(batch_size),
        "worker replies incomplete; reconcile journals, never blindly replay"
    );
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

#[test]
#[ignore = "imports synthetic assets through the real pipeline into debug.photoslibrary"]
fn photos_pipeline_debug_benchmark() -> anyhow::Result<()> {
    let library =
        PathBuf::from(std::env::var("MFB_LIVE_PHOTOS_SMOKE_DEBUG_LIBRARY")?).canonicalize()?;
    anyhow::ensure!(
        library.file_name() == Some(std::ffi::OsStr::new("debug.photoslibrary")),
        "pipeline benchmark is restricted to debug.photoslibrary"
    );
    let count: usize = std::env::var("MFB_PHOTOS_BENCH_COUNT")?.parse()?;
    anyhow::ensure!((1..=100_000).contains(&count));
    let backend = std::env::var("MFB_PHOTOS_IMPORT_BACKEND")?;
    anyhow::ensure!(
        ["photokit", "applescript"].contains(&backend.as_str()),
        "set MFB_PHOTOS_IMPORT_BACKEND to photokit or applescript"
    );
    let config = crate::runtime_config::load(None, true)?.config;
    let batch_size = if backend == "photokit" {
        config.photos.native_batch_size
    } else {
        FAST_IMG_PHOTOS_IMPORT_TRANSACTION_SIZE
    };
    let verification_batch_size = config.photos.verification_batch_size;
    crate::runtime_config::install(config)?;

    let _lock = acquire_photos_import_lock()?;
    require_active_photos_library(Some(&library))?;
    let count_assets = || -> anyhow::Result<usize> {
        require_active_photos_library(Some(&library))?;
        let output = run_fast_img_command_with_timeout(
            std::process::Command::new(resolve_osascript_command()).args([
                "-e",
                "tell application \"Photos\" to return count of media items",
            ]),
            Duration::from_secs(30),
            "count debug Photos assets",
        )?;
        anyhow::ensure!(output.status.success(), "Photos asset count failed");
        Ok(String::from_utf8(output.stdout)?.trim().parse()?)
    };
    let before = (count <= 1000).then(&count_assets).transpose()?;

    // Keep synthetic inputs and the report after any failed or uncertain import.
    let scratch = crate::media_conversion_gate::delivery_temp_dir_in_scratch_or_err(
        "Photos pipeline benchmark evidence",
        "mfb-pipeline-bench-",
    )?
    .keep();
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
    eprintln!("Photos pipeline benchmark evidence: {}", scratch.display());
    let source_root = scratch.join("source");
    let working_copy = scratch.join("optimized");
    std::fs::create_dir(&source_root)?;
    std::fs::create_dir(&working_copy)?;
    let jpeg = scratch.join("original.jpg");
    let jxl = scratch.join("original.jxl");
    image::RgbImage::from_fn(96, 64, |x, y| {
        image::Rgb([
            u8::try_from(x * 2).unwrap(),
            u8::try_from(y * 3).unwrap(),
            u8::try_from(x + y).unwrap(),
        ])
    })
    .save_with_format(&jpeg, image::ImageFormat::Jpeg)?;
    let encoded = run_fast_img_command_with_timeout(
        std::process::Command::new(
            crate::common_utils::resolve_tool_path("cjxl")
                .ok_or_else(|| anyhow::anyhow!("cjxl required"))?,
        )
        .arg(&jpeg)
        .arg(&jxl)
        .args(["--distance=0", "--effort=1"]),
        Duration::from_secs(60),
        "pipeline benchmark JXL fixture",
    )?;
    anyhow::ensure!(encoded.status.success(), "JXL fixture encoding failed");
    verify_jxl_roundtrip_integrity(&jpeg, &jxl)?;
    let xmp = scratch.join("metadata.xmp");
    std::fs::write(&xmp, br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" dc:description="Synthetic Photos pipeline benchmark"/></rdf:RDF></x:xmpmeta>"#)?;
    crate::metadata::append_xmp_overlay_to_jxl(&xmp, &jxl)?;
    let source_hash = crate::common_utils::calculate_blake3_hash(&jpeg)?;
    let output_hash = crate::common_utils::calculate_blake3_hash(&jxl)?;

    let mut marker = WorkingCopyMarker::new(source_root.clone(), working_copy.clone(), count);
    let run_name = scratch.file_name().unwrap().to_str().unwrap();
    for index in 0..count {
        let stem = format!("{run_name}-{index:06}");
        let source_rel = format!("{stem}.jpg");
        let output_rel = format!("{stem}.jxl");
        std::fs::hard_link(&jpeg, source_root.join(&source_rel))?;
        std::fs::hard_link(&jxl, working_copy.join(&output_rel))?;
        marker.blake3_log.insert(
            source_rel,
            Blake3Entry {
                out_rel: Some(output_rel),
                src: source_hash.clone(),
                out: output_hash.clone(),
                library_asset: None,
            },
        );
    }
    let output_paths = fast_img_marker_output_paths(&marker)?;
    validate_fast_img_marker_output_hashes(&marker)?;
    let started = std::time::Instant::now();
    let import = import_marker_outputs_with_photos_checkpoint_in_library(
        &marker,
        &output_paths,
        false,
        |uuids| query_osxphotos_asset_probes_from_library(uuids, &library),
        path_has_quarantine_xattr,
        Some(&library),
    )?;
    let import_seconds = started.elapsed().as_secs_f64();
    require_active_photos_library(Some(&library))?;
    anyhow::ensure!(import.import_error_count == 0 && import.imported_assets.len() == count);
    anyhow::ensure!(import.photos_library_path.as_deref() == Some(library.as_path()));
    let imported_uuids = import
        .imported_assets
        .iter()
        .map(|asset| {
            asset
                .photos_uuid
                .clone()
                .ok_or_else(|| anyhow::anyhow!("imported asset lacks Photos UUID"))
        })
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    anyhow::ensure!(imported_uuids.len() == count, "duplicate Photos UUID");
    for (source_rel, entry) in &marker.blake3_log {
        anyhow::ensure!(source_root.join(source_rel).is_file());
        anyhow::ensure!(
            working_copy
                .join(entry.out_rel.as_deref().expect("benchmark output path"))
                .is_file()
        );
    }
    let after_import = before.map(|_| count_assets()).transpose()?;
    if let (Some(before), Some(after)) = (before, after_import) {
        anyhow::ensure!(
            after == before + count,
            "Photos asset count did not increase by {count}"
        );
    }

    // Resume from the original marker, before it held any imported UUID proof.
    let resume_started = std::time::Instant::now();
    let resumed = import_marker_outputs_with_photos_checkpoint_in_library(
        &marker,
        &output_paths,
        true,
        |uuids| query_osxphotos_asset_probes_from_library(uuids, &library),
        path_has_quarantine_xattr,
        Some(&library),
    )?;
    let resume_seconds = resume_started.elapsed().as_secs_f64();
    require_active_photos_library(Some(&library))?;
    anyhow::ensure!(resumed.import_error_count == 0 && resumed.imported_assets.len() == count);
    let resumed_uuids = resumed
        .imported_assets
        .iter()
        .map(|asset| {
            asset
                .photos_uuid
                .clone()
                .ok_or_else(|| anyhow::anyhow!("resumed asset lacks Photos UUID"))
        })
        .collect::<anyhow::Result<BTreeSet<_>>>()?;
    anyhow::ensure!(
        resumed_uuids == imported_uuids,
        "resume changed Photos UUID custody"
    );
    let after_resume = after_import.map(|_| count_assets()).transpose()?;
    anyhow::ensure!(after_resume == after_import, "resume added Photos assets");

    let report = serde_json::json!({
        "schema_version": 1,
        "backend": backend,
        "synthetic": true,
        "assets": count,
        "batch_size": batch_size,
        "verification_batch_size": verification_batch_size,
        "import_seconds": import_seconds,
        "resume_seconds": resume_seconds,
        "total_seconds": started.elapsed().as_secs_f64(),
        "photos_count_before": before,
        "photos_count_after_import": after_import,
        "photos_count_after_resume": after_resume,
        "unique_uuids": imported_uuids.len(),
        "sources_and_outputs_retained": true,
        "marker_path": crate::pipeline::verification::marker_path_for_working_copy(&working_copy),
    });
    let mut file = std::fs::File::create_new(scratch.join("report.json"))?;
    serde_json::to_writer_pretty(&mut file, &report)?;
    file.sync_all()?;
    eprintln!("Photos pipeline benchmark: {report}");
    Ok(())
}

        let _ = std::fs::remove_file(&tmp_file);
    }

    info!(
        "✅ UltraHDR JPEG HDR synthesis completed: {}",
        output.display()
    );
    Ok(())
}

/// Migration Path B: Encode `UltraHDR` JPEG to JXL with `GainMap` as sidecar.
///
/// This does NOT synthesize a single HDR plane. Instead, it:
/// 1. Extracts the SDR base image.
/// 2. Extracts the `GainMap` sub-image.
/// 3. Losslessly recompresses the SDR base to JXL.
/// 4. Saves the `GainMap` as a sidecar `.gainmap.png`.
/// 5. Preserves Ultra HDR XMP metadata (`hdrgm`) via `ExiftoolBuilder`.
///
/// This preserves the original SDR appearance bit-perfectly while
/// keeping the gainmap for future HDR reconstruction.

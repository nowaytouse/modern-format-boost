//! Per-import measurements, not estimates derived from asset counts.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Instant;

thread_local! {
    static CURRENT: RefCell<Option<Data>> = const { RefCell::new(None) };
}

#[derive(Default, serde::Serialize)]
struct Phase {
    calls: usize,
    seconds: f64,
}

#[derive(Default, serde::Serialize)]
struct Data {
    backend: &'static str,
    phases: BTreeMap<&'static str, Phase>,
    committed_assets: usize,
    verified_assets: usize,
    peak_verification_backlog: usize,
    import_batch_size: Option<usize>,
    verification_batch_size: Option<usize>,
    helper_peak_rss_bytes: Option<u64>,
    helper_user_cpu_seconds: Option<f64>,
    helper_system_cpu_seconds: Option<f64>,
    #[serde(skip)]
    transaction_seconds: Vec<f64>,
}

pub(super) struct Profile {
    started: Instant,
    previous: Option<Data>,
}

impl Profile {
    pub(super) fn start(backend: &'static str) -> Self {
        Self {
            started: Instant::now(),
            previous: CURRENT.with(|current| {
                current.replace(Some(Data {
                    backend,
                    ..Data::default()
                }))
            }),
        }
    }

    // Rates are approximate diagnostics; exact custody counts stay integers.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn report(&self, succeeded: bool) {
        CURRENT.with_borrow(|current| {
            if let Some(data) = current {
                let total_seconds = self.started.elapsed().as_secs_f64();
                let mut samples = data.transaction_seconds.clone();
                samples.sort_by(f64::total_cmp);
                let verified_assets = data.verified_assets;
                let report = serde_json::json!({
                    "schema_version": 1,
                    "backend": data.backend,
                    "scope": "pending_import_and_verification",
                    "succeeded": succeeded,
                    "verified_assets": verified_assets,
                    "committed_assets": if data.backend == "native" { Some(data.committed_assets) } else { None },
                    "peak_verification_backlog": if data.backend == "native" { Some(data.peak_verification_backlog) } else { None },
                    "import_batch_size": data.import_batch_size,
                    "verification_batch_size": data.verification_batch_size,
                    "helper_peak_rss_bytes": data.helper_peak_rss_bytes,
                    "helper_user_cpu_seconds": data.helper_user_cpu_seconds,
                    "helper_system_cpu_seconds": data.helper_system_cpu_seconds,
                    "total_seconds": total_seconds,
                    "verified_assets_per_second": verified_assets as f64 / total_seconds.max(f64::EPSILON),
                    "phases": data.phases,
                    "transaction_samples": samples.len(),
                    "latency_sample_scope": if data.backend == "native" { "swift_transaction" } else { "applescript_session" },
                    "transaction_latest_seconds": data.transaction_seconds.last().copied(),
                    "transaction_p50_seconds": percentile(&samples, 50),
                    "transaction_p90_seconds": percentile(&samples, 90),
                    "transaction_p95_seconds": percentile(&samples, 95),
                    "transaction_p99_seconds": percentile(&samples, 99),
                    "transaction_mean_seconds": (!samples.is_empty()).then(|| samples.iter().sum::<f64>() / samples.len() as f64),
                    "note": "phase durations can nest; total excludes encoding and source cleanup"
                });
                eprintln!("[PHOTOS PROFILE] {report}");
            }
        });
    }
}

pub(super) fn batch_sizes(import: usize, verify: usize) {
    CURRENT.with_borrow_mut(|current| {
        if let Some(data) = current {
            data.import_batch_size = Some(import);
            data.verification_batch_size = Some(verify);
        }
    });
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn swift_transaction(reply: &serde_json::Value) -> anyhow::Result<()> {
    let seconds = reply["transactionSeconds"]
        .as_f64()
        .ok_or_else(|| anyhow::anyhow!("missing PhotoKit transaction timing"))?;
    anyhow::ensure!(
        seconds.is_finite() && seconds >= 0.0,
        "invalid PhotoKit transaction timing"
    );
    CURRENT.with_borrow_mut(|current| {
        if let Some(data) = current {
            let phase = data.phases.entry("swift_transaction").or_default();
            phase.calls += 1;
            phase.seconds += seconds;
            if data.transaction_seconds.len() < 100_000 {
                data.transaction_seconds.push(seconds);
            }
            if let Some(rss) = reply["peakRSSBytes"].as_u64() {
                data.helper_peak_rss_bytes =
                    Some(data.helper_peak_rss_bytes.map_or(rss, |old| old.max(rss)));
            }
            for (key, field) in [
                ("userCPUSeconds", &mut data.helper_user_cpu_seconds),
                ("systemCPUSeconds", &mut data.helper_system_cpu_seconds),
            ] {
                if let Some(value) = reply[key]
                    .as_f64()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                {
                    *field = Some(field.map_or(value, |old| old.max(value)));
                }
            }
        }
    });
    Ok(())
}

impl Drop for Profile {
    fn drop(&mut self) {
        CURRENT.with(|current| current.replace(self.previous.take()));
    }
}

fn percentile(sorted: &[f64], percent: usize) -> Option<f64> {
    sorted
        .get((sorted.len() * percent).div_ceil(100).saturating_sub(1))
        .copied()
}

#[cfg(any(target_os = "macos", test))]
pub(super) fn committed(count: usize, backlog: usize) {
    CURRENT.with_borrow_mut(|current| {
        if let Some(data) = current {
            data.committed_assets += count;
            data.peak_verification_backlog = data.peak_verification_backlog.max(backlog);
        }
    });
}

pub(super) fn verified(count: usize) {
    CURRENT.with_borrow_mut(|current| {
        if let Some(data) = current {
            data.verified_assets += count;
            eprintln!(
                "[PHOTOS PROGRESS] backend={} committed={} verified={} peak_backlog={}",
                data.backend,
                data.committed_assets,
                data.verified_assets,
                data.peak_verification_backlog
            );
        }
    });
}

pub(super) struct Timer {
    phase: &'static str,
    started: Instant,
}

pub(super) fn timer(phase: &'static str) -> Timer {
    Timer {
        phase,
        started: Instant::now(),
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let seconds = self.started.elapsed().as_secs_f64();
        CURRENT.with_borrow_mut(|current| {
            if let Some(data) = current {
                let phase = data.phases.entry(self.phase).or_default();
                phase.calls += 1;
                phase.seconds += seconds;
                // Bound diagnostics memory even for unusually long compatibility runs.
                if data.backend != "native"
                    && self.phase == "import_session"
                    && data.transaction_seconds.len() < 100_000
                {
                    data.transaction_seconds.push(seconds);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measurements_are_scoped_and_quantiles_do_not_invent_samples() {
        assert_eq!(percentile(&[], 99), None);
        assert_eq!(percentile(&[1., 2., 3., 4.], 50), Some(2.));
        assert_eq!(percentile(&[1., 2., 3., 4.], 99), Some(4.));
        let profile = Profile::start("applescript");
        drop(timer("import_session"));
        CURRENT.with_borrow(|data| {
            let data = data.as_ref().unwrap();
            assert_eq!(data.phases["import_session"].calls, 1);
            assert_eq!(data.transaction_seconds.len(), 1);
        });
        drop(profile);
        CURRENT.with_borrow(|data| assert!(data.is_none()));
        let profile = Profile::start("native");
        drop(timer("import_session"));
        drop(timer("native_request_wall"));
        swift_transaction(
            &serde_json::json!({"transactionSeconds":0.25,"peakRSSBytes":1024,
            "userCPUSeconds":1.25,"systemCPUSeconds":0.5}),
        )
        .unwrap();
        committed(100, 100);
        verified(50);
        CURRENT.with_borrow(|data| {
            let data = data.as_ref().unwrap();
            assert_eq!(data.backend, "native");
            assert_eq!(data.transaction_seconds.len(), 1);
            assert_eq!(data.transaction_seconds[0], 0.25);
            assert_eq!(data.helper_peak_rss_bytes, Some(1024));
            assert_eq!(data.helper_user_cpu_seconds, Some(1.25));
            assert_eq!(data.committed_assets, 100);
            assert_eq!(data.verified_assets, 50);
            assert_eq!(data.peak_verification_backlog, 100);
        });
        drop(profile);
    }
}

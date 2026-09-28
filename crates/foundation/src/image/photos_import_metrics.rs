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
    phases: BTreeMap<&'static str, Phase>,
    #[serde(skip)]
    transaction_seconds: Vec<f64>,
}

pub(super) struct Profile {
    started: Instant,
    previous: Option<Data>,
}

impl Profile {
    pub(super) fn start() -> Self {
        Self {
            started: Instant::now(),
            previous: CURRENT.with(|current| current.replace(Some(Data::default()))),
        }
    }

    // Rates are approximate diagnostics; exact custody counts stay integers.
    #[allow(clippy::cast_precision_loss)]
    pub(super) fn report(&self, verified_assets: usize, succeeded: bool) {
        CURRENT.with_borrow(|current| {
            if let Some(data) = current {
                let total_seconds = self.started.elapsed().as_secs_f64();
                let mut samples = data.transaction_seconds.clone();
                samples.sort_by(f64::total_cmp);
                let report = serde_json::json!({
                    "schema_version": 1,
                    "backend": "applescript",
                    "scope": "pending_import_and_verification",
                    "succeeded": succeeded,
                    "verified_assets": verified_assets,
                    "total_seconds": total_seconds,
                    "verified_assets_per_second": verified_assets as f64 / total_seconds.max(f64::EPSILON),
                    "phases": data.phases,
                    "transaction_samples": samples.len(),
                    "transaction_p50_seconds": percentile(&samples, 50),
                    "transaction_p90_seconds": percentile(&samples, 90),
                    "transaction_p99_seconds": percentile(&samples, 99),
                    "transaction_mean_seconds": (!samples.is_empty()).then(|| samples.iter().sum::<f64>() / samples.len() as f64),
                    "note": "phase durations can nest; total excludes encoding and source cleanup"
                });
                eprintln!("[PHOTOS PROFILE] {report}");
            }
        });
    }
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
                if self.phase == "import_session" && data.transaction_seconds.len() < 100_000 {
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
        let profile = Profile::start();
        drop(timer("import_session"));
        CURRENT.with_borrow(|data| {
            let data = data.as_ref().unwrap();
            assert_eq!(data.phases["import_session"].calls, 1);
            assert_eq!(data.transaction_seconds.len(), 1);
        });
        drop(profile);
        CURRENT.with_borrow(|data| assert!(data.is_none()));
    }
}

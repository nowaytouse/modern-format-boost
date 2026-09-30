//! Bounded native import/verification windows; no media bytes are queued here.
use crate::runtime_config::PhotosPolicy;
use std::ops::Range;
use std::time::Duration;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    Import(Range<usize>),
    Verify(Range<usize>),
}

pub(super) struct Schedule {
    total: usize,
    submitted: usize,
    verified: usize,
    batch: usize,
    policy: PhotosPolicy,
    healthy_windows: usize,
    draining: bool,
}

impl Schedule {
    pub(super) fn new(total: usize, policy: &PhotosPolicy) -> anyhow::Result<Self> {
        crate::runtime_config::RuntimeConfig {
            photos: policy.clone(),
            ..Default::default()
        }
        .validate()?;
        Ok(Self {
            total,
            submitted: 0,
            verified: 0,
            batch: policy.native_batch_size,
            policy: policy.clone(),
            healthy_windows: 0,
            draining: false,
        })
    }

    // Called only after the previous step succeeded. On any error the caller
    // stops; durable journals, not these in-memory counters, drive recovery.
    pub(super) fn next(&mut self) -> Option<Step> {
        if self.verified == self.total {
            return None;
        }
        let backlog = self.submitted - self.verified;
        if self.draining
            || backlog >= self.policy.verification_batch_size
            || self.submitted == self.total
        {
            let end = self.verified + backlog.min(self.policy.verification_batch_size);
            let range = self.verified..end;
            self.verified = end;
            // Adaptive feedback needs a complete cycle even for coprime sizes.
            self.draining = self.policy.adaptive_batching && end < self.submitted;
            Some(Step::Verify(range))
        } else {
            let end = self.submitted + (self.total - self.submitted).min(self.batch);
            let range = self.submitted..end;
            self.submitted = end;
            Some(Step::Import(range))
        }
    }

    pub(super) fn observe(
        &mut self,
        elapsed: Duration,
        memory_pressure: bool,
    ) -> Option<(usize, usize, &'static str)> {
        if !self.policy.adaptive_batching {
            return None;
        }
        let old = self.batch;
        let target = Duration::from_secs(self.policy.target_batch_seconds);
        let reason = if memory_pressure || elapsed > target {
            self.healthy_windows = 0;
            self.batch = (self.batch / 2).max(self.policy.native_min_batch_size);
            if memory_pressure {
                "memory_pressure"
            } else {
                "latency"
            }
        } else if elapsed < target / 2 {
            self.healthy_windows += 1;
            if self.healthy_windows >= 2 {
                self.batch = (self.batch * 2).min(self.policy.native_max_batch_size);
                self.healthy_windows = 0;
            }
            "verified_low_latency"
        } else {
            self.healthy_windows = 0;
            "stable"
        };
        (old != self.batch).then_some((old, self.batch, reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ten_thousand_assets_have_independent_bounded_windows() -> anyhow::Result<()> {
        for (import_size, verify_size) in [(50, 250), (500, 250), (250, 500), (1000, 10)] {
            let policy = PhotosPolicy {
                native_batch_size: import_size,
                verification_batch_size: verify_size,
                ..Default::default()
            };
            let mut schedule = Schedule::new(10_003, &policy)?;
            let (mut submitted, mut verified, mut calls, mut queries) = (0, 0, 0, 0);
            while let Some(step) = schedule.next() {
                match step {
                    Step::Import(range) => {
                        assert_eq!(range.start, submitted);
                        assert!(range.len() <= import_size);
                        submitted = range.end;
                        calls += 1;
                    }
                    Step::Verify(range) => {
                        assert_eq!(range.start, verified);
                        assert!(range.end <= submitted && range.len() <= verify_size);
                        verified = range.end;
                        queries += 1;
                    }
                }
                assert!(submitted - verified < import_size + verify_size);
            }
            assert_eq!(submitted, 10_003);
            assert_eq!(verified, submitted);
            assert_eq!(calls, 10_003_usize.div_ceil(import_size));
            assert_eq!(queries, 10_003_usize.div_ceil(verify_size));
        }
        Ok(())
    }

    #[test]
    fn adaptive_cycles_drain_coprime_windows_before_new_imports() -> anyhow::Result<()> {
        let policy = PhotosPolicy {
            adaptive_batching: true,
            verification_batch_size: 251,
            native_max_batch_size: 200,
            ..Default::default()
        };
        let mut schedule = Schedule::new(10_003, &policy)?;
        let (mut submitted, mut verified, mut cycles) = (0, 0, 0);
        let mut draining = false;
        while let Some(step) = schedule.next() {
            match step {
                Step::Import(range) => {
                    assert!(!draining);
                    assert_eq!(range.start, submitted);
                    assert!(range.len() <= policy.native_max_batch_size);
                    submitted = range.end;
                }
                Step::Verify(range) => {
                    assert_eq!(range.start, verified);
                    assert!(range.len() <= policy.verification_batch_size);
                    verified = range.end;
                    draining = verified < submitted;
                    if !draining {
                        cycles += 1;
                        schedule.observe(Duration::ZERO, false);
                    }
                }
            }
            assert!(
                submitted - verified
                    < policy.native_max_batch_size + policy.verification_batch_size
            );
        }
        assert_eq!(verified, 10_003);
        assert!(cycles > 5);
        assert_eq!(schedule.batch, 200);
        Ok(())
    }

    #[test]
    fn adaptive_batches_are_opt_in_bounded_and_pressure_sensitive() -> anyhow::Result<()> {
        let mut policy = PhotosPolicy::default();
        let mut fixed = Schedule::new(1000, &policy)?;
        assert_eq!(fixed.observe(Duration::ZERO, false), None);
        assert_eq!(fixed.observe(Duration::from_secs(100), true), None);
        policy.adaptive_batching = true;
        policy.native_max_batch_size = 200;
        let mut adaptive = Schedule::new(1000, &policy)?;
        assert_eq!(adaptive.observe(Duration::from_secs(1), false), None);
        assert_eq!(
            adaptive.observe(Duration::from_secs(1), false),
            Some((100, 200, "verified_low_latency"))
        );
        assert_eq!(
            adaptive.observe(Duration::from_secs(11), false),
            Some((200, 100, "latency"))
        );
        assert_eq!(
            adaptive.observe(Duration::ZERO, true),
            Some((100, 50, "memory_pressure"))
        );
        assert_eq!(adaptive.observe(Duration::ZERO, true), None);
        policy.verification_batch_size = 0;
        assert!(Schedule::new(1000, &policy).is_err());
        Ok(())
    }
}

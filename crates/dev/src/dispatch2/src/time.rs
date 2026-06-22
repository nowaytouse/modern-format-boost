use core::ops::{Add, Sub};
use core::time::Duration;

use crate::numeric_cast;

#[cfg(all(feature = "libc", feature = "std"))]
use std::time::SystemTime;

#[cfg(feature = "objc2")]
use objc2::encode::{Encode, Encoding, RefEncode};

/// An abstract representation of time on the uptime clock.
///
/// Zero means "now" and [`FOREVER`](Self::FOREVER) means "infinity"; other values
/// are opaque encodings produced by [`dispatch_time`](Self::time).
#[doc(alias = "dispatch_time_t")]
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, PartialOrd, Eq, Ord)]
pub struct DispatchTime(pub u64);

/// Wall-clock time (based on `gettimeofday(3)` / `dispatch_walltime`).
#[doc(alias = "dispatch_time_t")]
#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Ord, PartialOrd)]
pub struct DispatchWallTime(pub u64);

/// A time interval added to a [`DispatchTime`] or [`DispatchWallTime`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum DispatchTimeInterval {
    /// Whole seconds.
    Seconds(i64),
    /// Milliseconds.
    Milliseconds(i64),
    /// Microseconds.
    Microseconds(i64),
    /// Nanoseconds.
    Nanoseconds(i64),
    /// No offset (maps to `Int64::MAX` nanoseconds internally).
    Never,
}

#[cfg(feature = "objc2")]
// SAFETY: `DispatchTime` is `#[repr(transparent)]`.
unsafe impl Encode for DispatchTime {
    const ENCODING: Encoding = u64::ENCODING;
}

#[cfg(feature = "objc2")]
// SAFETY: Same as above.
unsafe impl RefEncode for DispatchTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

#[cfg(feature = "objc2")]
// SAFETY: `DispatchWallTime` is `#[repr(transparent)]`.
unsafe impl Encode for DispatchWallTime {
    const ENCODING: Encoding = u64::ENCODING;
}

#[cfg(feature = "objc2")]
// SAFETY: Same as above.
unsafe impl RefEncode for DispatchWallTime {
    const ENCODING_REF: Encoding = Encoding::Pointer(&<Self as Encode>::ENCODING);
}

impl DispatchTimeInterval {
    const NSEC_PER_SEC: i64 = 1_000_000_000;
    const NSEC_PER_MSEC: i64 = 1_000_000;
    const NSEC_PER_USEC: i64 = 1_000;

    /// Nanosecond delta suitable for `dispatch_time` / `dispatch_walltime`.
    #[inline]
    #[must_use]
    pub fn as_nanos(self) -> i64 {
        match self {
            Self::Seconds(s) => clamped_product(s, Self::NSEC_PER_SEC),
            Self::Milliseconds(ms) => clamped_product(ms, Self::NSEC_PER_MSEC),
            Self::Microseconds(us) => clamped_product(us, Self::NSEC_PER_USEC),
            Self::Nanoseconds(ns) => ns,
            Self::Never => i64::MAX,
        }
    }
}

#[inline]
fn clamped_product(m1: i64, m2: i64) -> i64 {
    debug_assert!(m2 > 0, "multiplier must be positive");
    m1.checked_mul(m2)
        .unwrap_or(if m1 > 0 { i64::MAX } else { i64::MIN })
}

#[cfg(target_vendor = "apple")]
mod mach_timebase {
    use std::sync::OnceLock;

    #[repr(C)]
    struct mach_timebase_info_data_t {
        numer: u32,
        denom: u32,
    }

    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();

    pub(super) fn numer_denom() -> (u32, u32) {
        *TIMEBASE.get_or_init(|| {
            let mut info = mach_timebase_info_data_t { numer: 1, denom: 1 };
            // SAFETY: `info` is a valid out-parameter for `mach_timebase_info`.
            let status = unsafe { mach_timebase_info(&mut info) };
            if status != 0 {
                // Identity timebase is ~41x wrong on Apple Silicon (real ratio 125/3);
                // never degrade silently.
                debug_assert_eq!(status, 0, "mach_timebase_info failed (status {status})");
                std::eprintln!(
                    "dispatch2: mach_timebase_info failed (status {status}); falling back to identity timebase — DispatchTime scaling will be wrong"
                );
                return (1, 1);
            }
            (info.numer, info.denom)
        })
    }

    extern "C" {
        fn mach_timebase_info(info: *mut mach_timebase_info_data_t) -> core::ffi::c_int;
    }
}

impl DispatchTime {
    /// The current uptime time.
    #[doc(alias = "DISPATCH_TIME_NOW")]
    pub const NOW: Self = Self(0);

    /// A time in the distant future.
    #[doc(alias = "DISPATCH_TIME_FOREVER")]
    pub const FOREVER: Self = Self(u64::MAX);

    /// Alias for [`FOREVER`](Self::FOREVER), matching Swift's `distantFuture`.
    pub const DISTANT_FUTURE: Self = Self::FOREVER;

    /// The current wall-clock time.
    #[doc(alias = "DISPATCH_WALLTIME_NOW")]
    pub const WALLTIME_NOW: Self = Self(!1);

    /// Returns the current uptime instant.
    #[inline]
    #[must_use]
    pub fn now() -> Self {
        Self::NOW.time(0)
    }

    /// Create a [`DispatchTime`] from nanoseconds since boot (uptime clock).
    ///
    /// On Apple platforms the value is scaled using `mach_timebase_info` when
    /// the timebase numerator and denominator differ.
    #[inline]
    #[must_use]
    pub fn from_uptime_nanos(nanos: u64) -> Self {
        let raw = scale_uptime_nanos_to_raw(nanos);
        Self(raw)
    }

    /// Nanoseconds since boot represented by this uptime value.
    #[inline]
    #[must_use]
    pub fn uptime_nanos(self) -> u64 {
        scale_raw_to_uptime_nanos(self.0)
    }

    /// Create a wall-clock [`DispatchTime`] from an optional anchor plus a delta.
    ///
    /// Passing `None` for `when` uses the current wall time from `gettimeofday(3)`.
    #[cfg(all(feature = "libc", feature = "std"))]
    #[inline]
    pub fn from_walltime(when: Option<SystemTime>, delta: Duration) -> Result<Self, ()> {
        Ok(DispatchWallTime::from_walltime(when, delta)?.into_dispatch_time())
    }

    /// Create a wall-clock [`DispatchTime`] from a [`SystemTime`].
    #[cfg(all(feature = "libc", feature = "std"))]
    #[inline]
    pub fn from_system_time(time: SystemTime) -> Result<Self, ()> {
        Ok(DispatchWallTime::from_system_time(time)?.into_dispatch_time())
    }

    /// Create a wall-clock [`DispatchTime`] relative to now.
    #[cfg(all(feature = "libc", feature = "std"))]
    #[inline]
    pub fn from_now(delta: Duration) -> Result<Self, ()> {
        Ok(DispatchWallTime::from_now(delta)?.into_dispatch_time())
    }
}

impl DispatchWallTime {
    /// Distant future on the wall clock.
    pub const DISTANT_FUTURE: Self = Self(u64::MAX);

    /// Current wall time.
    #[inline]
    #[must_use]
    pub const fn now() -> Self {
        Self(DispatchTime::WALLTIME_NOW.0)
    }

    /// Create from an optional anchor [`SystemTime`] and delta.
    #[cfg(all(feature = "libc", feature = "std"))]
    #[inline]
    pub fn from_walltime(when: Option<SystemTime>, delta: Duration) -> Result<Self, ()> {
        let delta_nanos = numeric_cast::duration_to_nanos_i64(delta)?;

        let raw = match when {
            None => DispatchTime::WALLTIME_NOW.time(delta_nanos).0,
            Some(time) => {
                let epoch = time
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_err(|_| ())?;
                let tv_sec = numeric_cast::u64_to_i64(epoch.as_secs())?;
                let ts = libc::timespec {
                    tv_sec: tv_sec as libc::time_t,
                    tv_nsec: numeric_cast::u32_to_c_long(epoch.subsec_nanos()),
                };
                // SAFETY: `ts` is a valid stack-allocated timespec.
                unsafe { DispatchTime::walltime(&ts as *const libc::timespec, delta_nanos).0 }
            }
        };
        Ok(Self(raw))
    }

    /// Create from a [`SystemTime`].
    #[cfg(all(feature = "libc", feature = "std"))]
    #[inline]
    pub fn from_system_time(time: SystemTime) -> Result<Self, ()> {
        Self::from_walltime(Some(time), Duration::ZERO)
    }

    /// Create relative to the current wall clock.
    #[cfg(all(feature = "libc", feature = "std"))]
    #[inline]
    pub fn from_now(delta: Duration) -> Result<Self, ()> {
        Self::from_walltime(None, delta)
    }

    /// Create from a `libc::timespec` anchor.
    #[cfg(feature = "libc")]
    #[inline]
    #[must_use]
    pub fn from_timespec(timespec: libc::timespec) -> Self {
        // SAFETY: `timespec` is a valid stack value.
        Self(unsafe { DispatchTime::walltime(&timespec as *const libc::timespec, 0).0 })
    }

    #[inline]
    pub(crate) const fn into_dispatch_time(self) -> DispatchTime {
        DispatchTime(self.0)
    }
}

impl From<DispatchWallTime> for DispatchTime {
    #[inline]
    fn from(value: DispatchWallTime) -> Self {
        value.into_dispatch_time()
    }
}

impl Add<DispatchTimeInterval> for DispatchTime {
    type Output = Self;

    #[inline]
    fn add(self, rhs: DispatchTimeInterval) -> Self::Output {
        self.time(rhs.as_nanos())
    }
}

impl Sub<DispatchTimeInterval> for DispatchTime {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: DispatchTimeInterval) -> Self::Output {
        self.time(-rhs.as_nanos())
    }
}

impl Add<DispatchTimeInterval> for DispatchWallTime {
    type Output = Self;

    #[inline]
    fn add(self, rhs: DispatchTimeInterval) -> Self::Output {
        Self((DispatchTime(self.0) + rhs).0)
    }
}

impl Sub<DispatchTimeInterval> for DispatchWallTime {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: DispatchTimeInterval) -> Self::Output {
        Self((DispatchTime(self.0) - rhs).0)
    }
}

impl TryFrom<Duration> for DispatchTime {
    type Error = ();

    #[inline]
    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        let delta = numeric_cast::duration_to_nanos_i64(value)?;
        Ok(Self::NOW.time(delta))
    }
}

impl TryFrom<Duration> for DispatchTimeInterval {
    type Error = ();

    #[inline]
    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Ok(Self::Nanoseconds(numeric_cast::duration_to_nanos_i64(
            value,
        )?))
    }
}

#[inline]
fn scale_uptime_nanos_to_raw(nanos: u64) -> u64 {
    if nanos == u64::MAX {
        return nanos;
    }
    #[cfg(target_vendor = "apple")]
    {
        let (numer, denom) = mach_timebase::numer_denom();
        if numer == denom {
            return nanos;
        }
        let scaled = u128::from(nanos)
            .saturating_mul(u128::from(denom))
            .saturating_add(u128::from(numer) - 1);
        (scaled / u128::from(numer)).min(u128::from(u64::MAX)) as u64
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        nanos
    }
}

#[inline]
fn scale_raw_to_uptime_nanos(raw: u64) -> u64 {
    if raw == u64::MAX {
        return raw;
    }
    #[cfg(target_vendor = "apple")]
    {
        let (numer, denom) = mach_timebase::numer_denom();
        if numer == denom {
            return raw;
        }
        let scaled = u128::from(raw).saturating_mul(u128::from(numer)) / u128::from(denom);
        scaled.min(u128::from(u64::MAX)) as u64
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        raw
    }
}

#[cfg(all(test, feature = "libc", feature = "std"))]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn from_duration() {
        let when = DispatchTime::try_from(Duration::from_millis(100)).unwrap();
        assert_ne!(when, DispatchTime::NOW);
    }

    #[test]
    fn from_walltime_now() {
        let when = DispatchWallTime::from_now(Duration::from_secs(1)).unwrap();
        assert_ne!(when.0, DispatchTime::NOW.0);
    }

    #[test]
    fn from_system_time() {
        let when =
            DispatchWallTime::from_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
                .unwrap();
        assert_ne!(when.0, DispatchTime::NOW.0);
    }

    #[test]
    fn interval_add() {
        let t = DispatchTime::now() + DispatchTimeInterval::Seconds(1);
        assert_ne!(t, DispatchTime::NOW);
    }

    #[test]
    fn uptime_roundtrip() {
        let nanos = 1_000_000u64;
        let t = DispatchTime::from_uptime_nanos(nanos);
        assert!(t.uptime_nanos() >= nanos);
    }
}

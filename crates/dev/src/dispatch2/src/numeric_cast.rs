use core::ffi::c_long;
use core::time::Duration;

pub(crate) const fn i16_to_c_long(value: i16) -> c_long {
    value as c_long
}

pub(crate) const fn c_long_to_isize(value: c_long) -> isize {
    value as isize
}

pub(crate) const fn u32_to_isize(value: u32) -> isize {
    value as isize
}

pub(crate) const fn u32_to_c_long(value: u32) -> c_long {
    value as c_long
}

pub(crate) fn u64_to_i64(value: u64) -> Result<i64, ()> {
    i64::try_from(value).map_err(|_| ())
}

pub(crate) fn u128_to_i64(value: u128) -> Result<i64, ()> {
    i64::try_from(value).map_err(|_| ())
}

pub(crate) fn duration_to_nanos_i64(duration: Duration) -> Result<i64, ()> {
    u128_to_i64(duration.as_nanos())
}

pub(crate) const fn i32_to_usize(value: i32) -> usize {
    value as usize
}

pub(crate) const fn u32_to_usize(value: u32) -> usize {
    value as usize
}

//! Times in the computer's own time zone, for dates and times shown to the
//! user (the `time` crate on its own only knows UTC here).

use time::{OffsetDateTime, UtcOffset};

/// A Unix time in the local time zone, or in UTC if the zone can't be read.
pub fn local_datetime(timestamp: i64) -> Option<OffsetDateTime> {
    let utc = OffsetDateTime::from_unix_timestamp(timestamp).ok()?;
    let offset = local_offset_seconds(timestamp)
        .and_then(|seconds| UtcOffset::from_whole_seconds(seconds).ok())
        .unwrap_or(UtcOffset::UTC);
    Some(utc.to_offset(offset))
}

/// The local zone's offset from UTC at `timestamp`, daylight saving included.
fn local_offset_seconds(timestamp: i64) -> Option<i32> {
    let time = libc::time_t::try_from(timestamp).ok()?;
    let mut tm = std::mem::MaybeUninit::<libc::tm>::uninit();
    // SAFETY: localtime_r reads `time` and fills in `tm`; it returns null
    // without touching `tm` on failure, and `tm` is only read on success.
    let filled = unsafe { libc::localtime_r(&time, tm.as_mut_ptr()) };
    if filled.is_null() {
        return None;
    }
    // SAFETY: localtime_r succeeded, so it initialized `tm`.
    let tm = unsafe { tm.assume_init() };
    i32::try_from(tm.tm_gmtoff).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_times_keep_the_same_instant() {
        let stamp = 1_791_326_348;
        let local = local_datetime(stamp).unwrap();
        assert_eq!(local.unix_timestamp(), stamp);
        assert!(local.offset().whole_hours().abs() <= 14);
    }
}

//! Creation time of a process, formatted like a UTC round-trip timestamp.
//!
//! The Windows query uses `GetProcessTimes` directly. Spawning PowerShell for
//! the same value costs the first launch of that runtime, which on a CI
//! runner is many seconds.

use time::OffsetDateTime;

/// 100-nanosecond ticks between 1601-01-01 and 1970-01-01.
const UNIX_EPOCH_FILETIME: u64 = 11_644_473_600 * 10_000_000;

/// `ticks` is a Windows `FILETIME`: 100-nanosecond intervals since 1601-01-01 UTC.
#[must_use]
pub fn filetime_to_roundtrip(ticks: u64) -> Option<String> {
    let unix_100ns = ticks.checked_sub(UNIX_EPOCH_FILETIME)?;
    let secs = unix_100ns / 10_000_000;
    let frac = unix_100ns % 10_000_000;
    let dt = OffsetDateTime::from_unix_timestamp(i64::try_from(secs).ok()?).ok()?;
    Some(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{frac:07}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
    ))
}

/// Creation time of `pid`, or `None` when the process is gone or not queryable.
#[cfg(windows)]
#[must_use]
pub fn process_creation_identity(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    if pid == 0 {
        return None;
    }
    // SAFETY: a null handle means this pid cannot be queried. On success the
    // four FILETIME outputs are written by GetProcessTimes before we read them,
    // and the handle is closed on every path.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation = std::mem::zeroed();
        let mut exit = std::mem::zeroed();
        let mut kernel = std::mem::zeroed();
        let mut user = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let ticks = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
        filetime_to_roundtrip(ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_filetime_uses_seven_fractional_digits() {
        assert_eq!(
            filetime_to_roundtrip(UNIX_EPOCH_FILETIME).as_deref(),
            Some("1970-01-01T00:00:00.0000000Z")
        );
        assert_eq!(
            filetime_to_roundtrip(UNIX_EPOCH_FILETIME + 2_182_731).as_deref(),
            Some("1970-01-01T00:00:00.2182731Z")
        );
    }
}

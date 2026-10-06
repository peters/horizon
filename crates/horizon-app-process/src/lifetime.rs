//! One machine's monotonic clock is shared by the parent and its private guardians.
use crate::{Error, Result};
use std::time::Duration;

#[cfg(unix)]
fn clock_millis(round_up: bool) -> Result<u64> {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let seconds = u64::try_from(time.tv_sec).map_err(|_| Error::Invalid)?;
    let nanos = u64::try_from(time.tv_nsec).map_err(|_| Error::Invalid)?;
    let fraction = if round_up {
        nanos.div_ceil(1_000_000)
    } else {
        nanos / 1_000_000
    };
    seconds
        .checked_mul(1000)
        .and_then(|value| value.checked_add(fraction))
        .ok_or(Error::Invalid)
}
#[cfg(not(unix))]
fn clock_millis(_round_up: bool) -> Result<u64> {
    Err(Error::Invalid)
}

/// # Errors
/// Capture a finite deadline before any guardian spawn or durable handshake.
pub fn deadline_after(lifetime: Duration) -> Result<u64> {
    if lifetime.is_zero() || lifetime > Duration::from_mins(30) {
        return Err(Error::Invalid);
    }
    clock_millis(false)?
        .checked_add(u64::try_from(lifetime.as_millis()).map_err(|_| Error::Invalid)?)
        .ok_or(Error::Invalid)
}
/// # Errors
/// Refuse expired deadlines; rounding is conservative and cannot renew an original lease.
pub fn remaining(deadline_millis: u64) -> Result<Duration> {
    let milliseconds = deadline_millis.saturating_sub(clock_millis(true)?);
    if milliseconds == 0 || milliseconds > 1_800_000 {
        return Err(Error::Timeout);
    }
    Ok(Duration::from_millis(milliseconds))
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn serialized_deadline_includes_handshake_time_and_refuses_expiry() {
        let deadline = deadline_after(Duration::from_millis(500)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(remaining(deadline).unwrap() <= Duration::from_millis(480));
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(remaining(deadline), Err(Error::Timeout));
    }
}

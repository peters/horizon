//! Key events and lease timing for one text stroke, independent of the X connection.

use super::keymap;

/// The shortest interval between the estimated server time and the lease
/// before the next stroke. Below it, the lease is extended first.
pub(super) const LEASE_GUARD_MS: u64 = 1_000;

/// Sends the key events of one stroke through `fake(kind, keycode)`: Shift
/// press, key press, key release and Shift release. Each release is sent even
/// when an earlier event failed, because a failed checked request can still
/// have reached the server. Returns the first error.
pub(super) fn send<E>(
    keycode: u8,
    shift: Option<u8>,
    press: u8,
    release: u8,
    mut fake: impl FnMut(u8, u8) -> Result<(), E>,
) -> Result<(), E> {
    let mut first = Ok(());
    let mut keep = |result: Result<(), E>| {
        if first.is_ok() {
            first = result;
        }
    };
    if let Some(shift) = shift {
        keep(fake(press, shift));
    }
    keep(fake(press, keycode));
    keep(fake(release, keycode));
    if let Some(shift) = shift {
        keep(fake(release, shift));
    }
    first
}

/// The lease of the record while strokes are sent, on the planning timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Lease {
    /// The end of the lease in the record.
    pub until: u64,
    /// The last observed server time.
    pub observed: u64,
}

impl Lease {
    /// The lease written before the first stroke, at the server time of the plan.
    pub(super) fn start(strokes: usize) -> Self {
        Self {
            until: keymap::TIMELINE_NOW + keymap::lease_ms(strokes),
            observed: keymap::TIMELINE_NOW,
        }
    }

    /// Whether the lease must be extended before the next stroke.
    /// `elapsed_ms` is the local time since the last server time observation.
    pub(super) fn needs_extension(self, elapsed_ms: u64) -> bool {
        self.observed.saturating_add(elapsed_ms).saturating_add(LEASE_GUARD_MS) >= self.until
    }

    /// The lease for the `remaining` strokes after the server time `observed`.
    pub(super) fn extended(observed: u64, remaining: usize) -> Self {
        Self {
            until: observed.saturating_add(keymap::lease_ms(remaining)),
            observed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LEASE_GUARD_MS, Lease, send};
    use crate::x11::keymap::{self, TIMELINE_NOW};

    const PRESS: u8 = 2;
    const RELEASE: u8 = 3;

    #[test]
    fn each_release_is_sent_after_a_failed_press() {
        let mut events = Vec::new();
        let result = send(38, Some(50), PRESS, RELEASE, |kind, keycode| {
            events.push((kind, keycode));
            if (kind, keycode) == (PRESS, 38) {
                Err("press")
            } else if kind == RELEASE {
                Err("release")
            } else {
                Ok(())
            }
        });
        assert_eq!(result, Err("press"), "the first error is kept");
        assert_eq!(events, vec![(PRESS, 50), (PRESS, 38), (RELEASE, 38), (RELEASE, 50)]);

        let mut events = Vec::new();
        let result = send(38, None, PRESS, RELEASE, |kind, keycode| {
            events.push((kind, keycode));
            Err::<(), _>(kind)
        });
        assert_eq!(result, Err(PRESS));
        assert_eq!(events, vec![(PRESS, 38), (RELEASE, 38)], "no Shift key");
        assert_eq!(send(38, None, PRESS, RELEASE, |_, _| Ok::<(), ()>(())), Ok(()));
    }

    #[test]
    fn a_slow_dispatch_extends_the_lease_from_the_observed_server_time() {
        let lease = Lease::start(256);
        assert_eq!(lease.until, TIMELINE_NOW + 11_240);
        // Until the guard interval before the lease, no extension is necessary.
        assert!(!lease.needs_extension(11_240 - LEASE_GUARD_MS - 1));
        assert!(lease.needs_extension(11_240 - LEASE_GUARD_MS));
        // A stalled server: the strokes continue after the first lease.
        let observed = TIMELINE_NOW + 30_000;
        let extended = Lease::extended(observed, 100);
        assert_eq!(extended.until, observed + keymap::lease_ms(100));
        assert!(extended.until > observed + LEASE_GUARD_MS);
        assert!(!extended.needs_extension(0));
        assert!(extended.needs_extension(keymap::lease_ms(100) - LEASE_GUARD_MS));
        // The lease of the last stroke still covers the guard interval.
        assert!(Lease::extended(observed, 1).until > observed + LEASE_GUARD_MS);
    }
}

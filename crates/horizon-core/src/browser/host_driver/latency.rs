//! The timing lines of a panel whose browser runs behind a driver on a host.
//!
//! `HORIZON_BROWSER_LATENCY=1` writes the same four lines of a local browser
//! panel (`docs/latency-trace.md`, `horizon_browser::latency`), with the same
//! field names where they mean the same thing. This path has fewer hops on this
//! computer, because the screen, the driver and the browser are on another
//! machine:
//!
//! - `latency action`: one command of this computer, from the moment the panel
//!   queued it to the picture that showed its effect. `queued_us`, `taken_us` and
//!   `written_us` are the queue of the link and the write of the request;
//!   `received_us`, `decoded_us` and `handed_us` are the pushed picture that
//!   followed it, so `received_us` carries the whole link: the far machine, the
//!   `ssh` route and the return of the frame. `ack` says whether the
//!   acknowledgement of that picture left before its decode or after it.
//! - `latency frame`: every picture of the link, with its bytes on the wire, its
//!   size and the time of its decode.
//! - `latency stall`: one pass of the worker over 50 ms, with the name of the
//!   largest wait of that pass. On this path the waits are the link, the send of
//!   a request and the decode of a picture.
//!
//! `latency paint` belongs to the window, which paints a picture of this link as
//! it paints one of a local browser: `horizon-browser` writes that line.
//!
//! Nothing of the far machine is instrumented. A slow host and a slow link are the
//! same line here, and the names of the waits separate them only as far as this
//! end can see.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// A pass of the worker that blocked longer than this is reported.
const STALL_THRESHOLD: Duration = Duration::from_millis(50);

/// Longest history that a test can read back.
#[cfg(test)]
const KEPT_LINES: usize = 32;

/// One command of this computer, from the panel's queue to the picture after it.
#[derive(Debug)]
struct Action {
    seq: u64,
    queued_at: Instant,
    taken_at: Option<Instant>,
    written_at: Option<Instant>,
    method: &'static str,
}

/// The timing of one panel's link.
#[derive(Debug, Default)]
pub(super) struct LinkProbe {
    enabled: bool,
    next_seq: u64,
    queued: Option<Action>,
    in_flight: Option<Action>,
    /// Lines that this probe wrote, kept for a test that has no subscriber.
    #[cfg(test)]
    lines: Vec<String>,
}

impl LinkProbe {
    /// A probe that follows the switch of the person.
    pub(super) fn new() -> Self {
        Self {
            enabled: horizon_browser::latency::enabled(),
            ..Self::default()
        }
    }

    /// A probe that writes its lines whatever the switch says, for a test.
    #[cfg(test)]
    pub(super) fn forced() -> Self {
        Self {
            enabled: true,
            ..Self::default()
        }
    }

    /// Whether this probe writes anything.
    pub(super) fn enabled(&self) -> bool {
        self.enabled
    }

    /// The panel queued one command of this computer.
    pub(super) fn queued(&mut self, queued_at: Instant) {
        if !self.enabled {
            return;
        }
        self.next_seq = self.next_seq.saturating_add(1);
        self.queued = Some(Action {
            seq: self.next_seq,
            queued_at,
            taken_at: None,
            written_at: None,
            method: "-",
        });
    }

    /// The worker took that command from the queue of the link.
    pub(super) fn taken(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(action) = self.queued.take() {
            self.queued = Some(Action {
                taken_at: Some(Instant::now()),
                ..action
            });
        }
    }

    /// The worker wrote that command to the link as `method`.
    pub(super) fn written(&mut self, method: &'static str) {
        if !self.enabled {
            return;
        }
        if let Some(action) = self.queued.take() {
            self.in_flight = Some(Action {
                written_at: Some(Instant::now()),
                method,
                ..action
            });
        }
    }

    /// One pushed picture is decoded into the frame slot of the panel.
    pub(super) fn frame_handed(
        &mut self,
        frame_seq: u64,
        bytes: usize,
        size: (u32, u32),
        received_at: Instant,
        decoded_at: Instant,
        ack_before_decode: bool,
    ) {
        if !self.enabled {
            return;
        }
        let mut line = format!(
            "latency frame seq={frame_seq} bytes={bytes} width={} height={}",
            size.0, size.1
        );
        let _ = write!(
            line,
            " received_to_decoded_us={} ack={}",
            micros(received_at, decoded_at),
            ack_placement(ack_before_decode)
        );
        self.emit(&line);

        let Some(action) = self.in_flight.take() else {
            return;
        };
        let taken_at = action.taken_at.unwrap_or(action.queued_at);
        let written_at = action.written_at.unwrap_or(action.queued_at);
        let mut line = format!(
            "latency action seq={} method={} queued_us=0 taken_us={} written_us={}",
            action.seq,
            action.method,
            micros(action.queued_at, taken_at),
            micros(action.queued_at, written_at)
        );
        let _ = write!(
            line,
            " received_us={} decoded_us={} handed_us={} frame={frame_seq} bytes={bytes} width={} height={} \
             decode_us={} ack={}",
            micros(action.queued_at, received_at),
            micros(action.queued_at, decoded_at),
            micros(action.queued_at, decoded_at),
            size.0,
            size.1,
            micros(received_at, decoded_at),
            ack_placement(ack_before_decode)
        );
        self.emit(&line);
    }

    /// One pass of the worker that blocked, with the largest wait of that pass.
    pub(super) fn stall(&mut self, pass: Duration, waited: Option<(&str, Duration)>) {
        if !self.enabled || pass <= STALL_THRESHOLD {
            return;
        }
        let (method, waited_us) = waited.map_or(("-".to_string(), "-".to_string()), |(method, waited)| {
            (method.to_string(), micros_of(waited).to_string())
        });
        self.emit(&format!(
            "latency stall pass_us={} waited={method} waited_us={waited_us}",
            micros_of(pass)
        ));
    }

    fn emit(&mut self, line: &str) {
        tracing::info!("{line}");
        self.remember(line);
    }

    /// Keep one line for a test that cannot install a subscriber. A build of the
    /// program keeps none, so the switch costs no allocation.
    #[cfg(test)]
    fn remember(&mut self, line: &str) {
        if self.lines.len() >= KEPT_LINES {
            self.lines.remove(0);
        }
        self.lines.push(line.to_string());
    }

    /// A build of the program keeps no line: the switch writes it and stops. It
    /// takes the address of the probe so the shape of the call is the same in
    /// both builds.
    #[cfg(not(test))]
    fn remember(&mut self, _line: &str) {
        let _ = std::ptr::addr_of!(self.enabled);
    }

    /// The lines that this probe wrote, for a test.
    #[cfg(test)]
    pub(super) fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }
}

/// How the acknowledgement of a picture stood with its decode.
const fn ack_placement(before_decode: bool) -> &'static str {
    if before_decode { "before" } else { "after" }
}

fn micros(from: Instant, to: Instant) -> u64 {
    micros_of(to.saturating_duration_since(from))
}

fn micros_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::LinkProbe;

    #[test]
    fn a_picture_and_the_command_before_it_write_the_fields_of_the_local_path() {
        let mut probe = LinkProbe::forced();
        let queued_at = Instant::now();
        probe.queued(queued_at);
        probe.taken();
        probe.written("input");
        let received_at = queued_at + Duration::from_millis(40);
        let decoded_at = received_at + Duration::from_millis(3);
        probe.frame_handed(7, 51_000, (1280, 714), received_at, decoded_at, false);
        let lines = probe.take_lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].starts_with("latency frame seq=7 bytes=51000 width=1280 height=714"),
            "{lines:?}"
        );
        assert!(lines[0].contains("received_to_decoded_us=3000"), "{lines:?}");
        assert!(lines[0].contains("ack=after"), "{lines:?}");
        assert!(lines[1].starts_with("latency action seq=1 method=input"), "{lines:?}");
        for field in [
            "queued_us=0",
            "taken_us=",
            "written_us=",
            "received_us=",
            "decoded_us=",
            "handed_us=",
            "frame=7",
            "bytes=51000",
            "width=1280",
            "height=714",
            "decode_us=3000",
        ] {
            assert!(lines[1].contains(field), "{field} is missing: {lines:?}");
        }
    }

    #[test]
    fn a_picture_without_a_command_writes_only_the_frame_line() {
        let mut probe = LinkProbe::forced();
        let now = Instant::now();
        probe.frame_handed(3, 40_000, (800, 600), now, now + Duration::from_millis(1), true);
        let lines = probe.take_lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("ack=before"), "{lines:?}");
    }

    #[test]
    fn a_pass_over_the_threshold_names_the_wait_and_a_short_one_writes_nothing() {
        let mut probe = LinkProbe::forced();
        probe.stall(Duration::from_millis(10), Some(("link", Duration::from_millis(9))));
        assert!(probe.take_lines().is_empty(), "a short pass writes nothing");
        probe.stall(Duration::from_millis(120), Some(("link", Duration::from_millis(110))));
        let lines = probe.take_lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("latency stall pass_us=120000"), "{lines:?}");
        assert!(lines[0].contains("waited=link waited_us=110000"), "{lines:?}");
        probe.stall(Duration::from_millis(60), None);
        let lines = probe.take_lines();
        assert!(lines[0].contains("waited=- waited_us=-"), "{lines:?}");
    }

    #[test]
    fn a_probe_of_the_switch_writes_nothing_while_the_switch_is_off() {
        // The switch is off in a test run unless the person turned it on.
        if horizon_browser::latency::enabled() {
            return;
        }
        let mut probe = LinkProbe::new();
        assert!(!probe.enabled());
        probe.queued(Instant::now());
        probe.frame_handed(1, 10, (2, 2), Instant::now(), Instant::now(), true);
        probe.stall(Duration::from_secs(1), None);
        assert!(probe.take_lines().is_empty());
    }
}

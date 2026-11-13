//! Opt-in timing of one person's key press, from the window event to the painted echo.
//!
//! `HORIZON_TERMINAL_LATENCY=1`, set before Horizon starts, writes one `tracing`
//! INFO line per key press of a terminal panel to stderr, so the answer time of a
//! terminal of a far machine is measured instead of guessed. The line keeps the
//! target of this module, and the window adds that target to its filter while the
//! switch is on, so the switch is the only setting that it needs. It is the switch
//! of the action of a browser panel (`HORIZON_BROWSER_LATENCY`, in the browser
//! crate) in another place: the same rule for what counts as on, the same target
//! of the module that writes the line, the same `_us` fields, and one relaxed
//! atomic load per hook is all that it costs while it is off. See
//! `docs/latency-trace.md`.
//!
//! The line carries four hops, always in this order, so a table of them reads
//! down the same columns:
//!
//! - `event_to_write_us`: from the frame that took the key event to the write of
//!   its bytes into the event loop of the terminal.
//! - `write_to_read_us`: from that write to the first output that the event loop
//!   read after it. For a shell of a far machine this is about one round trip.
//! - `read_to_request_us`: from that read to the frame that took the changed
//!   grid. Before [`super::OutputWake`] this was the poll of the window, up to a
//!   second; after it, it is the wake of the reader thread.
//! - `request_to_paint_us`: from that frame to the painted grid, which is the
//!   work of one frame.
//!
//! `total_us` repeats the sum, from the event to the pixel.
//!
//! A press leaves no line when nobody painted its echo within
//! [`STALE_AFTER`], for example a panel that a person typed into and then hid.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// `HORIZON_TERMINAL_LATENCY` is off, on, or not read from the environment yet.
const UNREAD: u8 = u8::MAX;

static ENABLED: AtomicU8 = AtomicU8::new(UNREAD);

/// The presses that one terminal keeps between their write and their paint.
const KEPT_PRESSES: usize = 16;

/// A press whose echo no frame painted within this time leaves no line.
pub(super) const STALE_AFTER: Duration = Duration::from_secs(5);

/// Whether the person asked for the lines. Read once, then cached, so a terminal
/// that starts later reads the switch of the process.
#[must_use]
pub fn enabled() -> bool {
    let cached = ENABLED.load(Ordering::Relaxed);
    if cached != UNREAD {
        return cached == 1;
    }
    let value = std::env::var_os("HORIZON_TERMINAL_LATENCY").is_some_and(|value| !value.is_empty() && value != "0");
    ENABLED.store(u8::from(value), Ordering::Relaxed);
    value
}

/// One terminal's timing state.
///
/// The reader thread of the terminal and the frame that paints it both reach
/// this through an `Arc`, so the presses live behind a lock. The switch is a
/// plain atomic, so a hook whose switch is off takes no lock at all.
pub(super) struct Probe {
    on: AtomicBool,
    presses: Mutex<Presses>,
}

impl Probe {
    pub(super) fn new() -> Self {
        Self {
            on: AtomicBool::new(enabled()),
            presses: Mutex::new(Presses::default()),
        }
    }

    fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Whether this terminal writes the latency lines, for the frame that takes a
    /// key event or paints its echo.
    pub(super) fn timed(&self) -> bool {
        self.on()
    }

    /// A frame wrote the bytes of one key press into the event loop.
    pub(super) fn key_written(&self, event_at: Instant, bytes: usize) {
        if !self.on() {
            return;
        }
        self.presses().key_written(event_at, bytes);
    }

    /// The event loop of the terminal read output, which carries the echo of the
    /// oldest press that still waits for one.
    pub(super) fn output_read(&self) {
        if !self.on() {
            return;
        }
        self.presses().output_read();
    }

    /// A frame drained the events of the terminal.
    pub(super) fn drained(&self) {
        if !self.on() {
            return;
        }
        self.presses().drained();
    }

    /// A frame painted the grid of the terminal.
    pub(super) fn painted(&self) {
        if !self.on() {
            return;
        }
        self.presses().painted();
    }

    fn presses(&self) -> std::sync::MutexGuard<'_, Presses> {
        self.presses.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Turn the lines on or off for this terminal only, so a test of the lines
    /// does not depend on the switch of the process.
    #[cfg(test)]
    pub(super) fn force(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    /// The lines this terminal wrote, for a test that checks their fields.
    #[cfg(test)]
    pub(super) fn take_lines(&self) -> Vec<String> {
        std::mem::take(&mut self.presses().lines)
    }
}

/// One key press, timed from the window event that carried it.
#[derive(Clone, Copy, Debug)]
struct Press {
    seq: u64,
    bytes: usize,
    event_at: Instant,
    written_at: Option<Instant>,
    read_at: Option<Instant>,
    drained_at: Option<Instant>,
}

#[derive(Default)]
struct Presses {
    next_seq: u64,
    queue: VecDeque<Press>,
    #[cfg(test)]
    lines: Vec<String>,
}

impl Presses {
    fn key_written(&mut self, event_at: Instant, bytes: usize) {
        while self.queue.len() >= KEPT_PRESSES {
            self.queue.pop_front();
        }
        self.next_seq += 1;
        self.queue.push_back(Press {
            seq: self.next_seq,
            bytes,
            event_at,
            written_at: Some(Instant::now()),
            read_at: None,
            drained_at: None,
        });
    }

    fn output_read(&mut self) {
        let now = Instant::now();
        if let Some(press) = self.queue.iter_mut().find(|press| press.read_at.is_none()) {
            press.read_at = Some(now);
        }
    }

    fn drained(&mut self) {
        let now = Instant::now();
        if let Some(press) = self
            .queue
            .iter_mut()
            .find(|press| press.read_at.is_some() && press.drained_at.is_none())
        {
            press.drained_at = Some(now);
        }
    }

    fn painted(&mut self) {
        let painted_at = Instant::now();
        self.queue
            .retain(|press| painted_at.saturating_duration_since(press.event_at) < STALE_AFTER);
        while let Some(index) = self.queue.iter().position(|press| press.drained_at.is_some()) {
            let Some(press) = self.queue.remove(index) else {
                break;
            };
            let line = format!("latency key {}", key_fields(&press, painted_at));
            // The default filter of the window allows `horizon_core=info`, so the
            // module path is the target that makes the switch enough on its own.
            tracing::info!("{line}");
            #[cfg(test)]
            self.lines.push(line);
        }
    }
}

/// The fields of one key line, in the order they are written.
fn key_fields(press: &Press, painted_at: Instant) -> String {
    let written_at = press.written_at.unwrap_or(press.event_at);
    let read_at = press.read_at.unwrap_or(written_at);
    let drained_at = press.drained_at.unwrap_or(read_at);
    let mut line = String::new();
    let _ = write!(
        line,
        "seq={} bytes={} event_to_write_us={} write_to_read_us={} read_to_request_us={} request_to_paint_us={} \
         total_us={}",
        press.seq,
        press.bytes,
        micros(press.event_at, written_at),
        micros(written_at, read_at),
        micros(read_at, drained_at),
        micros(drained_at, painted_at),
        micros(press.event_at, painted_at)
    );
    line
}

fn micros(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::{Press, Presses, Probe, STALE_AFTER, key_fields};

    #[test]
    fn a_key_line_keeps_its_fields_in_one_order() {
        let event_at = Instant::now();
        let press = Press {
            seq: 3,
            bytes: 1,
            event_at,
            written_at: Some(event_at + Duration::from_micros(300)),
            read_at: Some(event_at + Duration::from_micros(175_300)),
            drained_at: Some(event_at + Duration::from_micros(424_400)),
        };
        let painted_at = event_at + Duration::from_micros(432_742);
        let line = key_fields(&press, painted_at);
        let names: Vec<&str> = line
            .split(' ')
            .filter_map(|field| field.split_once('=').map(|(name, _)| name))
            .collect();

        assert_eq!(
            names,
            [
                "seq",
                "bytes",
                "event_to_write_us",
                "write_to_read_us",
                "read_to_request_us",
                "request_to_paint_us",
                "total_us",
            ]
        );
        let values: Vec<&str> = line
            .split(' ')
            .filter_map(|field| field.split_once('=').map(|(_, value)| value))
            .collect();
        assert_eq!(values[2], "300");
        assert_eq!(values[3], "175000");
        assert_eq!(values[4], "249100");
        assert_eq!(values[5], "8342");
        assert_eq!(values[6], "432742");
    }

    #[test]
    fn a_press_that_was_never_painted_leaves_its_slot_for_the_next_one() {
        let mut presses = Presses::default();
        let start = Instant::now();
        for _ in 0..super::KEPT_PRESSES + 4 {
            presses.key_written(start, 1);
        }

        assert_eq!(presses.queue.len(), super::KEPT_PRESSES);
        assert_eq!(presses.queue.back().map(|press| press.seq), Some(20));
    }

    #[test]
    fn a_press_that_nobody_painted_within_the_bound_leaves_no_line() {
        let mut presses = Presses::default();
        let stale = Instant::now()
            .checked_sub(STALE_AFTER + Duration::from_secs(1))
            .expect("a clock that can look back");
        presses.key_written(stale, 1);
        presses.output_read();
        presses.drained();
        presses.painted();

        assert!(presses.queue.is_empty());
        assert!(presses.lines.is_empty(), "a stale press writes no line");
    }

    #[test]
    fn a_pressed_key_that_was_written_read_drained_and_painted_writes_one_line() {
        let mut presses = Presses::default();
        presses.key_written(Instant::now(), 1);
        presses.output_read();
        presses.drained();
        presses.painted();

        assert_eq!(presses.lines.len(), 1);
        assert!(presses.queue.is_empty());
        assert!(presses.lines[0].starts_with("latency key seq=1 bytes=1 "));
    }

    /// Collects the target of every event that it is given, so a test can check
    /// which part of the window's log filter a line has to pass.
    #[derive(Default)]
    struct EventTargets(Mutex<Vec<String>>);

    impl Subscriber for EventTargets {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}

        fn event(&self, event: &Event<'_>) {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event.metadata().target().to_string());
        }
    }

    #[test]
    fn the_line_of_a_key_press_keeps_a_target_that_the_default_filter_allows() {
        // `main` filters an unset `RUST_LOG` to `horizon=info,horizon_core=info`,
        // so a line under any other target disappears until the person sets one
        // more variable. The switch of these lines must be the only setting.
        let subscriber = Arc::new(EventTargets::default());
        let targets = Arc::clone(&subscriber);
        tracing::subscriber::with_default(subscriber, || {
            let probe = Probe::new();
            probe.force(true);
            probe.key_written(Instant::now(), 1);
            probe.output_read();
            probe.drained();
            probe.painted();
        });
        let targets = targets
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        assert_eq!(
            targets,
            ["horizon_core::terminal::latency"],
            "the line of a key press is the only event, under the target that the default filter allows"
        );
        assert!(
            targets[0].starts_with("horizon_core"),
            "the default filter of the window allows horizon_core at INFO: {}",
            targets[0]
        );
    }
}

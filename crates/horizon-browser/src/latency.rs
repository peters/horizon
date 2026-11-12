//! Opt-in timing of one person's input, from the UI event to the painted pixel.
//!
//! `HORIZON_BROWSER_LATENCY=1` writes `tracing` INFO lines that follow one
//! action through the pipeline, so a slow far browser is measured instead of
//! guessed. The switch works in every build, including a release build without
//! the `trace-profiling` feature, and one relaxed atomic load is all it costs
//! while it is off.
//!
//! Four lines, each with the fields named in one place so the order is stable:
//!
//! - `latency action`: the first picture after one input was written. It carries
//!   the times from the UI event to the handoff to the UI, the size of that
//!   picture, its decode time and whether the ack left before or after it.
//! - `latency frame`: every picture, with its bytes, size and decode time.
//! - `latency paint`: the egui frame that applied a picture to a texture, so the
//!   last hop of the same action is measurable.
//! - `latency stall`: a driver pass that blocked on the protocol, with the name
//!   of the command it waited for.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// `HORIZON_BROWSER_LATENCY` is off, on, or not read from the environment yet.
const UNREAD: u8 = u8::MAX;

static ENABLED: AtomicU8 = AtomicU8::new(UNREAD);

#[cfg(test)]
static FORCE: AtomicU8 = AtomicU8::new(UNREAD);

/// Turn the lines on or off for a test, which must hold its own panel state.
#[cfg(test)]
pub(crate) fn force_for_tests(on: bool) {
    FORCE.store(u8::from(on), Ordering::Relaxed);
}

/// Whether the person asked for latency lines. Read once, then cached.
#[must_use]
pub fn enabled() -> bool {
    #[cfg(test)]
    {
        let forced = FORCE.load(Ordering::Relaxed);
        if forced != UNREAD {
            return forced == 1;
        }
    }
    let cached = ENABLED.load(Ordering::Relaxed);
    if cached != UNREAD {
        return cached == 1;
    }
    let value = std::env::var_os("HORIZON_BROWSER_LATENCY").is_some_and(|value| !value.is_empty() && value != "0");
    ENABLED.store(u8::from(value), Ordering::Relaxed);
    value
}

/// A driver pass that blocked longer than this is reported.
pub(crate) const STALL_THRESHOLD: Duration = Duration::from_millis(50);

/// One person's input action, timed from the moment the UI queued it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Action {
    pub seq: u64,
    pub queued_at: Instant,
    pub taken_at: Option<Instant>,
    pub written_at: Option<Instant>,
    pub method: &'static str,
}

impl Action {
    fn new(seq: u64) -> Self {
        Self {
            seq,
            queued_at: Instant::now(),
            taken_at: None,
            written_at: None,
            method: "-",
        }
    }

    /// The driver took this action from its queue.
    pub(crate) fn taken(mut self) -> Self {
        self.taken_at = Some(Instant::now());
        self
    }

    /// The driver wrote this action to the socket as `method`.
    pub(crate) fn written(mut self, method: &'static str) -> Self {
        self.written_at = Some(Instant::now());
        self.method = method;
        self
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Handoff {
    frame_seq: u64,
    action: Option<Action>,
    handed_at: Instant,
}

/// The pipeline state behind one panel's timing lines.
#[derive(Debug, Default)]
pub(crate) struct Probe {
    next_seq: u64,
    queued: Option<Action>,
    in_flight: Option<Action>,
    handed: Option<Handoff>,
    /// The lines this probe produced, kept for a test that cannot install a
    /// tracing subscriber. Bounded, and only filled while the switch is on.
    emitted: Vec<String>,
}

impl Probe {
    fn emit(&mut self, line: String) {
        if self.emitted.len() >= 32 {
            self.emitted.remove(0);
        }
        tracing::info!(target: "browser", "{line}");
        self.emitted.push(line);
    }

    /// Take the lines this probe produced since the last call, for a test.
    #[cfg(test)]
    pub(crate) fn take_emitted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.emitted)
    }
}

impl Probe {
    /// The UI queued one input of a person.
    pub(crate) fn queued(&mut self) {
        if !enabled() {
            return;
        }
        self.next_seq += 1;
        self.queued = Some(Action::new(self.next_seq));
    }

    /// The driver took the newest queued input. `None` while the switch is off.
    pub(crate) fn taken(&mut self) -> Option<Action> {
        if !enabled() {
            return None;
        }
        self.queued.take().map(Action::taken)
    }

    /// The driver wrote that input to the socket.
    pub(crate) fn written(&mut self, action: Action, method: &'static str) {
        if !enabled() {
            return;
        }
        self.in_flight = Some(action.written(method));
    }

    /// One decoded picture is handed to the UI.
    pub(crate) fn frame_handed(
        &mut self,
        frame_seq: u64,
        bytes: usize,
        size: (u32, u32),
        received_at: Instant,
        decoded_at: Instant,
        ack_before_decode: bool,
    ) {
        if !enabled() {
            return;
        }
        let line = format!(
            "latency frame {}",
            frame_fields(frame_seq, bytes, size, received_at, decoded_at, ack_before_decode)
        );
        self.emit(line);
        let Some(action) = self.in_flight.take() else {
            self.handed = Some(Handoff {
                frame_seq,
                action: None,
                handed_at: decoded_at,
            });
            return;
        };
        let line = format!(
            "latency action {}",
            action_fields(
                &action,
                frame_seq,
                bytes,
                size,
                received_at,
                decoded_at,
                ack_before_decode
            )
        );
        self.emit(line);
        self.handed = Some(Handoff {
            frame_seq,
            action: Some(action),
            handed_at: decoded_at,
        });
    }

    /// A picture is about to be drawn by the UI.
    pub(crate) fn painted(&mut self, frame_seq: u64, painted_at: Instant) {
        if !enabled() {
            return;
        }
        let Some(handoff) = self.handed.filter(|handoff| handoff.frame_seq == frame_seq) else {
            return;
        };
        self.handed = None;
        let line = format!("latency paint {}", paint_fields(&handoff, painted_at));
        self.emit(line);
    }
}

/// The fields of one action line, in the order they are written.
pub(crate) fn action_fields(
    action: &Action,
    frame_seq: u64,
    bytes: usize,
    size: (u32, u32),
    received_at: Instant,
    decoded_at: Instant,
    ack_before_decode: bool,
) -> String {
    let taken_at = action.taken_at.unwrap_or(action.queued_at);
    let written_at = action.written_at.unwrap_or(action.queued_at);
    let mut line = String::new();
    let _ = write!(
        line,
        "seq={} method={} queued_ms={} taken_ms={} written_ms={} received_ms={} decoded_ms={} handed_ms={} \
         frame={frame_seq} bytes={bytes} width={} height={} decode_ms={} ack={}",
        action.seq,
        action.method,
        millis(action.queued_at, action.queued_at),
        millis(action.queued_at, taken_at),
        millis(action.queued_at, written_at),
        millis(action.queued_at, received_at),
        millis(action.queued_at, decoded_at),
        millis(action.queued_at, decoded_at),
        size.0,
        size.1,
        millis(received_at, decoded_at),
        ack_placement(ack_before_decode)
    );
    line
}

/// The fields of one picture line, in the order they are written.
pub(crate) fn frame_fields(
    frame_seq: u64,
    bytes: usize,
    size: (u32, u32),
    received_at: Instant,
    decoded_at: Instant,
    ack_before_decode: bool,
) -> String {
    let mut line = String::new();
    let _ = write!(
        line,
        "seq={frame_seq} bytes={bytes} width={} height={} received_to_decoded_ms={} ack={}",
        size.0,
        size.1,
        millis(received_at, decoded_at),
        ack_placement(ack_before_decode)
    );
    line
}

/// The fields of one paint line, in the order they are written.
pub(crate) fn paint_fields(handoff: &Handoff, painted_at: Instant) -> String {
    let mut line = String::new();
    let action = handoff
        .action
        .map_or_else(|| "-".to_string(), |action| action.seq.to_string());
    let event_to_painted = handoff
        .action
        .map_or_else(|| f64::NAN.to_string(), |action| millis(action.queued_at, painted_at));
    let _ = write!(
        line,
        "seq={} action={action} handed_to_painted_ms={} event_to_painted_ms={event_to_painted}",
        handoff.frame_seq,
        millis(handoff.handed_at, painted_at)
    );
    line
}

/// The fields of one blocked driver pass, in the order they are written.
pub(crate) fn stall_fields(pass: Duration, waited: Option<(&str, Duration)>) -> String {
    let (method, waited_ms) = waited.map_or(("-".to_string(), f64::NAN), |(method, waited)| {
        (method.to_string(), millis_of(waited))
    });
    format!(
        "pass_ms={:.1} waited={method} waited_ms={waited_ms:.1}",
        millis_of(pass)
    )
}

/// One driver pass that blocked on the protocol.
pub(crate) fn stall(pass: Duration, waited: Option<(&str, Duration)>) {
    if !enabled() || pass <= STALL_THRESHOLD {
        return;
    }
    tracing::info!(
        target: "browser",
        "latency stall {}",
        stall_fields(pass, waited)
    );
}

fn millis(from: Instant, to: Instant) -> String {
    format!("{:.1}", millis_of(to.saturating_duration_since(from)))
}

fn millis_of(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn ack_placement(before_decode: bool) -> &'static str {
    if before_decode { "before_decode" } else { "after_decode" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action_at(started: Instant) -> Action {
        let mut action = Action::new(1);
        action.queued_at = started;
        action.taken_at = Some(started + Duration::from_millis(4));
        action.written_at = Some(started + Duration::from_millis(5));
        action.method = "Input.dispatchMouseEvent";
        action
    }

    /// The position of `field` in `line`, for the order assertions.
    fn at(line: &str, field: &str) -> usize {
        line.find(field)
            .unwrap_or_else(|| panic!("{field} missing from {line}"))
    }

    #[test]
    fn an_action_line_names_its_times_in_order() {
        let started = Instant::now();
        let action = action_at(started);
        let line = action_fields(
            &action,
            9,
            205_000,
            (1480, 880),
            started + Duration::from_millis(87),
            started + Duration::from_millis(131),
            true,
        );
        let order = [
            "seq=1 ",
            "method=Input.dispatchMouseEvent ",
            "queued_ms=0.0 ",
            "taken_ms=4.0 ",
            "written_ms=5.0 ",
            "received_ms=87.0 ",
            "decoded_ms=131.0 ",
            "handed_ms=131.0 ",
            "frame=9 ",
            "bytes=205000 ",
            "width=1480 ",
            "height=880 ",
            "decode_ms=44.0 ",
            "ack=before_decode",
        ];
        let mut cursor = 0;
        for field in order {
            let found = at(&line[cursor..], field);
            cursor += found;
        }
        assert_eq!(cursor + "ack=before_decode".len(), line.len(), "{line}");
    }

    #[test]
    fn a_picture_line_names_its_shape_and_the_ack_placement() {
        let received = Instant::now();
        let line = frame_fields(
            4,
            2048,
            (740, 440),
            received,
            received + Duration::from_millis(20),
            false,
        );
        assert!(line.contains("seq=4 "), "{line}");
        assert!(line.contains("bytes=2048 "), "{line}");
        assert!(line.contains("width=740 height=440 "), "{line}");
        assert!(line.contains("received_to_decoded_ms=20.0 "), "{line}");
        assert!(line.ends_with("ack=after_decode"), "{line}");
    }

    #[test]
    fn a_paint_line_says_when_the_handoff_became_a_pixel() {
        let started = Instant::now();
        let handoff = Handoff {
            frame_seq: 9,
            action: Some(action_at(started)),
            handed_at: started + Duration::from_millis(131),
        };
        let line = paint_fields(&handoff, started + Duration::from_millis(148));
        assert_eq!(
            line,
            "seq=9 action=1 handed_to_painted_ms=17.0 event_to_painted_ms=148.0"
        );
    }

    #[test]
    fn a_blocked_pass_names_the_command_it_waited_for() {
        let line = stall_fields(
            Duration::from_millis(180),
            Some(("Target.getTargetInfo", Duration::from_millis(175))),
        );
        assert_eq!(line, "pass_ms=180.0 waited=Target.getTargetInfo waited_ms=175.0");
    }

    #[test]
    fn one_action_produces_its_line_from_the_event_to_the_handed_picture() {
        force_for_tests(true);
        let mut probe = Probe::default();
        probe.queued();
        let action = probe.taken().expect("the driver takes the queued action");
        probe.written(action, "Input.dispatchMouseEvent");
        let received = Instant::now();
        probe.frame_handed(
            3,
            205_000,
            (1480, 880),
            received,
            received + Duration::from_millis(44),
            true,
        );
        probe.painted(3, Instant::now());
        let lines = probe.take_emitted();
        let action_line = lines
            .iter()
            .find(|line| line.starts_with("latency action "))
            .unwrap_or_else(|| panic!("no action line in {lines:?}"));
        let mut cursor = 0;
        for field in [
            "seq=1 ",
            "method=Input.dispatchMouseEvent ",
            "queued_ms=",
            "taken_ms=",
            "written_ms=",
            "received_ms=",
            "decoded_ms=",
            "handed_ms=4",
            "frame=3 ",
            "bytes=205000 ",
            "width=1480 height=880 ",
            "decode_ms=44.0 ",
            "ack=before_decode",
        ] {
            let at = action_line[cursor..]
                .find(field)
                .unwrap_or_else(|| panic!("{field} missing from {action_line}"));
            cursor += at;
        }
        assert!(lines.iter().any(|line| line.starts_with("latency frame ")), "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|line| line.starts_with("latency paint seq=3 action=1")),
            "{lines:?}"
        );
        force_for_tests(false);
    }

    #[test]
    fn a_disabled_probe_records_nothing_at_all() {
        if enabled() {
            return;
        }
        let mut probe = Probe::default();
        probe.queued();
        assert!(probe.taken().is_none(), "the switch is off: nothing is timed");
        assert_eq!(probe.next_seq, 0);
        probe.painted(1, Instant::now());
        assert!(probe.handed.is_none());
    }
}

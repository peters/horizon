//! Opt-in timing of one person's input, from the UI event to the painted pixel.
//!
//! `HORIZON_BROWSER_LATENCY=1` writes `tracing` INFO lines that follow one
//! action through the pipeline, so a slow far browser is measured instead of
//! guessed. The switch works in every build, including a release build without
//! the `trace-profiling` feature, and one relaxed atomic load is all it costs
//! while it is off. It is the switch of the key press of a terminal
//! (`HORIZON_TERMINAL_LATENCY`, in the core) in another place: the same rule for
//! what counts as on, the same target of the module that writes the line, the
//! same `_us` fields, and the window adds the target of the module to its filter
//! while the switch is on, so the switch is the only setting. See
//! `docs/latency-trace.md`.
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
        // The module path is the target, as it is for the line of a key press:
        // the window names it in its filter while this switch is on.
        tracing::info!("{line}");
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
        "seq={} method={} queued_us={} taken_us={} written_us={} received_us={} decoded_us={} handed_us={} \
         frame={frame_seq} bytes={bytes} width={} height={} decode_us={} ack={}",
        action.seq,
        action.method,
        micros(action.queued_at, action.queued_at),
        micros(action.queued_at, taken_at),
        micros(action.queued_at, written_at),
        micros(action.queued_at, received_at),
        micros(action.queued_at, decoded_at),
        micros(action.queued_at, decoded_at),
        size.0,
        size.1,
        micros(received_at, decoded_at),
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
        "seq={frame_seq} bytes={bytes} width={} height={} received_to_decoded_us={} ack={}",
        size.0,
        size.1,
        micros(received_at, decoded_at),
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
    let event_to_painted = handoff.action.map_or_else(
        || "-".to_string(),
        |action| micros(action.queued_at, painted_at).to_string(),
    );
    let _ = write!(
        line,
        "seq={} action={action} handed_to_painted_us={} event_to_painted_us={event_to_painted}",
        handoff.frame_seq,
        micros(handoff.handed_at, painted_at)
    );
    line
}

/// The fields of one blocked driver pass, in the order they are written.
pub(crate) fn stall_fields(pass: Duration, waited: Option<(&str, Duration)>) -> String {
    let (method, waited_us) = waited.map_or(("-".to_string(), "-".to_string()), |(method, waited)| {
        (method.to_string(), micros_of(waited).to_string())
    });
    format!("pass_us={} waited={method} waited_us={waited_us}", micros_of(pass))
}

/// A pass that blocked on the protocol says so, with the command it waited for.
pub(crate) fn report_blocked_pass(link: &mut crate::cdp::CdpLink, pass_started: Instant) {
    if !enabled() {
        return;
    }
    let waited = link.take_blocking_call();
    stall(
        pass_started.elapsed(),
        waited.as_ref().map(|(method, waited)| (method.as_str(), *waited)),
    );
}

/// One driver pass that blocked on the protocol.
pub(crate) fn stall(pass: Duration, waited: Option<(&str, Duration)>) {
    if !enabled() || pass <= STALL_THRESHOLD {
        return;
    }
    tracing::info!("latency stall {}", stall_fields(pass, waited));
}

/// One hop in microseconds, as the line of a key press writes it.
fn micros(from: Instant, to: Instant) -> u64 {
    micros_of(to.saturating_duration_since(from))
}

fn micros_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
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
            "queued_us=0 ",
            "taken_us=4000 ",
            "written_us=5000 ",
            "received_us=87000 ",
            "decoded_us=131000 ",
            "handed_us=131000 ",
            "frame=9 ",
            "bytes=205000 ",
            "width=1480 ",
            "height=880 ",
            "decode_us=44000 ",
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
        assert!(line.contains("received_to_decoded_us=20000 "), "{line}");
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
            "seq=9 action=1 handed_to_painted_us=17000 event_to_painted_us=148000"
        );
    }

    #[test]
    fn a_blocked_pass_names_the_command_it_waited_for() {
        let line = stall_fields(
            Duration::from_millis(180),
            Some(("Target.getTargetInfo", Duration::from_millis(175))),
        );
        assert_eq!(line, "pass_us=180000 waited=Target.getTargetInfo waited_us=175000");
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
            "queued_us=",
            "taken_us=",
            "written_us=",
            "received_us=",
            "decoded_us=",
            "handed_us=",
            "frame=3 ",
            "bytes=205000 ",
            "width=1480 height=880 ",
            "decode_us=",
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

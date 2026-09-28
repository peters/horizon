//! "Share local network": the owner's switch for one cloud's Local Network Bridge. It is held in
//! memory only, so it is never read from configuration or an agent and is off after a restart.
//! When the cloud disconnects, sharing pauses; it resumes by itself once that cloud has been
//! made ready again in this run of Horizon, until the owner switches it off.
use super::{Connection, HorizonApp, Runtime, Settings, Stage, cloud_runtime, lifecycle::Action};
use horizon_core::cloud_runtime::local_network::{BYTE_BUDGET, Bridge, Destination, Relay, State, Status};
use std::sync::{
    Mutex, OnceLock, PoisonError,
    mpsc::{Sender, channel},
};

#[derive(Default)]
pub(super) enum Sharing {
    #[default]
    Off,
    On(Running),
    /// The owner switched sharing on but the cloud left Connected+Ready; the bridge is stopped.
    /// `ready_again` is set once the cloud has completed readiness again.
    Paused {
        ready_again: bool,
    },
    /// Why the bridge could not start, shown until the owner tries again.
    Refused(String),
}

impl Sharing {
    fn stop(&mut self) {
        if matches!(self, Self::On(_)) {
            *self = Self::Off;
        }
    }

    /// The cloud completed readiness (after a reconnect, resume or rebuild).
    pub(super) fn ready_again(&mut self) {
        if let Self::Paused { ready_again } = self {
            *ready_again = true;
        }
    }

    /// Forgets earlier readiness, so only the readiness of the connection that follows counts.
    pub(super) fn await_ready(&mut self) {
        if let Self::Paused { ready_again } = self {
            *ready_again = false;
        }
    }

    fn step(&self, connected: bool) -> Step {
        match self {
            Self::On(_) if !connected => Step::Pause,
            // A reconnect that has not finished yet still looks connected and Ready.
            Self::Paused { ready_again: true } if connected => Step::Resume,
            _ => Step::Keep,
        }
    }

    fn intended(&self) -> bool {
        matches!(self, Self::On(_) | Self::Paused { .. })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    Keep,
    Pause,
    Resume,
}

/// A running bridge. However it is dropped (switched off, cloud disconnected, runtimes
/// cleared), its teardown, which waits for the SSH session to end, runs on a shutdown thread
/// and never on the UI thread.
pub(super) struct Running(Option<Bridge>);

impl Running {
    fn status(&self) -> Option<Status> {
        self.0.as_ref().map(Bridge::status)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(bridge) = self.0.take() {
            // Access ends here; only the wait for the SSH session moves off this thread.
            bridge.revoke();
            retire(bridge);
        }
    }
}

/// Hands `bridge` to one long-lived shutdown thread, started on first use. Only if that thread
/// cannot be started at all is the bridge dropped here.
fn retire(bridge: Bridge) {
    static SHUTDOWN: OnceLock<Option<Mutex<Sender<Bridge>>>> = OnceLock::new();
    let shutdown = SHUTDOWN.get_or_init(|| {
        let (sender, receiver) = channel::<Bridge>();
        std::thread::Builder::new()
            .name("local-network-stop".into())
            .spawn(move || receiver.into_iter().for_each(drop))
            .ok()
            .map(|_| Mutex::new(sender))
    });
    if let Some(sender) = shutdown {
        let sender = sender.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(returned) = sender.send(bridge) {
            drop(returned.0);
        }
    } else {
        drop(bridge);
    }
}

impl HorizonApp {
    /// A bridge lives only while this Horizon is connected to the Ready cloud: it pauses when the
    /// cloud disconnects and restarts once the cloud is ready again. Runs after failed
    /// operations dropped their receivers, so a disconnect in this frame counts.
    pub(super) fn reconcile_sharing(&mut self) {
        let resume: Vec<_> = self
            .cloud_prototype
            .production
            .runtimes
            .iter_mut()
            .filter_map(|(&id, runtime)| runtime.reconcile_sharing().then_some(id))
            .collect();
        for id in resume {
            self.share_local_network(id, true);
        }
    }
}

impl Runtime {
    /// Pauses a running bridge whose cloud left Connected+Ready, which revokes it at once, and
    /// says whether a paused one should restart now.
    fn reconcile_sharing(&mut self) -> bool {
        let connected = self.connected_and_ready();
        if !connected {
            // Readiness seen before this disconnect says nothing about the next connection.
            self.sharing.await_ready();
        }
        match self.sharing.step(connected) {
            Step::Pause => {
                self.sharing = Sharing::Paused { ready_again: false };
                false
            }
            Step::Resume => true,
            Step::Keep => false,
        }
    }

    fn connected_and_ready(&self) -> bool {
        self.stage == Some(Stage::Ready) && self.receiver.is_some()
    }
}

impl HorizonApp {
    pub(super) fn share_local_network(&mut self, id: u32, share: bool) {
        let connection = share.then(|| self.cloud_connection(id));
        let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
            return;
        };
        runtime.sharing.stop();
        runtime.sharing = match connection {
            None => Sharing::Off,
            Some(Err(error)) => Sharing::Refused(error.to_string()),
            Some(Ok(connection)) => match Bridge::start(&connection) {
                Ok(bridge) => Sharing::On(Running(Some(bridge))),
                Err(error) => Sharing::Refused(error.to_string()),
            },
        };
    }

    fn cloud_connection(&self, id: u32) -> cloud_runtime::Result<Connection> {
        let disconnected = cloud_runtime::Error::Invalid("Cloud is disconnected");
        let launch = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .and_then(|group| group.remote.as_ref())
            .ok_or(cloud_runtime::Error::Invalid("Missing cloud"))?;
        let root = self.cloud_prototype.root.as_ref().ok_or(disconnected)?;
        let worker = self
            .cloud_prototype
            .production
            .runtimes
            .get(&id)
            .and_then(|runtime| runtime.state.as_ref())
            .and_then(|state| state.worker.as_ref())
            .ok_or(cloud_runtime::Error::Invalid("Cloud is disconnected"))?;
        let settings = Settings::load(&root.join("settings.json"))?;
        Connection::new(
            worker,
            &settings,
            &cloud_runtime::state::cloud_directory(root, &launch.id)?,
        )
    }
}

/// The switch and, while it is on, what the bridge is doing.
pub(super) fn show(ui: &mut egui::Ui, runtime: &Runtime) -> Option<Action> {
    // A disconnected card offers Reconnect instead; a paused switch stays so it can be turned off.
    let paused = matches!(runtime.sharing, Sharing::Paused { .. });
    if !runtime.connected_and_ready() && !paused {
        return None;
    }
    let mut sharing = runtime.sharing.intended();
    let changed = ui
        .checkbox(&mut sharing, "Share local network")
        .on_hover_text(
            "Lets this cloud's agents reach devices on the network this computer is on, over TCP, \
             relayed through this computer. Only you can turn it on; it is off again after Horizon \
             restarts. While it is on, every process on the worker can use it.",
        )
        .changed();
    let status = match &runtime.sharing {
        Sharing::On(bridge) => {
            // The state, counters and connections change without other repaints.
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            bridge.status()
        }
        _ => None,
    };
    let line = match &runtime.sharing {
        Sharing::Off => None,
        Sharing::Refused(error) => Some(error.clone()),
        Sharing::Paused { .. } => Some(PAUSED.to_owned()),
        Sharing::On(_) => status.as_ref().map(describe),
    };
    if let Some(line) = line {
        ui.small(line);
    }
    if let Some(status) = status.filter(|status| matches!(status.state, State::Active { .. })) {
        let _ = connections(ui, &status.relays, std::time::Instant::now());
    }
    changed.then_some(if sharing {
        Action::ShareLocalNetwork
    } else {
        Action::StopSharingLocalNetwork
    })
}

const PAUSED: &str = "Sharing paused: cloud disconnected. It resumes when the cloud is connected again.";

fn describe(status: &Status) -> String {
    match &status.state {
        State::Starting => format!("Connecting to share {}…", status.subnet),
        State::Active { .. } if status.counters.bytes >= BYTE_BUDGET => {
            "Data limit reached; switch sharing off and on to continue".to_owned()
        }
        State::Active { .. } => format!(
            "Sharing {} · {} open · {}",
            status.subnet,
            status.counters.connections,
            amount(status.counters.bytes)
        ),
        State::Reconnecting { error } => format!("Reconnecting: {error}"),
        State::Failed { error } => error.clone(),
    }
}

/// Rows the open connection list keeps room for before it scrolls.
const LIST_ROWS: u8 = 6;

/// The open connections, newest first, collapsed by default. The header stays while the bridge
/// is active and the open list keeps one height, so connections that start and end never move
/// the controls below it. Returns where the header is.
fn connections(ui: &mut egui::Ui, relays: &[Relay], now: std::time::Instant) -> egui::Rect {
    egui::CollapsingHeader::new(format!("Open connections ({})", relays.len()))
        .id_salt("local-network-connections")
        .default_open(false)
        .show(ui, |ui| {
            let row = ui.text_style_height(&egui::TextStyle::Small) + ui.spacing().item_spacing.y;
            egui::ScrollArea::vertical()
                .id_salt("local-network-connection-list")
                .max_height(row * f32::from(LIST_ROWS))
                .show(ui, |ui| {
                    ui.set_min_height(row * f32::from(LIST_ROWS));
                    if relays.is_empty() {
                        ui.small("No connections open.");
                    }
                    for relay in relays.iter().rev() {
                        ui.small(connection(relay, now));
                    }
                });
        })
        .header_response
        .rect
}

/// One open connection: where it goes, what it relayed and for how long it has been open.
fn connection(relay: &Relay, now: std::time::Instant) -> String {
    let target = match &relay.requested {
        Destination::Name(name, port) => format!("{name}:{port} ({})", relay.address.ip()),
        Destination::Address(_) => relay.address.to_string(),
    };
    let open = now.saturating_duration_since(relay.opened).as_secs();
    let age = match open {
        0..60 => format!("{open} s"),
        60..3600 => format!("{} min", open / 60),
        _ => format!("{} h {} min", open / 3600, open % 3600 / 60),
    };
    format!("{target} · {} · {age}", amount(relay.bytes))
}

fn amount(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let (mut divisor, mut unit) = (1000_u64, 0);
    while bytes / divisor >= 1000 && unit + 1 < UNITS.len() {
        divisor *= 1000;
        unit += 1;
    }
    let tenths = bytes / (divisor / 10);
    format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_egui::DiscardTextures;
    use horizon_core::cloud_runtime::local_network::Counters;

    fn status(state: State, connections: usize, bytes: u64) -> Status {
        Status {
            subnet: "192.168.1.0/24".parse().unwrap(),
            state,
            counters: Counters {
                connections,
                bytes,
                refused: 0,
            },
            relays: Vec::new(),
        }
    }

    #[test]
    fn a_connection_names_its_destination_bytes_and_age() {
        let opened = std::time::Instant::now();
        let relay = |requested| Relay {
            requested,
            address: "192.168.1.216:80".parse().unwrap(),
            bytes: 12_400,
            opened,
        };
        let after = |seconds| opened + std::time::Duration::from_secs(seconds);
        assert_eq!(
            connection(
                &relay(Destination::Address("192.168.1.216:80".parse().unwrap())),
                after(42)
            ),
            "192.168.1.216:80 · 12.4 KB · 42 s"
        );
        let named = relay(Destination::Name("printer.local".into(), 80));
        assert_eq!(
            connection(&named, after(185)),
            "printer.local:80 (192.168.1.216) · 12.4 KB · 3 min"
        );
        assert_eq!(
            connection(&named, after(7_380)),
            "printer.local:80 (192.168.1.216) · 12.4 KB · 2 h 3 min"
        );
    }

    /// Where a control below the connection list lands, with `count` relays and the list
    /// collapsed or open, once the header animation has settled.
    fn control_below(count: usize, open: bool) -> f32 {
        let ctx = egui::Context::default();
        let now = std::time::Instant::now();
        let relays: Vec<_> = (0..count)
            .map(|index| Relay {
                requested: Destination::Name(format!("device-{index}.local"), 80),
                address: "192.168.1.216:80".parse().unwrap(),
                bytes: u64::try_from(index).unwrap(),
                opened: now,
            })
            .collect();
        let mut top = 0.0;
        let mut header = egui::Rect::NOTHING;
        for frame in 0..10 {
            // The owner opens the list by clicking its header, pressed and released in one frame.
            let events = if open && frame == 1 {
                [true, false]
                    .map(|pressed| egui::Event::PointerButton {
                        pos: header.center(),
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    })
                    .into()
            } else {
                Vec::new()
            };
            let input = egui::RawInput {
                time: Some(f64::from(frame)),
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let _ = ctx
                .run_ui(input, |ui| {
                    header = connections(ui, &relays, now);
                    top = ui.button("Delete worker").rect.top();
                })
                .discard_textures();
        }
        top
    }

    #[test]
    fn connections_starting_and_ending_never_move_the_controls_below_the_list() {
        for open in [false, true] {
            let empty = control_below(0, open);
            for count in [1, usize::from(LIST_ROWS) + 1, 20] {
                assert!(
                    (control_below(count, open) - empty).abs() < 0.5,
                    "{count} relays, open: {open}"
                );
            }
        }
        assert!(
            control_below(0, true) > control_below(0, false) + 20.0,
            "the open list takes its fixed height"
        );
    }

    #[test]
    fn the_card_says_what_the_bridge_is_doing() {
        let active = State::Active {
            proxy: "127.0.0.1:41234".parse().unwrap(),
        };
        assert_eq!(
            describe(&status(State::Starting, 0, 0)),
            "Connecting to share 192.168.1.0/24…"
        );
        assert_eq!(
            describe(&status(active.clone(), 2, 12_400_000)),
            "Sharing 192.168.1.0/24 · 2 open · 12.4 MB"
        );
        assert_eq!(
            describe(&status(active, 0, BYTE_BUDGET)),
            "Data limit reached; switch sharing off and on to continue"
        );
        assert_eq!(
            describe(&status(
                State::Reconnecting {
                    error: "Connection closed".into()
                },
                0,
                0
            )),
            "Reconnecting: Connection closed"
        );
        assert_eq!(
            describe(&status(
                State::Failed {
                    error: "Rebuild".into()
                },
                0,
                0
            )),
            "Rebuild"
        );
    }

    #[test]
    fn amounts_use_decimal_units() {
        assert_eq!(amount(0), "0 B");
        assert_eq!(amount(999), "999 B");
        assert_eq!(amount(1_500), "1.5 KB");
        assert_eq!(amount(64_000_000_000), "64.0 GB");
        assert_eq!(amount(u64::MAX), "18446744.0 TB");
    }

    #[test]
    fn sharing_pauses_on_disconnect_and_resumes_only_after_the_cloud_is_ready_again() {
        let mut sharing = Sharing::On(Running(None));
        assert_eq!(sharing.step(true), Step::Keep);
        assert_eq!(sharing.step(false), Step::Pause);
        sharing = Sharing::Paused { ready_again: false };
        assert!(sharing.intended());
        // Still disconnected, or reconnecting with only the old Ready stage: stay paused.
        assert_eq!(sharing.step(false), Step::Keep);
        assert_eq!(sharing.step(true), Step::Keep);
        sharing.ready_again();
        assert!(matches!(sharing, Sharing::Paused { ready_again: true }));
        assert_eq!(sharing.step(false), Step::Keep);
        assert_eq!(sharing.step(true), Step::Resume);
    }

    #[test]
    fn a_runtime_pauses_a_running_bridge_when_its_cloud_disconnects_and_resumes_after_ready() {
        let mut runtime = Runtime::default();
        let (_sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        runtime.stage = Some(Stage::Ready);
        runtime.sharing = Sharing::On(Running(None));
        assert!(!runtime.reconcile_sharing());
        assert!(matches!(runtime.sharing, Sharing::On(_)));
        // The presentation watch failed: the receiver is gone while the stage still says Ready.
        runtime.receiver = None;
        assert!(!runtime.reconcile_sharing());
        assert!(matches!(runtime.sharing, Sharing::Paused { ready_again: false }));
        let (_sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        assert!(!runtime.reconcile_sharing(), "a reconnect in progress does not resume");
        runtime.observe(&horizon_core::cloud_runtime::Event::Resumed);
        assert!(!runtime.reconcile_sharing());
        runtime.sharing.ready_again();
        assert!(runtime.reconcile_sharing());
        runtime.stage = Some(Stage::Stopped);
        assert!(!runtime.reconcile_sharing());
    }

    #[test]
    fn readiness_before_a_disconnect_never_resumes_the_next_reconnect_early() {
        let mut runtime = Runtime {
            stage: Some(Stage::Ready),
            sharing: Sharing::Paused { ready_again: false },
            ..Default::default()
        };
        // Ready arrives, then the presentation fails in the same frame: the receiver is gone.
        runtime.sharing.ready_again();
        assert!(!runtime.reconcile_sharing());
        assert!(matches!(runtime.sharing, Sharing::Paused { ready_again: false }));
        // A reconnect starts: a new receiver while the stage still says Ready.
        let (_sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        runtime.sharing.await_ready();
        assert!(!runtime.reconcile_sharing(), "resumed before the reconnect was ready");
        // That reconnect's own Ready resumes sharing.
        runtime.sharing.ready_again();
        assert!(runtime.reconcile_sharing());
    }

    #[test]
    fn switching_off_while_paused_clears_the_intent_and_nothing_else_starts_sharing() {
        // The switch's off action replaces a paused intent with Off.
        let mut sharing = Sharing::Off;
        assert!(!sharing.intended());
        sharing.ready_again();
        assert_eq!(sharing.step(true), Step::Keep);
        assert!(matches!(Runtime::default().sharing, Sharing::Off));
        let mut refused = Sharing::Refused("No network".into());
        refused.ready_again();
        refused.stop();
        assert!(matches!(refused, Sharing::Refused(_)));
        assert_eq!(refused.step(true), Step::Keep);
    }
}

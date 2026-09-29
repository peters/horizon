//! "Share local network": the owner's switch for one cloud's Local Network Bridge. It is held in
//! memory only, so it is never read from configuration or an agent and is off after a restart.
//! When the cloud disconnects or this computer sleeps, sharing pauses; it resumes by itself once
//! that cloud is ready again in this run of Horizon, but only on the network it started on. On
//! another network, such as a new Wi-Fi, sharing stops and the card asks the owner again.
use super::{Connection, HorizonApp, Runtime, Settings, Stage, cloud_runtime, lifecycle::Action};
mod editor;
mod watch;
pub(super) use editor::Editor;
use horizon_core::cloud_runtime::local_network::{
    BYTE_BUDGET, Bridge, Destination, Network, Relay, Rules, StartError, State, Status, Subnet,
};
use std::{
    sync::{
        Mutex, MutexGuard, OnceLock, PoisonError,
        mpsc::{Sender, channel},
    },
    time::{Duration, Instant},
};
use watch::Clock;
pub(super) use watch::Watch;

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
    /// This computer left the network sharing started on; the bridge is stopped and nothing
    /// resumes until the owner shares the network it is on now (`to`, when there is one). A
    /// start from here must find exactly that network.
    Moved {
        to: Option<Network>,
        /// As `Paused::ready_again`: set once the cloud is Ready on its current connection, so
        /// a reconnect in progress, which still looks Ready, does not offer the new network.
        ready: bool,
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
        if let Self::Paused { ready_again } | Self::Moved { ready: ready_again, .. } = self {
            *ready_again = true;
        }
    }

    /// Forgets earlier readiness, so only the readiness of the connection that follows counts.
    pub(super) fn await_ready(&mut self) {
        if let Self::Paused { ready_again } | Self::Moved { ready: ready_again, .. } = self {
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
        matches!(self, Self::On(_) | Self::Paused { .. } | Self::Moved { .. })
    }
}

/// Why a bridge did not start.
enum NotStarted {
    /// This computer is on another network than the one the start was approved for, or on none.
    Moved(Option<Network>),
    Refused(String),
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
pub(super) struct Running {
    bridge: Option<Bridge>,
    /// The bridge's last status and when it was read. A snapshot copies every open relay, so
    /// frames between refreshes reuse it instead of taking their own. Boxed, as it is most of
    /// a running bridge's size.
    status: Box<Mutex<Option<(Instant, Status)>>>,
}

/// How often the card reads the bridge's status; it repaints at the same cadence.
pub(super) const STATUS_REFRESH: Duration = Duration::from_secs(1);

impl Running {
    fn new(bridge: Option<Bridge>) -> Self {
        Self {
            bridge,
            status: Box::new(Mutex::new(None)),
        }
    }

    /// The bridge's status, read again only once the last reading is [`STATUS_REFRESH`] old.
    fn status(&self, now: Instant) -> MutexGuard<'_, Option<(Instant, Status)>> {
        let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(bridge) = &self.bridge
            && status
                .as_ref()
                .is_none_or(|(read, _)| now.saturating_duration_since(*read) >= STATUS_REFRESH)
        {
            *status = Some((now, bridge.status()));
        }
        status
    }
}

#[cfg(test)]
impl Running {
    /// A bridge still starting, as far as a reader of its status can tell.
    pub(super) fn starting() -> Self {
        Self::new(None)
    }
}

impl Running {
    /// Connections the bridge relays now, once it is sharing; `None` while it starts or reconnects.
    pub(super) fn open_connections(&self) -> Option<usize> {
        let status = self.status(Instant::now());
        match status.as_ref().map(|(_, status)| status) {
            Some(Status {
                state: State::Active { .. },
                relays,
                ..
            }) => Some(relays.len()),
            _ => None,
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(bridge) = self.bridge.take() {
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
    pub(super) fn reconcile_sharing(&mut self, ctx: &egui::Context) {
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
        // Sharing that is on, paused or waiting for the owner keeps watching the network even
        // when nothing else repaints, for example with its card hidden or the cloud idle.
        if self
            .cloud_prototype
            .production
            .runtimes
            .values()
            .any(|runtime| runtime.sharing.intended())
        {
            ctx.request_repaint_after(watch::NETWORK_CHECK);
        }
    }
}

impl Runtime {
    /// Stops any bridge, then starts one through `start` with the owner's scope when there is
    /// a connection. A bridge that resumes after a pause therefore starts no wider than the
    /// owner left it, while one switched on after being off starts with the whole network.
    /// Unless sharing is on afterwards, the scope is forgotten.
    fn start_sharing<T, E: std::fmt::Display>(
        &mut self,
        connection: Option<Result<T, E>>,
        start: impl FnOnce(&T, Rules, Option<&Network>) -> Result<Running, NotStarted>,
    ) {
        // A resume may only share the network sharing started on, and the owner's answer to a
        // move only the network offered; switching on anew shares whatever network this is.
        let expected = match &self.sharing {
            Sharing::Paused { .. } => self.shared.clone(),
            Sharing::Moved { to, .. } => to.clone(),
            Sharing::Off | Sharing::On(_) | Sharing::Refused(_) => None,
        };
        self.sharing.stop();
        self.sharing = match connection {
            None => Sharing::Off,
            Some(Err(error)) => Sharing::Refused(error.to_string()),
            Some(Ok(connection)) => match start(&connection, self.scope.applied.clone(), expected.as_ref()) {
                Ok(running) => Sharing::On(running),
                Err(NotStarted::Moved(now)) => {
                    self.moved(now);
                    return;
                }
                Err(NotStarted::Refused(error)) => Sharing::Refused(error),
            },
        };
        self.shared = match &self.sharing {
            Sharing::On(running) => running.bridge.as_ref().and_then(Bridge::network),
            _ => None,
        };
        if !matches!(self.sharing, Sharing::On(_)) {
            self.scope.reset();
        }
    }

    /// Pauses a running bridge whose cloud left Connected+Ready or whose computer slept, which
    /// revokes it at once, stops one whose computer moved to another network, and says whether
    /// a paused one should restart now.
    fn reconcile_sharing(&mut self) -> bool {
        self.reconcile_sharing_with(Clock::now(), watch::current)
    }

    /// As [`Self::reconcile_sharing`], with the clocks and the current network given.
    /// `current` is read only when the network is compared. When it cannot read the interfaces,
    /// which says nothing about a move, nothing changes and the next comparison tries again.
    fn reconcile_sharing_with(
        &mut self,
        now: Clock,
        current: impl Fn() -> Result<Option<Network>, StartError>,
    ) -> bool {
        let connected = self.connected_and_ready();
        if !connected {
            // Readiness seen before this disconnect says nothing about the next connection.
            self.sharing.await_ready();
        }
        // The clocks are read every frame, so a wake is measured from the last frame before it.
        let slept = self.watch.slept(now) && matches!(self.sharing, Sharing::On(_));
        let check = self.watch.check_due(now.monotonic);
        if slept {
            // Resumed below at once when the cloud is still ready, after the network check.
            self.sharing = Sharing::Paused { ready_again: connected };
        } else if check {
            match &self.sharing {
                // A paused bridge stops waiting as soon as this computer is elsewhere.
                Sharing::On(_) | Sharing::Paused { .. } => {
                    if let Ok(on) = current()
                        && !watch::same_network(self.shared.as_ref(), on.as_ref())
                    {
                        self.moved(on);
                        return false;
                    }
                }
                // The offer always names the network this computer is on now.
                Sharing::Moved { to, ready } => {
                    if let Ok(on) = current()
                        && on != *to
                    {
                        self.sharing = Sharing::Moved { to: on, ready: *ready };
                    }
                }
                Sharing::Off | Sharing::Refused(_) => {}
            }
        }
        match self.sharing.step(connected) {
            Step::Pause => {
                self.sharing = Sharing::Paused { ready_again: false };
                false
            }
            Step::Resume => match current() {
                Ok(on) if watch::same_network(self.shared.as_ref(), on.as_ref()) => true,
                Ok(on) => {
                    self.moved(on);
                    false
                }
                // Keeps waiting, and tries again on the next frame.
                Err(_) => false,
            },
            Step::Keep => false,
        }
    }

    /// Stops sharing because this computer is no longer on the network it shared. The scope
    /// belonged to that network, so it is forgotten; the owner decides whether to share `to`.
    fn moved(&mut self, to: Option<Network>) {
        // The cloud is as ready as it was: a running bridge's cloud is, a paused one's only once
        // it completed readiness on its new connection.
        let ready = self.connected_and_ready()
            && match self.sharing {
                Sharing::On(_) => true,
                Sharing::Paused { ready_again } | Sharing::Moved { ready: ready_again, .. } => ready_again,
                Sharing::Off | Sharing::Refused(_) => false,
            };
        self.sharing = Sharing::Moved { to, ready };
        self.scope.reset();
        self.shared = None;
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
        runtime.start_sharing(connection, |connection, rules, expected| {
            Bridge::start_with(connection, rules, expected)
                .map(|bridge| Running::new(Some(bridge)))
                .map_err(|error| match error {
                    StartError::Moved(now) => NotStarted::Moved(now),
                    error => NotStarted::Refused(error.to_string()),
                })
        });
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
pub(super) fn show(ui: &mut egui::Ui, runtime: &mut Runtime) -> Option<Action> {
    // A disconnected card offers Reconnect instead; a paused switch stays so it can be turned off.
    let paused = matches!(runtime.sharing, Sharing::Paused { .. } | Sharing::Moved { .. });
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
    let now = Instant::now();
    let reading = match &runtime.sharing {
        Sharing::On(bridge) => {
            // The state, counters and connections change without other repaints.
            ui.ctx().request_repaint_after(STATUS_REFRESH);
            Some(bridge.status(now))
        }
        _ => None,
    };
    let status = reading
        .as_ref()
        .and_then(|reading| reading.as_ref())
        .map(|(_, status)| status);
    let line = match &runtime.sharing {
        Sharing::Off => None,
        Sharing::Refused(error) => Some(error.clone()),
        Sharing::Paused { .. } => Some(PAUSED.to_owned()),
        Sharing::Moved { to, .. } => Some(moved(to.as_ref().map(Network::subnet))),
        Sharing::On(_) => status.map(describe),
    };
    if let Some(line) = line {
        ui.small(line);
    }
    if let Some(status) = status.filter(|status| matches!(status.state, State::Active { .. })) {
        let _ = connections(ui, &status.relays, now);
        if let Some(rules) = runtime.scope.show(ui)
            && let Sharing::On(running) = &runtime.sharing
            && let Some(bridge) = &running.bridge
        {
            let applied = bridge.set_rules(rules.clone()).map_err(|error| error.to_string());
            runtime.scope.outcome(rules, applied);
        }
    }
    // Sharing a new network is the owner's decision, made here, never by resuming.
    if let Some(subnet) = move_offer(runtime)
        && ui.button(format!("Share {subnet}")).clicked()
    {
        return Some(Action::ShareLocalNetwork);
    }
    changed.then_some(if sharing {
        Action::ShareLocalNetwork
    } else {
        Action::StopSharingLocalNetwork
    })
}

const PAUSED: &str = "Sharing paused: the cloud disconnected or this computer slept. It resumes when the cloud is \
     connected again, if this computer is still on the same network.";

/// The network the card offers to share after a move: only once the cloud is connected and
/// has completed readiness on that connection, since a bridge needs it. The switch stays either way, to stop sharing.
fn move_offer(runtime: &Runtime) -> Option<Subnet> {
    match &runtime.sharing {
        Sharing::Moved {
            to: Some(network),
            ready: true,
        } if runtime.connected_and_ready() => Some(network.subnet()),
        _ => None,
    }
}

/// Why sharing stopped after a move to another network, and what the owner can do.
fn moved(to: Option<Subnet>) -> String {
    match to {
        Some(subnet) => format!(
            "Sharing stopped: this computer moved to another network ({subnet}). Nothing is shared until you share it."
        ),
        None => "Sharing stopped: this computer left the shared network and is on none it can share.".to_owned(),
    }
}

fn describe(status: &Status) -> String {
    match &status.state {
        State::Starting => format!("Connecting to share {}…", status.subnet),
        State::Active { .. } if status.counters.bytes >= BYTE_BUDGET => {
            "Data limit reached; switch sharing off and on to continue".to_owned()
        }
        State::Active { .. } => format!(
            "Sharing {} · {} open · {}",
            status.subnet,
            // The relays the list below shows, not the proxy's count, which also includes
            // connections still negotiating, so the two never disagree.
            status.relays.len(),
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
            // A box of fixed size that the list fills: inside a scrolling card, a list sized by
            // its content would shrink to whatever space is left in view.
            let size = egui::vec2(ui.available_width(), row * f32::from(LIST_ROWS));
            ui.allocate_ui(size, |ui| {
                ui.set_min_size(size);
                egui::ScrollArea::vertical()
                    .id_salt("local-network-connection-list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if relays.is_empty() {
                            ui.small("No connections open.");
                        }
                        for relay in relays.iter().rev() {
                            ui.small(connection(relay, now));
                        }
                    });
            });
        })
        .header_response
        .rect
}

/// One open connection: where it goes, what it relayed and for how long it has been open.
fn connection(relay: &Relay, now: std::time::Instant) -> String {
    let target = match &relay.requested {
        Destination::Name(name, port) => format!("{name}:{port} ({})", relay.address.ip()),
        Destination::Address(requested) if *requested == relay.address => requested.to_string(),
        // The scope dials an IPv4-mapped address as the IPv4 address it carries.
        Destination::Address(requested) => format!("{requested} ({})", relay.address.ip()),
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

    /// A bridge relaying `connections` connections, with one more still negotiating.
    fn status(state: State, connections: usize, bytes: u64) -> Status {
        let address: std::net::SocketAddr = "192.168.1.50:80".parse().unwrap();
        Status {
            subnet: "192.168.1.0/24".parse().unwrap(),
            state,
            counters: Counters {
                connections: connections + 1,
                bytes,
                refused: 0,
            },
            relays: vec![
                Relay {
                    requested: Destination::Address(address),
                    address,
                    bytes: 0,
                    opened: std::time::Instant::now(),
                };
                connections
            ],
        }
    }

    #[test]
    fn a_connection_names_its_destination_bytes_and_age() {
        let opened = std::time::Instant::now();
        let relay = |requested| Relay {
            requested,
            address: "192.168.1.50:80".parse().unwrap(),
            bytes: 12_400,
            opened,
        };
        let after = |seconds| opened + std::time::Duration::from_secs(seconds);
        assert_eq!(
            connection(
                &relay(Destination::Address("192.168.1.50:80".parse().unwrap())),
                after(42)
            ),
            "192.168.1.50:80 · 12.4 KB · 42 s"
        );
        assert_eq!(
            connection(
                &relay(Destination::Address("[::ffff:192.168.1.50]:80".parse().unwrap())),
                after(42)
            ),
            "[::ffff:192.168.1.50]:80 (192.168.1.50) · 12.4 KB · 42 s",
            "the address the worker asked for, then the one dialled"
        );
        let named = relay(Destination::Name("printer.local".into(), 80));
        assert_eq!(
            connection(&named, after(185)),
            "printer.local:80 (192.168.1.50) · 12.4 KB · 3 min"
        );
        assert_eq!(
            connection(&named, after(7_380)),
            "printer.local:80 (192.168.1.50) · 12.4 KB · 2 h 3 min"
        );
    }

    /// A scrolling card with little room left in view below the list, as in the Manage window,
    /// and the control that must not move below it.
    struct Card {
        ctx: egui::Context,
        frame: u32,
        header: egui::Rect,
    }

    /// Where the control below the list is, and whether the empty-list line is in view.
    struct Frame {
        below: f32,
        empty_line_visible: bool,
        /// One list row, in this style.
        row: f32,
    }

    impl Card {
        fn new() -> Self {
            Self {
                ctx: egui::Context::default(),
                frame: 0,
                header: egui::Rect::NOTHING,
            }
        }

        /// Renders frames with `count` relays until the header animation settles; `click`
        /// presses and releases on the header in the first of them.
        fn show(&mut self, count: usize, click: bool) -> Frame {
            let now = std::time::Instant::now();
            let relays: Vec<_> = (0..count)
                .map(|index| Relay {
                    requested: Destination::Name(format!("device-{index}.local"), 80),
                    address: "192.168.1.50:80".parse().unwrap(),
                    bytes: u64::try_from(index).unwrap(),
                    opened: now,
                })
                .collect();
            let mut result = Frame {
                below: 0.0,
                empty_line_visible: false,
                row: 0.0,
            };
            for step in 0..10 {
                self.frame += 1;
                let events = if click && step == 0 {
                    [true, false]
                        .map(|pressed| egui::Event::PointerButton {
                            pos: self.header.center(),
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        })
                        .into()
                } else {
                    Vec::new()
                };
                let input = egui::RawInput {
                    time: Some(f64::from(self.frame)),
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))),
                    events,
                    ..Default::default()
                };
                let mut header = self.header;
                let output = self
                    .ctx
                    .run_ui(input, |ui| {
                        result.row = ui.text_style_height(&egui::TextStyle::Small) + ui.spacing().item_spacing.y;
                        egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                            ui.add_space(60.0);
                            header = connections(ui, &relays, now);
                            result.below = ui.button("Delete cloud resources…").rect.top();
                        });
                    })
                    .discard_textures();
                self.header = header;
                result.empty_line_visible = output.shapes.iter().any(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "No connections open." => {
                        clipped.clip_rect.contains(text.visual_bounding_rect().center())
                    }
                    _ => false,
                });
            }
            result
        }
    }

    #[test]
    fn connections_starting_and_ending_never_move_the_controls_below_the_list() {
        for open in [false, true] {
            let mut card = Card::new();
            card.show(0, false);
            let empty = card.show(0, open).below;
            for count in [1, usize::from(LIST_ROWS) + 1, 20, 0] {
                assert!(
                    (card.show(count, false).below - empty).abs() < 0.5,
                    "{count} relays, open: {open}"
                );
            }
        }
    }

    #[test]
    fn the_open_list_keeps_room_for_its_rows_in_a_scrolling_card() {
        let mut card = Card::new();
        let collapsed = card.show(0, false).below;
        let opened = card.show(0, true);
        assert!(
            opened.below - collapsed >= opened.row * f32::from(LIST_ROWS),
            "the list keeps six rows although little of the card is in view"
        );
        assert!(opened.empty_line_visible);
        card.show(20, false);
        assert!(
            card.show(0, false).empty_line_visible,
            "a list that empties after scrolling shows its empty line"
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
        let mut sharing = Sharing::On(Running::new(None));
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
        runtime.sharing = Sharing::On(Running::new(None));
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

    /// A narrowed scope, as the owner applied it on the card.
    fn narrowed() -> Rules {
        Rules {
            devices: vec![horizon_core::cloud_runtime::local_network::Device {
                address: std::net::Ipv4Addr::new(192, 168, 1, 50),
                ports: vec![554],
            }],
            local_ports: vec![3000],
        }
    }

    #[test]
    fn a_resumed_bridge_starts_with_the_owners_scope_and_a_failed_resume_forgets_it() {
        let mut runtime = Runtime::default();
        runtime.scope.outcome(narrowed(), Ok(()));
        runtime.sharing = Sharing::Paused { ready_again: true };
        let mut given = None;
        runtime.start_sharing(Some(Ok::<_, String>(())), |(), rules, _| {
            given = Some(rules);
            Ok(Running::new(None))
        });
        assert_eq!(given, Some(narrowed()), "the resumed bridge is narrowed from the start");
        assert!(matches!(runtime.sharing, Sharing::On(_)));
        assert_eq!(runtime.scope.applied, narrowed(), "the scope outlasts the resume");
        // The network changed while paused: the saved scope no longer fits, so sharing is
        // refused rather than started wider, and the scope is forgotten.
        runtime.sharing = Sharing::Paused { ready_again: true };
        runtime.start_sharing(Some(Ok::<_, String>(())), |(), rules, _| {
            assert_eq!(rules, narrowed());
            Err(NotStarted::Refused(
                "The saved scope does not fit the current network".into(),
            ))
        });
        assert!(matches!(&runtime.sharing, Sharing::Refused(why) if why.contains("does not fit")));
        assert_eq!(runtime.scope.applied, Rules::default());
    }

    #[test]
    fn switching_sharing_off_or_losing_the_connection_forgets_the_scope() {
        for connection in [None, Some(Err("Cloud is disconnected".to_owned()))] {
            let mut runtime = Runtime::default();
            runtime.scope.outcome(narrowed(), Ok(()));
            runtime.sharing = Sharing::On(Running::new(None));
            runtime.start_sharing(connection, |(), _, _| panic!("nothing starts without a connection"));
            assert!(!matches!(runtime.sharing, Sharing::On(_)));
            assert_eq!(runtime.scope.applied, Rules::default());
        }
        let mut runtime = Runtime::default();
        let mut given = None;
        runtime.start_sharing(Some(Ok::<_, String>(())), |(), rules, _| {
            given = Some(rules);
            Ok(Running::new(None))
        });
        assert_eq!(given, Some(Rules::default()), "switched on anew, the whole network");
    }

    fn home() -> Network {
        Network::new(
            "192.168.1.0/24".parse().unwrap(),
            "192.168.1.20".parse().unwrap(),
            "wlan0",
        )
    }

    fn office() -> Network {
        Network::new("10.0.0.0/24".parse().unwrap(), "10.0.0.7".parse().unwrap(), "wlan0")
    }

    /// A runtime whose cloud is connected and Ready, sharing `home`.
    fn sharing_home() -> (Runtime, std::sync::mpsc::Sender<cloud_runtime::Event>) {
        let mut runtime = Runtime::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        runtime.receiver = Some(receiver);
        runtime.stage = Some(Stage::Ready);
        runtime.sharing = Sharing::On(Running::new(None));
        runtime.shared = Some(home());
        runtime.scope.outcome(narrowed(), Ok(()));
        (runtime, sender)
    }

    /// The clocks `seconds` from now, with the wall clock `slept` seconds further ahead.
    fn later(seconds: u64, slept: u64) -> Clock {
        Clock {
            monotonic: Instant::now() + Duration::from_secs(seconds),
            wall: std::time::SystemTime::now() + Duration::from_secs(seconds + slept),
        }
    }

    #[test]
    fn a_computer_that_moved_to_another_network_stops_sharing_and_forgets_its_scope() {
        let (mut runtime, _sender) = sharing_home();
        assert!(!runtime.reconcile_sharing_with(later(3, 0), || Ok(Some(home()))));
        assert!(matches!(runtime.sharing, Sharing::On(_)), "still on the shared network");
        assert!(!runtime.reconcile_sharing_with(later(6, 0), || Ok(Some(office()))));
        assert!(matches!(&runtime.sharing, Sharing::Moved { to: Some(to), .. } if *to == office()));
        assert_eq!(
            (runtime.scope.applied.clone(), runtime.shared.clone()),
            (Rules::default(), None)
        );
        // Nothing resumes by itself on the new network.
        assert!(!runtime.reconcile_sharing_with(later(9, 0), || Ok(Some(office()))));
        assert!(matches!(runtime.sharing, Sharing::Moved { .. }));
        // Leaving every network stops sharing too.
        let (mut runtime, _sender) = sharing_home();
        assert!(!runtime.reconcile_sharing_with(later(3, 0), || Ok(None)));
        assert!(matches!(runtime.sharing, Sharing::Moved { to: None, .. }));
    }

    #[test]
    fn sharing_resumes_after_sleep_only_on_the_network_it_started_on() {
        let (mut runtime, _sender) = sharing_home();
        assert!(
            runtime.reconcile_sharing_with(later(1, 600), || Ok(Some(home()))),
            "ten minutes asleep, same network: the bridge restarts"
        );
        assert!(matches!(runtime.sharing, Sharing::Paused { ready_again: true }));
        assert_eq!(runtime.scope.applied, narrowed(), "and keeps the owner's scope");
        let (mut runtime, _sender) = sharing_home();
        assert!(!runtime.reconcile_sharing_with(later(1, 600), || Ok(Some(office()))));
        assert!(
            matches!(runtime.sharing, Sharing::Moved { .. }),
            "woke on another network"
        );
    }

    #[test]
    fn a_paused_bridge_stops_waiting_when_this_computer_moves() {
        let (mut runtime, _sender) = sharing_home();
        runtime.receiver = None;
        runtime.sharing = Sharing::Paused { ready_again: false };
        assert!(!runtime.reconcile_sharing_with(later(1, 0), || Ok(Some(office()))));
        assert!(
            matches!(runtime.sharing, Sharing::Paused { .. }),
            "checked every few seconds"
        );
        assert!(!runtime.reconcile_sharing_with(later(3, 0), || Ok(Some(office()))));
        assert!(
            matches!(&runtime.sharing, Sharing::Moved { to: Some(to), .. } if *to == office()),
            "moved while still disconnected"
        );
    }

    #[test]
    fn the_move_offer_follows_this_computer_and_waits_for_a_ready_cloud() {
        let (mut runtime, _sender) = sharing_home();
        assert!(!runtime.reconcile_sharing_with(later(3, 0), || Ok(Some(office()))));
        assert_eq!(move_offer(&runtime), Some(office().subnet()));
        let cafe = Network::new("172.20.0.0/24".parse().unwrap(), "172.20.0.9".parse().unwrap(), "wlan0");
        assert!(!runtime.reconcile_sharing_with(later(6, 0), || Ok(Some(cafe.clone()))));
        assert_eq!(move_offer(&runtime), Some(cafe.subnet()), "moved again before sharing");
        assert!(!runtime.reconcile_sharing_with(later(9, 0), || Ok(None)));
        assert_eq!(move_offer(&runtime), None, "no network to offer");
        assert!(matches!(runtime.sharing, Sharing::Moved { to: None, .. }));
        assert!(!runtime.reconcile_sharing_with(later(12, 0), || Ok(Some(cafe.clone()))));
        runtime.receiver = None;
        assert_eq!(
            move_offer(&runtime),
            None,
            "not offered while the cloud is disconnected"
        );
    }

    #[test]
    fn every_start_is_bound_to_the_network_it_was_approved_for() {
        // A resume expects the network sharing started on.
        let (mut runtime, _sender) = sharing_home();
        runtime.sharing = Sharing::Paused { ready_again: true };
        let mut expected = None;
        runtime.start_sharing(Some(Ok::<_, String>(())), |(), _, network| {
            expected = network.cloned();
            Ok(Running::new(None))
        });
        assert_eq!(expected, Some(home()));
        // The owner's answer to a move expects exactly the network offered; finding another
        // one at start asks again about that one instead of sharing it.
        runtime.sharing = Sharing::Moved {
            to: Some(office()),
            ready: true,
        };
        let cafe = Network::new("172.20.0.0/24".parse().unwrap(), "172.20.0.9".parse().unwrap(), "wlan0");
        runtime.start_sharing(Some(Ok::<_, String>(())), |(), _, network| {
            assert_eq!(network, Some(&office()));
            Err(NotStarted::Moved(Some(cafe.clone())))
        });
        assert!(matches!(&runtime.sharing, Sharing::Moved { to: Some(to), .. } if *to == cafe));
        assert_eq!(
            move_offer(&runtime),
            Some(cafe.subnet()),
            "still ready, so asked at once"
        );
        // A resume that finds no network at all is a move too, not a refusal.
        runtime.sharing = Sharing::Paused { ready_again: true };
        runtime.shared = Some(home());
        runtime.start_sharing(Some(Ok::<_, String>(())), |(), _, _| Err(NotStarted::Moved(None)));
        assert!(matches!(runtime.sharing, Sharing::Moved { to: None, .. }));
        // Switching on anew shares whatever network this is.
        runtime.sharing = Sharing::Off;
        runtime.start_sharing(Some(Ok::<_, String>(())), |(), _, network| {
            assert_eq!(network, None);
            Ok(Running::new(None))
        });
    }

    #[test]
    fn a_move_during_a_reconnect_is_offered_only_once_that_reconnect_is_ready() {
        // A reconnect in progress still looks connected and Ready.
        let (mut runtime, _sender) = sharing_home();
        runtime.sharing = Sharing::Paused { ready_again: false };
        assert!(!runtime.reconcile_sharing_with(later(3, 0), || Ok(Some(office()))));
        assert!(matches!(runtime.sharing, Sharing::Moved { ready: false, .. }));
        assert_eq!(move_offer(&runtime), None, "offered before the reconnect was ready");
        runtime.sharing.ready_again();
        assert_eq!(move_offer(&runtime), Some(office().subnet()));
        // Another reconnect forgets that readiness until its own Ready.
        runtime.sharing.await_ready();
        assert_eq!(move_offer(&runtime), None);
        runtime.sharing.ready_again();
        assert!(!runtime.reconcile_sharing_with(later(6, 0), || Ok(Some(office()))));
        assert_eq!(move_offer(&runtime), Some(office().subnet()));
        // A disconnect forgets it as well.
        runtime.receiver = None;
        assert!(!runtime.reconcile_sharing_with(later(9, 0), || Ok(Some(office()))));
        assert!(matches!(runtime.sharing, Sharing::Moved { ready: false, .. }));
    }

    #[test]
    fn unreadable_interfaces_are_not_a_move() {
        let unreadable = || Err(StartError::Io(std::io::Error::other("unreadable")));
        let (mut runtime, _sender) = sharing_home();
        assert!(!runtime.reconcile_sharing_with(later(3, 0), unreadable));
        assert!(
            matches!(runtime.sharing, Sharing::On(_)),
            "a running bridge keeps running"
        );
        assert_eq!(
            (runtime.scope.applied.clone(), runtime.shared.clone()),
            (narrowed(), Some(home()))
        );
        // A resume waits for a readable network instead of stopping or starting blind.
        runtime.sharing = Sharing::Paused { ready_again: true };
        assert!(!runtime.reconcile_sharing_with(later(4, 0), unreadable));
        assert!(matches!(runtime.sharing, Sharing::Paused { ready_again: true }));
        assert!(runtime.reconcile_sharing_with(later(5, 0), || Ok(Some(home()))));
        // The offer after a move keeps naming the network it last saw.
        runtime.moved(Some(office()));
        assert!(!runtime.reconcile_sharing_with(later(9, 0), unreadable));
        assert!(matches!(&runtime.sharing, Sharing::Moved { to: Some(to), .. } if *to == office()));
    }

    #[test]
    fn a_reconnect_on_another_network_asks_instead_of_resuming() {
        let (mut runtime, _sender) = sharing_home();
        runtime.sharing = Sharing::Paused { ready_again: true };
        assert!(!runtime.reconcile_sharing_with(later(1, 0), || Ok(Some(office()))));
        assert!(matches!(runtime.sharing, Sharing::Moved { .. }));
        let (mut runtime, _sender) = sharing_home();
        runtime.sharing = Sharing::Paused { ready_again: true };
        assert!(runtime.reconcile_sharing_with(later(1, 0), || Ok(Some(home()))));
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

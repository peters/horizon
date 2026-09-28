//! "Share local network": the owner's switch for one cloud's Local Network Bridge. It is held in
//! memory only, so it is never read from configuration or an agent and is off after a restart.
use super::{Connection, HorizonApp, Runtime, Settings, Stage, cloud_runtime, lifecycle::Action};
use horizon_core::cloud_runtime::local_network::{BYTE_BUDGET, Bridge, State, Status};
use std::sync::{
    Mutex, OnceLock, PoisonError,
    mpsc::{Sender, channel},
};

#[derive(Default)]
pub(super) enum Sharing {
    #[default]
    Off,
    On(Running),
    /// Why the bridge could not start, shown until the owner tries again.
    Refused(String),
}

impl Sharing {
    fn stop(&mut self) {
        if matches!(self, Self::On(_)) {
            *self = Self::Off;
        }
    }
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
    /// A bridge lives only while this Horizon is connected to the Ready cloud. Runs after failed
    /// operations dropped their receivers, so a disconnect in this frame counts.
    pub(super) fn stop_disconnected_sharing(&mut self) {
        for runtime in self.cloud_prototype.production.runtimes.values_mut() {
            runtime.stop_sharing_when_disconnected();
        }
    }
}

impl Runtime {
    fn stop_sharing_when_disconnected(&mut self) {
        if !self.connected_and_ready() {
            self.sharing.stop();
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
    // A disconnected card still shows Ready; it offers Reconnect instead.
    if !runtime.connected_and_ready() {
        return None;
    }
    let mut sharing = matches!(runtime.sharing, Sharing::On(_));
    let changed = ui
        .checkbox(&mut sharing, "Share local network")
        .on_hover_text(
            "Lets this cloud's agents reach devices on the network this computer is on, over TCP, \
             relayed through this computer. Only you can turn it on; it is off again after Horizon \
             restarts. While it is on, every process on the worker can use it.",
        )
        .changed();
    let line = match &runtime.sharing {
        Sharing::Off => None,
        Sharing::Refused(error) => Some(error.clone()),
        Sharing::On(bridge) => {
            // The state and counters change without other repaints.
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
            bridge.status().as_ref().map(describe)
        }
    };
    if let Some(line) = line {
        ui.small(line);
    }
    changed.then_some(if sharing {
        Action::ShareLocalNetwork
    } else {
        Action::StopSharingLocalNetwork
    })
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
            status.counters.connections,
            amount(status.counters.bytes)
        ),
        State::Reconnecting { error } => format!("Reconnecting: {error}"),
        State::Failed { error } => error.clone(),
    }
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
        }
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
    fn sharing_starts_off_and_stops_when_the_cloud_is_not_connected_and_ready() {
        let mut runtime = Runtime::default();
        assert!(matches!(runtime.sharing, Sharing::Off));
        runtime.sharing = Sharing::Refused("No network".into());
        runtime.stop_sharing_when_disconnected();
        // A refusal stays visible; only a running bridge is stopped.
        assert!(matches!(runtime.sharing, Sharing::Refused(_)));
    }
}

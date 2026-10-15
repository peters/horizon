//! Agent stops of ready clouds whose worker cannot stop itself (Hetzner, a Docker host).
//! Once such a cloud is ready, Horizon reads the worker's stop request every 30
//! seconds; a request the worker took becomes a Stop of the cloud for that agent, which
//! the card shows as any Stop. Reading skips a cloud that another operation holds or that
//! is not ready, and goes on after a failure the card shows, so a lost connection does
//! not lose the request. Only a running Horizon does this.
use super::{Confirmation, HorizonApp, Runtime, Settings, Stage, cloud_runtime, lifecycle::Action};
use cloud_runtime::{
    Cancellation, Error,
    lifecycle::{AgentStop, Requested},
};
use std::{path::Path, sync::mpsc::TryRecvError, time::Duration};

/// How often the worker's stop request is read.
const CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// What reading the stop requests finds, for the card.
#[derive(Debug)]
pub(in crate::app::cloud_panel) enum Found {
    /// A read is about to run; sent only to learn whether the card still listens.
    Watching,
    /// The worker took an agent's request: the card stops the cloud for it.
    Stop(AgentStop),
    /// A line for the card's log.
    Log(String),
}

/// Reads every `interval` until `send` finds that the card no longer listens. A failed
/// read is logged once until it changes.
fn run(interval: Duration, read: impl Fn() -> cloud_runtime::Result<Requested>, send: &dyn Fn(Found) -> bool) {
    let mut failed = None;
    loop {
        std::thread::sleep(interval);
        if !send(Found::Watching) {
            return;
        }
        let found = match read() {
            // Until the stop starts, a later read gives the same stop again.
            Ok(Requested::Stop(stop)) => Found::Stop(stop),
            Ok(Requested::Refused(words)) => Found::Log(format!("An agent's stop request was dropped: {words}")),
            Ok(Requested::Nothing | Requested::Waiting(_)) => {
                failed = None;
                continue;
            }
            // Another operation holds the cloud: the next read decides.
            Err(Error::Busy) => continue,
            Err(error) => {
                let message = format!("Reading the agent's stop request failed: {error}");
                if failed.as_ref() == Some(&message) {
                    continue;
                }
                failed = Some(message.clone());
                Found::Log(message)
            }
        };
        if !send(found) {
            return;
        }
    }
}

impl Runtime {
    /// The agent stop that reading found, if any; its log lines go to the card.
    pub(super) fn poll_agent_stops(&mut self) -> Option<AgentStop> {
        let mut stop = None;
        loop {
            let Some(found) = &self.agent_stops else {
                return stop;
            };
            match found.try_recv() {
                Ok(Found::Watching) => {}
                Ok(Found::Stop(found)) => stop = Some(found),
                Ok(Found::Log(line)) => self.push_log(line),
                Err(TryRecvError::Empty) => return stop,
                Err(TryRecvError::Disconnected) => {
                    self.agent_stops = None;
                    return stop;
                }
            }
        }
    }
}

impl HorizonApp {
    /// Starts reading the agents' stop requests of each cloud in `ready`, which just
    /// became ready, in place of an earlier reading, when its worker records them.
    pub(super) fn watch_agent_stops(&mut self, ready: Vec<u32>, ctx: &egui::Context) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        for id in ready {
            let state_root = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .find(|group| group.issue == id)
                .and_then(|group| group.remote.as_ref())
                .and_then(|launch| cloud_runtime::state::cloud_directory(&root, &launch.id).ok());
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
                continue;
            };
            runtime.agent_stops = None;
            let (Some(state_root), Some(state)) = (state_root, runtime.state.as_ref()) else {
                continue;
            };
            if !cloud_runtime::lifecycle::takes_stop_requests(state) {
                continue;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            runtime.agent_stops = Some(rx);
            let settings = root.join("settings.json");
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                run(CHECK_INTERVAL, || read(&state_root, &settings), &|found| {
                    let repaint = !matches!(found, Found::Watching);
                    let delivered = tx.send(found).is_ok();
                    if repaint {
                        ctx.request_repaint();
                    }
                    delivered
                });
            });
        }
    }

    /// Stops each cloud whose agent asked for it, as Stop does, for that agent. A cloud
    /// that is not ready, that another operation holds, or that has a dialog open gives
    /// the request back to its worker, which drops it, and the agent may ask again. A
    /// request given back once is never carried out later, even if the worker kept it.
    pub(super) fn stop_clouds_for_agents(&mut self, stops: Vec<(u32, AgentStop)>, ctx: &egui::Context) {
        for (id, stop) in stops {
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
                continue;
            };
            // A stop already under way, as one for this very request: nothing to give back.
            if runtime.operation == Some(Action::Stop) && runtime.stage == Some(Stage::Stopping) {
                continue;
            }
            let ready = runtime.stage == Some(Stage::Ready)
                && runtime
                    .state
                    .as_ref()
                    .is_some_and(cloud_runtime::lifecycle::takes_stop_requests);
            if !ready
                || runtime.busy()
                || !matches!(runtime.confirmation, Confirmation::None)
                || runtime.given_back == Some(stop.at)
            {
                if runtime.given_back != Some(stop.at) {
                    runtime.push_log(format!(
                        "{} asked to stop this cloud while it was not ready for it, or another operation of it \
                         was open; Horizon gives the request back",
                        stop.agent
                    ));
                    runtime.given_back = Some(stop.at);
                }
                if let Some(shown) = runtime.state.clone() {
                    self.give_back_stop_request(id, shown, stop);
                }
                continue;
            }
            runtime.agent_stop = Some(stop);
            self.change_production_worker(id, Action::Stop, ctx);
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
                runtime.agent_stop = None;
            }
        }
    }

    /// Gives `stop` back to the worker of cloud `id`, as `shown` names it, off the UI thread.
    fn give_back_stop_request(&self, id: u32, shown: cloud_runtime::state::Deployment, stop: AgentStop) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let Some(state_root) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == id)
            .and_then(|group| group.remote.as_ref())
            .and_then(|launch| cloud_runtime::state::cloud_directory(&root, &launch.id).ok())
        else {
            return;
        };
        std::thread::spawn(move || {
            let given = Settings::load(&root.join("settings.json")).and_then(|settings| {
                cloud_runtime::lifecycle::give_back_stop_request(
                    &state_root,
                    &shown,
                    &settings,
                    &Cancellation::default(),
                    &stop,
                )
            });
            // A request the worker keeps lapses ten minutes after its take; until then the
            // card gives it back again each time it is found.
            drop(given);
        });
    }
}

/// One read, with the settings read on this thread, not the UI's.
fn read(state_root: &Path, settings: &Path) -> cloud_runtime::Result<Requested> {
    let settings = Settings::load(settings)?;
    cloud_runtime::lifecycle::read_stop_request(state_root, &settings, &Cancellation::default())
}

#[cfg(test)]
mod tests;

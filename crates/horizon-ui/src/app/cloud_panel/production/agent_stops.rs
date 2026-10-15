//! Agent stops of ready clouds whose worker cannot stop itself (Hetzner, a Docker host).
//! While the cloud is ready and no other operation runs, Horizon reads the worker's
//! stop request every 30 seconds; a request the worker took becomes a Stop of the cloud
//! for that agent, which the card shows as any Stop. Only a running Horizon does this.
use super::{Deployment, Event, HorizonApp, Settings, cloud_runtime, lifecycle::Action};
use cloud_runtime::{
    Cancellation, Error,
    lifecycle::{AgentStop, Requested},
};
use std::{path::Path, sync::mpsc::Sender, time::Duration};

/// How often the worker's stop request is read.
const CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Starts reading the stop requests of the ready cloud `state`, if its worker records
/// them. It ends with `cancel`, which every later operation of the cloud cancels, or
/// once the card no longer listens.
pub(super) fn watch(
    state: &Deployment,
    settings: &Settings,
    root: &Path,
    cancel: &Cancellation,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) {
    if !cloud_runtime::lifecycle::takes_stop_requests(state) {
        return;
    }
    let (settings, root, cancel, tx, ctx) = (
        settings.clone(),
        root.to_owned(),
        cancel.clone(),
        tx.clone(),
        ctx.clone(),
    );
    std::thread::spawn(move || {
        run(
            &cancel,
            CHECK_INTERVAL,
            |cancel| cloud_runtime::lifecycle::read_stop_request(&root, &settings, cancel),
            &|event| {
                let repaint = !matches!(event, Event::Watching);
                let delivered = tx.send(event).is_ok();
                if repaint {
                    ctx.request_repaint();
                }
                delivered
            },
        );
    });
}

/// Reads every `interval` until `cancel`, or until `send` finds that the card no
/// longer listens. A failed read is logged once until it changes.
fn run(
    cancel: &Cancellation,
    interval: Duration,
    read: impl Fn(&Cancellation) -> cloud_runtime::Result<Requested>,
    send: &dyn Fn(Event) -> bool,
) {
    let mut failed = None;
    while wait(cancel, interval) {
        if !send(Event::Watching) {
            return;
        }
        let result = read(cancel);
        if cancel.is_cancelled() {
            return;
        }
        let event = match result {
            // Until the stop starts, which cancels this watch, a later read gives the
            // same stop again.
            Ok(Requested::Stop(stop)) => Event::AgentStopRequested(stop),
            Ok(Requested::Refused(words)) => Event::Output(format!("An agent's stop request was dropped: {words}")),
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
                Event::Output(message)
            }
        };
        if !send(event) {
            return;
        }
    }
}

/// Sleeps for `interval` in short steps; false once `cancel` is cancelled.
fn wait(cancel: &Cancellation, interval: Duration) -> bool {
    let step = Duration::from_millis(250).min(interval);
    let mut waited = Duration::ZERO;
    while waited < interval {
        if cancel.is_cancelled() {
            return false;
        }
        std::thread::sleep(step);
        waited += step;
    }
    !cancel.is_cancelled()
}

impl HorizonApp {
    /// Stops each cloud whose agent asked for it, as Stop does, for that agent.
    pub(super) fn stop_clouds_for_agents(&mut self, stops: Vec<(u32, AgentStop)>, ctx: &egui::Context) {
        for (id, stop) in stops {
            let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) else {
                continue;
            };
            runtime.agent_stop = Some(stop);
            self.change_production_worker(id, Action::Stop, ctx);
            // A Stop that did not start leaves nothing for a later Stop to take; the
            // worker gives the request again.
            if let Some(runtime) = self.cloud_prototype.production.runtimes.get_mut(&id) {
                runtime.agent_stop = None;
            }
        }
    }
}

#[cfg(test)]
mod tests;

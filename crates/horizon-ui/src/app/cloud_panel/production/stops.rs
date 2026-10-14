//! Stops the card learns of without making them: a stop the provider confirmed that
//! Horizon did not make, as an agent's stop on the worker, and a stop that finished
//! while an operation of the card waited for the cloud's record.
use super::{Deployment, Runtime, Stage, cloud_runtime};
use cloud_runtime::{Error, provider::Description, provider::StoppedCost};
use std::time::{Duration, Instant};

/// The line logged for a stop Horizon did not make.
fn outside_line(state: &Deployment) -> String {
    let resume = match Description::of(&state.profile).stopped {
        StoppedCost::WorkerKept => "Resume starts the same worker again.",
        StoppedCost::ServerDeleted => "Resume creates a new server that attaches the same workspace volume.",
    };
    format!(
        "The provider reports that this worker is stopped. Horizon did not stop it; \
         an agent on the worker or the provider account may have. {resume}"
    )
}

/// The saved record once the lock an operation found busy is released, as when
/// Reconnect is chosen while another stop, such as one from `cloud_deploy`, shuts the
/// worker down: the stop holds the lock for a few seconds, and the card should then
/// show what it left rather than the busy failure. `None` when the lock stays held or
/// the record cannot be read.
pub(super) fn after_busy(
    load: impl Fn() -> cloud_runtime::Result<Option<Deployment>>,
    pause: Duration,
) -> Option<Deployment> {
    (0..30)
        .find_map(|attempt| {
            if attempt > 0 {
                std::thread::sleep(pause);
            }
            match load() {
                Err(Error::Busy) => None,
                other => Some(other.ok().flatten()),
            }
        })
        .flatten()
}

/// What an operation that found the record busy reports once it could read it again:
/// a finished stop is shown as the card shows any Stop, and a stop that did not
/// finish offers to finish it. Any other record comes back, and the failure stands.
pub(super) fn stopped_while_busy(state: Deployment) -> Result<Vec<cloud_runtime::Event>, Box<Deployment>> {
    use cloud_runtime::Event;
    match state.stage {
        Stage::Stopped => Ok(vec![
            Event::Output(
                "This cloud was stopped while this operation waited for it. \
                 Choose Resume worker to start it again."
                    .into(),
            ),
            Event::Stopped(Box::new(state)),
        ]),
        Stage::Stopping => Ok(vec![
            Event::Snapshot(Box::new(state)),
            Event::failed("A stop of this cloud did not finish; choose Reconcile stop to finish it.".into()),
        ]),
        _ => Err(Box::new(state)),
    }
}

impl Runtime {
    /// Shows a stop the provider confirmed and Horizon did not make. A record that
    /// was already stopped before the check, as by an earlier Stop, shows as that stop.
    pub(super) fn show_stopped_outside(&mut self, state: Deployment) {
        if self.state.as_ref().is_some_and(|shown| shown.stop_requested) {
            self.show_stopped(
                state,
                "The provider confirmed that this worker is stopped. Choose Resume worker to start it again.".into(),
            );
            self.stopped_outside = false;
            return;
        }
        let line = outside_line(&state);
        self.show_stopped(state, line);
        self.stopped_outside = true;
    }

    fn show_stopped(&mut self, state: Deployment, line: String) {
        self.cancel = None;
        self.progress.stage(Stage::Stopped, Instant::now());
        self.stage = Some(state.stage);
        self.state = Some(state);
        // The record was just read: a read that found it busy before no longer counts.
        self.state_unavailable = false;
        self.desktop = None;
        self.error = None;
        self.receiver = None;
        self.push_note(line);
    }
}

#[cfg(test)]
mod tests;

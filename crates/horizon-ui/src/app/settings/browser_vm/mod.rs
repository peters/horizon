//! The Browser VM Settings section: the one machine of this Horizon, its hourly cost, and
//! the rent, stop, resume and delete that the host bridge runs off the UI thread.
//!
//! The section shows the record the host bridge answers and never reaches a provider
//! itself. Every button queues a request through the bridge, which shares the queue, the
//! refusals and the run threads of the `browser_vm` tool request. A refusal reaches the
//! section as text, never as a panic.
use std::time::{Duration, Instant};

use horizon_core::browser::manifest::provider_usage::{BrowserVmAction, BrowserVmRequest};
use horizon_core::cloud_runtime::browser_vm::{self, Phase, Status};
use horizon_core::cloud_runtime::cost;
use horizon_core::{BrowserHostConfig, Config};

use crate::app::browser_vm_requests::DeviceRoute;

mod app;
#[cfg(test)]
mod tests;
mod view;

pub(super) use view::render;

/// How often the section reads the phase again while a provider run is in flight. A run
/// reports the phase it started, and the phase it ends in is in the record.
const READ_INTERVAL: Duration = Duration::from_secs(2);

/// What the section asks of the window after it drew.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Action {
    /// One `browser_vm` request, from a button or from the fields of the section.
    Queue(BrowserVmRequest),
    /// Open the Device panel of the ready machine for the one-time sign-in.
    OpenDevicePanel,
}

/// What the section shows, and the fields a person rents with.
pub(in crate::app) struct Panel {
    image: String,
    server_type: String,
    location: String,
    volume_gb: u16,
    /// The record as the last answer named it.
    status: Option<Status>,
    /// The refusal or the failure of the last provider action, as text.
    error: Option<String>,
    /// Whether a provider run of this Horizon is in flight.
    busy: bool,
    /// Whether the delete waits for its second press.
    confirms_delete: bool,
    /// When the section last asked for the phase.
    last_read: Option<Instant>,
    /// Whether the person changed a field, so a recorded machine never overwrites them.
    edited: bool,
    actions: Vec<Action>,
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            image: String::new(),
            // The smallest dedicated size that the image fits on. Core refuses any other
            // server family, and the cloud settings name the locations that are allowed.
            server_type: format!("{}13", browser_vm::SERVER_TYPE_PREFIX),
            location: String::new(),
            volume_gb: browser_vm::DEFAULT_VOLUME_GB,
            status: None,
            error: None,
            busy: false,
            confirms_delete: false,
            last_read: None,
            edited: false,
            actions: Vec::new(),
        }
    }
}

/// Which buttons the section draws as available, from the record and the run in flight.
struct Offered {
    refresh: bool,
    start: bool,
    stop: bool,
    resume: bool,
    delete: bool,
    /// The Device panel and the settings write need the SSH route of the machine.
    sign_in: bool,
}

impl Panel {
    fn phase(&self) -> Option<Phase> {
        self.status.as_ref().map(|status| status.phase)
    }

    fn offered(&self) -> Offered {
        let phase = self.phase();
        let idle = !self.busy;
        Offered {
            // A read touches no provider, so it is answered while a run is in flight.
            refresh: true,
            // A recorded rent that no run of this window performs is taken over by a start,
            // which is how an interrupted deploy is finished.
            start: idle
                && matches!(
                    phase,
                    None | Some(Phase::Idle | Phase::Deleted | Phase::Failed | Phase::Deploying)
                ),
            stop: idle && matches!(phase, Some(Phase::Ready | Phase::Failed)),
            resume: idle && phase == Some(Phase::Stopped),
            // Any phase that names a machine can remove it, also one that a crash left in
            // progress, because only a delete removes what a lost run may have rented.
            delete: idle && !matches!(phase, None | Some(Phase::Idle | Phase::Deleted)),
            sign_in: phase == Some(Phase::Ready) && self.device_route().is_some(),
        }
    }

    /// Why the recorded run failed, when the record names a reason. A run that starts and
    /// then fails inside the provider answers its beginning, so its reason is only in the
    /// record; a person must still read it.
    fn failure(&self) -> Option<&str> {
        let status = self.status.as_ref()?;
        status.error.as_deref().filter(|reason| !reason.trim().is_empty())
    }

    /// The SSH target of the machine, which is the managed entry `horizon-browser-vm` while
    /// the machine runs, when the record names one.
    fn ssh_target(&self) -> Option<&str> {
        self.status.as_ref().and_then(|status| status.ssh_target.as_deref())
    }

    /// What the record says about the managed ssh entry of the machine, when it is not the
    /// plain case: the entry is missing, or the host keys are not trusted yet.
    fn ssh_note(&self) -> Option<&str> {
        self.status
            .as_ref()
            .and_then(|status| status.ssh_note.as_deref())
            .filter(|note| !note.trim().is_empty())
    }

    /// Takes the recorded machine into the fields that the person did not change, so a
    /// retry of a failed rent starts from the image that failed.
    fn prefill(&mut self) {
        let Some(status) = &self.status else {
            return;
        };
        if self.image.trim().is_empty() {
            self.image = status.image.clone().unwrap_or_default();
        }
        if self.location.trim().is_empty() {
            self.location = status.location.clone().unwrap_or_default();
        }
        if let Some(server_type) = &status.server_type {
            self.server_type.clone_from(server_type);
        }
        if let Some(volume_gb) = status.volume_gb {
            self.volume_gb = volume_gb;
        }
    }

    /// Shows the answer of one request. A read keeps the refusal of an earlier action
    /// visible until the next action answers.
    pub(in crate::app) fn absorb(&mut self, action: BrowserVmAction, result: Result<Status, String>) {
        match result {
            Ok(status) => {
                if action != BrowserVmAction::Status {
                    self.error = None;
                }
                if !self.edited {
                    self.prefill();
                }
                self.status = Some(status);
            }
            Err(error) => self.error = Some(error),
        }
    }

    /// Shows a refusal that this window produced itself, such as a Device panel that could
    /// not open.
    fn refuse(&mut self, text: String) {
        self.error = Some(text);
    }

    /// Reads the phase again after a run ended, so the phase it ended in shows.
    pub(in crate::app) fn read_again(&mut self) {
        self.last_read = None;
    }

    /// Keeps the run flag current, and asks for the phase when that is due: once when the
    /// section opens, and again [`READ_INTERVAL`] after the last read while a run is in
    /// flight.
    fn sync(&mut self, visible: bool, running: bool) -> Option<BrowserVmRequest> {
        self.busy = running;
        if !visible {
            return None;
        }
        let due = self
            .last_read
            .is_none_or(|read| running && read.elapsed() >= READ_INTERVAL);
        due.then(|| {
            self.last_read = Some(Instant::now());
            self.request(BrowserVmAction::Status)
        })
    }

    /// The request of one action. Only a rent names the machine; every other action reads
    /// or changes the record of the machine that the record already names.
    fn request(&self, action: BrowserVmAction) -> BrowserVmRequest {
        let field = |value: &str| Some(value.trim().to_string()).filter(|value| !value.is_empty());
        let mut request = BrowserVmRequest {
            action,
            image: None,
            server_type: None,
            location: None,
            volume_gb: None,
        };
        if action == BrowserVmAction::Deploy {
            request.image = field(&self.image);
            request.server_type = field(&self.server_type);
            request.location = field(&self.location);
            request.volume_gb = Some(self.volume_gb);
        }
        request
    }

    /// Sends one request and takes the delete out of its confirmation.
    fn ask(&mut self, action: BrowserVmAction) {
        self.confirms_delete = false;
        self.actions.push(Action::Queue(self.request(action)));
    }

    fn arm_delete(&mut self) {
        self.confirms_delete = true;
    }

    fn open_device_panel(&mut self) {
        self.actions.push(Action::OpenDevicePanel);
    }

    fn cancel_delete(&mut self) {
        self.confirms_delete = false;
    }

    /// The route of the Device panel of the ready machine, for the one-time sign-in.
    fn device_route(&self) -> Option<DeviceRoute> {
        self.status.as_ref().and_then(DeviceRoute::of)
    }

    /// Writes the SSH target of the ready machine into the settings entry `browser_host`,
    /// which new browser panels read at the next start. Returns whether the entry changed,
    /// or why the machine cannot name it.
    fn use_host(&self, config: &mut Config) -> Result<bool, &'static str> {
        let Some(target) = self.ssh_target() else {
            return Err("The machine has no SSH target yet.");
        };
        let entry = BrowserHostConfig {
            ssh_target: Some(target.to_string()),
            profile: config.browser_host.profile.clone(),
        };
        if entry.validate().is_err() {
            return Err(
                "The SSH target of the machine is not a name that ssh accepts, so it cannot go \
                 into browser_host.",
            );
        }
        if config.browser_host.ssh_target() == Some(target) {
            return Ok(false);
        }
        config.browser_host = entry;
        Ok(true)
    }

    fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
}

/// The hourly price of the server type, as the shared cost code writes a Hetzner rate, and
/// that it is net of VAT.
fn hourly_line(rate: f64) -> String {
    format!("{} net", cost::format_rate(rate, "EUR"))
}

/// The section's choice of words for a phase.
fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Idle => "Never rented",
        Phase::Deploying => "Starting",
        Phase::Ready => "Ready",
        Phase::Stopping => "Stopping",
        Phase::Stopped => "Stopped",
        Phase::Resuming => "Resuming",
        Phase::Deleting => "Deleting",
        Phase::Deleted => "Deleted",
        Phase::Failed => "Failed",
    }
}

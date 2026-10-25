//! Host bridge for browser VM requests from MCP. A provider run takes minutes, so a
//! request is answered as soon as its run is recorded, and the caller polls `status`.
//! Every run happens on its own thread: no provider request or record write reaches the
//! UI thread.
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};

use horizon_core::browser::manifest::provider_usage::{BrowserVmAction, BrowserVmRequest, UsageRequest};
use horizon_core::cloud_runtime::{
    Cancellation,
    browser_vm::{self, Deploy, Operation, Phase, Status},
    settings::Settings,
};
use serde_json::{Value, json};

use super::{HorizonApp, browser_provider_usage::UsageHostState};

/// The run of this Horizon.
#[derive(Default)]
pub(super) struct State {
    job: Option<Job>,
}

struct Job {
    request: UsageRequest,
    answers: Receiver<Answer>,
    cancel: Cancellation,
}

impl Drop for Job {
    fn drop(&mut self) {
        // A job dropped with a run in flight stops at its next provider step instead of
        // renting on while nobody reads the answer.
        self.cancel.cancel();
    }
}

enum Answer {
    /// The run is recorded, with the phase it starts in.
    Begun(Result<Status, String>),
    /// The run ended, with the phase it reached.
    Finished,
}

impl State {
    fn busy(&self) -> bool {
        self.job.is_some()
    }
}

impl HorizonApp {
    /// Starts a browser VM request on its own thread, or refuses it while a run of this
    /// Horizon is in flight.
    pub(in crate::app) fn queue_browser_vm(&mut self, request: UsageRequest, ctx: &egui::Context) {
        let Some(root) = self.cloud_root() else {
            self.answer_browser_vm(
                &request,
                Err("browser_vm_unavailable: cloud support is not set up on this Horizon".into()),
            );
            return;
        };
        let state: &UsageHostState = &self.browser_create_host.provider_usage;
        if state.browser_vm.busy() {
            self.answer_browser_vm(
                &request,
                Err("browser_vm_busy: a browser VM run is already in progress; read its status".into()),
            );
            return;
        }
        let Some(action) = request.browser_vm.clone() else {
            return;
        };
        let (sender, answers) = channel();
        let cancel = Cancellation::default();
        let worker_cancel = cancel.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let begun = begin(&root, &action);
            let running = begun.as_ref().is_ok_and(|status| {
                matches!(
                    status.phase,
                    Phase::Deploying | Phase::Resuming | Phase::Stopping | Phase::Deleting
                )
            });
            let _ = sender.send(Answer::Begun(begun));
            repaint.request_repaint();
            if running {
                // A stop and a delete report no step the person can act on, so their
                // progress is read from the record with `status`. The phase a run ends in
                // is in that record too.
                let _ = browser_vm::run(&root, &worker_cancel, &mut |_| {});
            }
            // A read, or a request that was refused before it started, ends the job here,
            // so the next request runs.
            let _ = sender.send(Answer::Finished);
            repaint.request_repaint();
        });
        self.browser_create_host.provider_usage.browser_vm.job = Some(Job {
            request,
            answers,
            cancel,
        });
    }

    /// Answers a recorded run, and clears the job when its provider work ends.
    pub(in crate::app) fn poll_browser_vm(&mut self, ctx: &egui::Context) {
        let mut begun = None;
        let mut answered = false;
        let mut finished = false;
        if let Some(job) = self.browser_create_host.provider_usage.browser_vm.job.as_mut() {
            while let Ok(answer) = job.answers.try_recv() {
                match answer {
                    Answer::Begun(result) => {
                        answered = true;
                        begun = Some((job.request.clone(), result));
                    }
                    Answer::Finished => finished = true,
                }
            }
        }
        let failed = begun.as_ref().is_some_and(|(_, result)| result.is_err());
        if let Some((request, result)) = begun {
            self.answer_browser_vm(&request, result);
        }
        // A refused run never started, so its answer ends the job.
        if finished || (answered && failed) {
            self.browser_create_host.provider_usage.browser_vm.job = None;
            ctx.request_repaint();
        }
    }

    fn answer_browser_vm(&mut self, request: &UsageRequest, result: Result<Status, String>) {
        let mut answer = request.result(Vec::new(), None);
        match result {
            Ok(status) => answer.browser_vm = Some(describe(&status)),
            Err(error) => answer.error = Some(error),
        }
        self.publish_host_usage_result(answer);
    }
}

/// The recorded run this request asks for.
fn begin(root: &Path, request: &BrowserVmRequest) -> Result<Status, String> {
    let operation = match request.action {
        BrowserVmAction::Status => return browser_vm::status(root).map_err(|error| error.to_string()),
        BrowserVmAction::Resume => Operation::Resume,
        BrowserVmAction::Stop => Operation::Stop,
        BrowserVmAction::Delete => Operation::Delete,
        BrowserVmAction::Deploy => Operation::Deploy(Deploy {
            image: request
                .image
                .clone()
                .ok_or("browser_vm_invalid_request: deploy needs an image")?,
            server_type: request
                .server_type
                .clone()
                .ok_or("browser_vm_invalid_request: deploy needs a ccx server type")?,
            location: match &request.location {
                Some(location) => location.clone(),
                None => first_location(root)?,
            },
            volume_gb: request.volume_gb.unwrap_or(browser_vm::DEFAULT_VOLUME_GB),
        }),
    };
    browser_vm::begin(root, &operation).map_err(|error| error.to_string())
}

/// The location a deploy uses when it names none: the first the Hetzner settings allow.
fn first_location(root: &Path) -> Result<String, String> {
    let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
    settings
        .hetzner
        .as_ref()
        .and_then(|hetzner| hetzner.locations.first().cloned())
        .ok_or_else(|| "browser_vm_unavailable: the Hetzner settings name no location; name one in the request".into())
}

/// The answer of one browser VM request, as the tools read it.
fn describe(status: &Status) -> Value {
    let mut answer = json!({
        "phase": status.phase,
        "ssh_target": status.ssh_target,
        "image": status.image,
        "server_type": status.server_type,
        "location": status.location,
        "volume_gb": status.volume_gb,
        "hourly_net": status.hourly_net,
        "error": status.error,
    });
    if status.phase == Phase::Ready
        && let Some(host) = status.ssh_host()
    {
        answer["device_panel"] = json!({
            "ssh": {"host": host, "user": "root", "vnc": browser_vm::VNC_ADDRESS}
        });
        answer["person_step"] = json!(
            "A person must sign in now: call device_panel with the ssh route above and sign in to the sites on the display of the machine. The state volume keeps those sign-ins across a stop and a resume. An agent cannot do this step."
        );
        answer["browser_host"] = json!({
            "ssh_target": status.ssh_target,
            "note": "set this entry in the Settings window or the exported config; it applies at the next start of Horizon",
        });
    }
    answer
}

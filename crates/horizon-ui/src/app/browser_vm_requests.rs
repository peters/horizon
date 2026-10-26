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

use super::HorizonApp;

/// The requests of this Horizon that are on their own thread.
#[derive(Default)]
pub(super) struct State {
    jobs: Vec<Job>,
}

struct Job {
    request: UsageRequest,
    answers: Receiver<Answer>,
    cancel: Cancellation,
    /// Whether this job runs provider work, so a second one must wait for it.
    mutating: bool,
    finished: bool,
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
    /// Whether a provider run of this Horizon is in flight.
    fn running(&self) -> bool {
        self.jobs.iter().any(|job| job.mutating)
    }
}

/// Why a request waits, or nothing when it may start. A read touches nothing, so it is
/// never refused while a run is in flight: reading the phase is how the caller watches
/// that run.
fn refusal(action: BrowserVmAction, running: bool) -> Option<&'static str> {
    (running && runs_provider(action))
        .then_some("browser_vm_busy: a browser VM run is already in progress; read its status")
}

/// Whether this action runs provider work. Only these wait for each other, and only these
/// answer with the phase of a run that is still going.
fn runs_provider(action: BrowserVmAction) -> bool {
    action != BrowserVmAction::Status
}

impl HorizonApp {
    /// Starts a browser VM request on its own thread, or refuses a provider run while one
    /// of this Horizon is in flight.
    pub(in crate::app) fn queue_browser_vm(&mut self, request: UsageRequest, ctx: &egui::Context) {
        let Some(root) = self.cloud_root() else {
            self.answer_browser_vm(
                &request,
                Err("browser_vm_unavailable: cloud support is not set up on this Horizon".into()),
            );
            return;
        };
        let Some(action) = request.browser_vm.clone() else {
            return;
        };
        if let Some(refusal) = refusal(
            action.action,
            self.browser_create_host.provider_usage.browser_vm.running(),
        ) {
            self.answer_browser_vm(&request, Err(refusal.into()));
            return;
        }
        let mutating = runs_provider(action.action);
        let (sender, answers) = channel();
        let cancel = Cancellation::default();
        let worker_cancel = cancel.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let begun = work(&root, &action, &worker_cancel, &mut run_provider);
            let _ = sender.send(Answer::Begun(begun));
            repaint.request_repaint();
            let _ = sender.send(Answer::Finished);
            repaint.request_repaint();
        });
        self.browser_create_host.provider_usage.browser_vm.jobs.push(Job {
            request,
            answers,
            cancel,
            mutating,
            finished: false,
        });
    }

    /// Answers every recorded run, and drops a job when its thread ended.
    pub(in crate::app) fn poll_browser_vm(&mut self, ctx: &egui::Context) {
        let mut answers = Vec::new();
        {
            let state = &mut self.browser_create_host.provider_usage.browser_vm;
            for job in &mut state.jobs {
                while let Ok(answer) = job.answers.try_recv() {
                    match answer {
                        Answer::Begun(result) => answers.push((job.request.clone(), result)),
                        Answer::Finished => job.finished = true,
                    }
                }
            }
            state.jobs.retain(|job| !job.finished);
        }
        if answers.is_empty() {
            return;
        }
        for (request, result) in answers {
            self.answer_browser_vm(&request, result);
        }
        ctx.request_repaint();
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

/// The provider run an action asks for, or nothing for a read.
fn operation(request: &BrowserVmRequest) -> Option<Operation> {
    Some(match request.action {
        BrowserVmAction::Status => return None,
        BrowserVmAction::Resume => Operation::Resume,
        BrowserVmAction::Stop => Operation::Stop,
        BrowserVmAction::Delete => Operation::Delete,
        BrowserVmAction::Deploy => Operation::Deploy(Deploy {
            image: request.image.clone().unwrap_or_default(),
            server_type: request.server_type.clone().unwrap_or_default(),
            location: request.location.clone().unwrap_or_default(),
            volume_gb: request.volume_gb.unwrap_or(browser_vm::DEFAULT_VOLUME_GB),
        }),
    })
}

/// Answers one request. A read answers the recorded phase and never reaches `run`; an
/// interrupted run is finished by an explicit deploy, stop or delete.
fn work(
    root: &Path,
    request: &BrowserVmRequest,
    cancel: &Cancellation,
    run: &mut dyn FnMut(&Path, &Cancellation),
) -> Result<Status, String> {
    let Some(mut operation) = operation(request) else {
        return browser_vm::status(root).map_err(|error| error.to_string());
    };
    if let Operation::Deploy(deploy) = &mut operation
        && deploy.location.is_empty()
    {
        deploy.location = first_location(root)?;
    }
    let begun = browser_vm::begin(root, &operation).map_err(|error| error.to_string())?;
    if runs_provider(request.action) {
        run(root, cancel);
    }
    Ok(begun)
}

fn run_provider(root: &Path, cancel: &Cancellation) {
    // A stop and a delete report no step the person can act on, so their progress is read
    // from the record with `status`. The phase a run ends in is in that record too.
    let _ = browser_vm::run(root, cancel, &mut |_| {});
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

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::cloud_runtime::browser_vm::{Phase, Record};

    fn request(action: BrowserVmAction) -> BrowserVmRequest {
        BrowserVmRequest {
            action,
            image: None,
            server_type: None,
            location: None,
            volume_gb: None,
        }
    }

    /// A cloud settings directory with no cloud settings, so the answer of a read needs
    /// no provider and a run that reached one would fail at its first check.
    fn root(phase: Phase) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        Record {
            phase,
            image: Some("ghcr.io/example/vm@sha256:synthetic".into()),
            server_type: Some("ccx13".into()),
            location: Some("fsn1".into()),
            volume_gb: Some(20),
            ..Record::default()
        }
        .save(root.path())
        .unwrap();
        root
    }

    #[test]
    fn a_status_read_is_answered_while_a_provider_run_is_in_flight() {
        // The tool, the skill and the busy message all tell the caller to read its status.
        assert_eq!(refusal(BrowserVmAction::Status, true), None);
        assert_eq!(refusal(BrowserVmAction::Status, false), None);
        for action in [
            BrowserVmAction::Deploy,
            BrowserVmAction::Stop,
            BrowserVmAction::Resume,
            BrowserVmAction::Delete,
        ] {
            let refused = refusal(action, true).unwrap_or_else(|| panic!("{action:?} must wait for the run"));
            assert!(refused.starts_with("browser_vm_busy"), "{refused}");
            assert_eq!(refusal(action, false), None, "{action:?} runs when nothing else does");
        }
    }

    #[test]
    fn a_status_read_runs_no_provider_work() {
        assert!(!runs_provider(BrowserVmAction::Status));
        assert!(
            operation(&request(BrowserVmAction::Status)).is_none(),
            "a read has no provider operation to run"
        );
        for action in [
            BrowserVmAction::Deploy,
            BrowserVmAction::Stop,
            BrowserVmAction::Resume,
            BrowserVmAction::Delete,
        ] {
            assert!(runs_provider(action), "{action:?}");
        }
    }

    #[test]
    fn a_status_read_reports_a_stale_deploying_phase_and_runs_nothing() {
        // A crash during a deploy leaves the record at `deploying`. A read answers that
        // phase; it must never pick the rental up again by itself.
        let root = root(Phase::Deploying);
        let mut started = 0;
        let answered = work(
            root.path(),
            &request(BrowserVmAction::Status),
            &Cancellation::default(),
            &mut |_, _| started += 1,
        )
        .unwrap();
        assert_eq!(answered.phase, Phase::Deploying);
        assert_eq!(started, 0, "a read must not run the provider");
        assert_eq!(
            Record::load(root.path()).unwrap().phase,
            Phase::Deploying,
            "a read changes no record"
        );
    }

    #[test]
    fn a_stop_answers_the_started_phase_and_runs_the_provider_once() {
        let root = root(Phase::Ready);
        let mut started = 0;
        let answered = work(
            root.path(),
            &request(BrowserVmAction::Stop),
            &Cancellation::default(),
            &mut |_, _| started += 1,
        )
        .unwrap();
        assert_eq!(answered.phase, Phase::Stopping);
        assert_eq!(started, 1);
    }

    #[test]
    fn a_refused_action_runs_nothing() {
        // Nothing was ever rented, so the delete is refused before the provider.
        let root = root(Phase::Idle);
        let mut started = 0;
        let refused = work(
            root.path(),
            &request(BrowserVmAction::Delete),
            &Cancellation::default(),
            &mut |_, _| started += 1,
        )
        .unwrap_err();
        assert!(refused.contains("nothing to delete"), "{refused}");
        assert_eq!(started, 0);
    }

    fn ready() -> Status {
        Status {
            phase: Phase::Ready,
            ssh_target: Some("root@203.0.113.10".into()),
            image: Some("ghcr.io/example/browser-vm@sha256:synthetic".into()),
            server_type: Some("ccx13".into()),
            location: Some("fsn1".into()),
            volume_gb: Some(20),
            hourly_net: Some(0.0689),
            error: None,
        }
    }

    #[test]
    fn a_ready_machine_names_the_route_a_person_signs_in_through() {
        let answer = describe(&ready());
        assert_eq!(answer["phase"], "ready");
        assert_eq!(answer["ssh_target"], "root@203.0.113.10");
        assert_eq!(answer["device_panel"]["ssh"]["host"], "203.0.113.10");
        assert_eq!(answer["device_panel"]["ssh"]["user"], "root");
        assert_eq!(answer["device_panel"]["ssh"]["vnc"], "127.0.0.1:5900");
        assert!(
            answer["person_step"]
                .as_str()
                .unwrap()
                .contains("A person must sign in"),
            "the step where a person is needed is stated plainly"
        );
        assert_eq!(answer["browser_host"]["ssh_target"], "root@203.0.113.10");
    }

    #[test]
    fn a_machine_that_is_not_ready_names_no_sign_in() {
        for phase in [Phase::Deploying, Phase::Stopped, Phase::Deleting] {
            let answer = describe(&Status { phase, ..ready() });
            assert!(answer.get("person_step").is_none(), "{phase:?}");
            assert!(answer.get("device_panel").is_none(), "{phase:?}");
        }
    }

    #[test]
    fn a_failed_run_reports_its_reason() {
        let failed = Status {
            phase: Phase::Failed,
            error: Some("the provider refused the server".into()),
            ..ready()
        };
        assert_eq!(describe(&failed)["error"], "the provider refused the server");
    }
}

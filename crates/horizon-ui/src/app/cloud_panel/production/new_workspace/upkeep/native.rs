//! Native tests of Upkeep pull requests on test clouds. An Upkeep agent asks for one with a
//! request file; [`super::sync`] brings the requests here. One test at a time per Upkeep:
//! Horizon checks the request on a thread, starts a test cloud from the repository's own
//! profile on the pull request's head in the Upkeep's workspace, opens a Codex panel and the
//! desktop viewer on it, gives the agent its task, looks for its result every 30 seconds,
//! keeps the evidence and closes the cloud. The ledger on disk lets a restart continue.
//!
//! The request, the result and the evidence are untrusted: see
//! [`horizon_core::upkeep::native`] and `docs/upkeep.md`.
use std::path::Path;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

use horizon_core::cloud_panel::{CloudLaunch, Placement};
use horizon_core::cloud_runtime::{self, Stage};
use horizon_core::upkeep::native::{self, Report, Request, State as Test};
use horizon_core::upkeep::worker::Config;
use horizon_core::{PanelKind, PanelOptions};

use super::super::super::{HorizonApp, Runtime, creation_job, repository_setup};
use crate::app::upkeep::Native;

mod jobs;
use jobs::{approve, deliver, expired_message, finished, load, look, save};

/// How often the test cloud is asked for its result.
const PROBE_EVERY: Duration = Duration::from_secs(30);
/// How long after a failed read of the ledger, or a failed delivery, Horizon tries again.
const RETRY: Duration = Duration::from_secs(30);
/// The agent gets its task once its panel has been open this long and has gone quiet.
const PROMPT_SETTLE: Duration = Duration::from_secs(15);
const PROMPT_QUIET: Duration = Duration::from_secs(3);
const AGENT: &str = "Native test agent";

impl HorizonApp {
    /// Each frame: follows the native tests of every Upkeep this board holds.
    pub(super) fn poll_upkeep_natives(&mut self, ctx: &egui::Context, saved: &[(String, Config)]) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        for (cloud_id, _) in saved {
            let Some(sync) = self.upkeep.syncs.get_mut(cloud_id) else {
                continue;
            };
            // Taken out for the step, which also changes the board and the clouds.
            let mut native = std::mem::take(&mut sync.native);
            self.native_step(ctx, &root, cloud_id, &mut native);
            if let Some(sync) = self.upkeep.syncs.get_mut(cloud_id) {
                sync.native = native;
            }
        }
    }

    fn native_step(&mut self, ctx: &egui::Context, root: &Path, cloud_id: &str, native: &mut Native) {
        let now = Instant::now();
        if !load(ctx, cloud_id, native, now) {
            return;
        }
        let Some(mut ledger) = native.ledger.take() else { return };
        if !self.native_on_board(cloud_id, &ledger) {
            native.ledger = Some(ledger);
            return;
        }
        if let Some((_, intake)) = &native.intake {
            while let Ok(requests) = intake.try_recv() {
                native.dirty |= ledger.take(requests);
            }
        }
        let closing = self.native_closing(ctx, &mut ledger, native);
        approval(&mut ledger, native);
        if !closing && native.approving.is_none() {
            // A test whose check a restart cut short is checked again.
            let unchecked = ledger
                .current
                .as_ref()
                .filter(|current| current.cloud.is_none())
                .map(|current| current.request.clone());
            if let Some(request) = unchecked.or_else(|| ledger.start_next()) {
                native.dirty = true;
                native.approving = Some(approve(ctx, root, cloud_id, request));
            }
        }
        self.native_test(ctx, root, cloud_id, &mut ledger, native, now);
        self.native_delivery(ctx, root, cloud_id, &ledger, native, now);
        if let Some(stored) = save(ctx, cloud_id, &ledger, native) {
            self.native_launch(ctx, cloud_id, &mut ledger, native, stored);
        }
        native.ledger = Some(ledger);
    }

    /// Whether this board holds the Upkeep's cloud or a test cloud of its ledger. The tests
    /// of an Upkeep on another session's board wait until that board is open again, and a
    /// test whose Upkeep was closed still ends.
    fn native_on_board(&self, upkeep: &str, ledger: &native::Ledger) -> bool {
        let on_board = |cloud: &str| {
            self.cloud_prototype
                .groups
                .0
                .iter()
                .any(|group| group.remote.as_ref().is_some_and(|launch| launch.id == cloud))
        };
        on_board(upkeep)
            || ledger
                .current
                .as_ref()
                .and_then(|current| current.cloud.as_deref())
                .is_some_and(on_board)
            || ledger.closing.iter().any(|cloud| on_board(cloud))
    }

    /// Starts the approved test cloud once the ledger on disk names it (`stored`), so a
    /// restart closes a cloud that started; a ledger that could not be saved starts none.
    fn native_launch(
        &mut self,
        ctx: &egui::Context,
        upkeep: &str,
        ledger: &mut native::Ledger,
        native: &mut Native,
        stored: bool,
    ) {
        let Some((approved, cloud)) = native.launching.take() else {
            return;
        };
        let Some(request) = ledger.current.as_ref().map(|current| current.request.clone()) else {
            return;
        };
        native.dirty = true;
        let started = if stored {
            self.launch_test_cloud(ctx, upkeep, &request, approved, cloud)
        } else {
            Err("Horizon could not save its record of native tests, so it starts no test cloud.".into())
        };
        match started {
            Ok(title) => ledger.report(Report::new(
                &request,
                Test::Starting,
                format!("Horizon starts the test cloud {title}."),
            )),
            Err(error) => ledger.finish(Report::new(&request, Test::Refused, error)),
        }
    }

    /// Adds the test cloud's card to the Upkeep's workspace and starts it. Returns its title.
    fn launch_test_cloud(
        &mut self,
        ctx: &egui::Context,
        upkeep: &str,
        request: &Request,
        approved: native::Approved,
        cloud: String,
    ) -> Result<String, String> {
        if !self.active_session.as_ref().is_some_and(|session| session.persistent) {
            return Err("Horizon has no saved session open, and a cloud is recorded with one.".into());
        }
        let workspace = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.remote.as_ref().is_some_and(|launch| launch.id == upkeep))
            .map(|group| group.workspace.clone())
            .ok_or("The Upkeep's cloud is gone.")?;
        let profile = creation_job::launch_profile(&approved.profile, Some(&cloud_runtime::provider::HETZNER), None)
            .map_err(|error| format!("The repository's profile cannot run on Hetzner: {error}."))?;
        let launch = CloudLaunch {
            deployment_started: false,
            id: cloud,
            revision: approved.revision,
            profile_name: approved.profile_name,
            profile,
            placement: Placement::default(),
        };
        let name = request.repository.rsplit('/').next().unwrap_or(&request.repository);
        let title = format!("Test {name} #{}", request.number);
        let id = self
            // The image comes from the pull request's head commit, never from a working copy.
            .add_cloud_group(title.clone(), workspace, approved.checkout, launch, Vec::new(), None)
            .map_err(|error| format!("{error}."))?;
        self.start_production_deployment(id, ctx);
        Ok(title)
    }

    /// Follows the test that runs: opens its panels and gives the agent its task once the
    /// cloud is ready, looks for the result, and ends the test with its report.
    fn native_test(
        &mut self,
        ctx: &egui::Context,
        root: &Path,
        upkeep: &str,
        ledger: &mut native::Ledger,
        native: &mut Native,
        now: Instant,
    ) {
        if native.launching.is_some() {
            return;
        }
        let Some(current) = ledger.current.clone() else { return };
        let Some(cloud) = current.cloud.clone() else { return };
        let Some(index) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .position(|group| group.remote.as_ref().is_some_and(|launch| launch.id == cloud))
        else {
            native.dirty = true;
            let ending = native.ending.take();
            native.forget_test();
            let report = ending.unwrap_or_else(|| {
                Report::new(
                    &current.request,
                    Test::Blocked,
                    "The test cloud was closed before its agent wrote a result.",
                )
            });
            ledger.finish(report);
            return;
        };
        let card = self.cloud_prototype.groups.0[index].issue;
        if native.ending.is_some() {
            self.close_test_cloud(ctx, card, ledger, native);
            return;
        }
        // Before anything waits on a look at the cloud, so the task never sits unsent.
        let prompted = current.prompted || self.submit_task(upkeep, ledger, native, now);
        if let Some(rx) = &native.probing {
            match rx.try_recv() {
                Ok(probed) => {
                    native.probing = None;
                    // Only a look that copied the evidence ends a test at its time limit; a look
                    // that started before the limit is followed by one that does.
                    if let Some(report) = finished(&current.request, probed, native.last_look) {
                        self.end_test(ctx, card, ledger, native, report);
                    }
                    return;
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => native.probing = None,
            }
        }
        let expired = current.expired(native::now());
        let Some(runtime) = self.cloud_prototype.production.runtimes.get(&card) else {
            // A card that a restart did not deploy again: it can still reach its time limit.
            if expired {
                let report = Report::new(&current.request, Test::Expired, expired_message());
                self.end_test(ctx, card, ledger, native, report);
            }
            return;
        };
        let ready = runtime
            .state
            .as_ref()
            .filter(|state| {
                runtime.stage == Some(Stage::Ready) && state.worker.is_some() && runtime.preparation.is_none()
            })
            .cloned();
        if ready.is_none() && !runtime.busy() && runtime.error.is_some() && runtime.stage != Some(Stage::Ready) {
            let error = runtime.error.clone().unwrap_or_default();
            let report = Report::new(
                &current.request,
                Test::Blocked,
                format!("The test cloud did not start: {error}"),
            );
            self.end_test(ctx, card, ledger, native, report);
            return;
        }
        let Some(state) = ready else {
            if expired {
                let report = Report::new(&current.request, Test::Expired, expired_message());
                self.end_test(ctx, card, ledger, native, report);
            }
            return;
        };
        // Until the Enter reaches the panel, the task counts as not given: a session switch
        // or a restart before it gives the task again.
        if !prompted && native.submit_at.is_none() && !expired {
            self.prompt_test_agent(ctx, index, ledger, native, now);
        }
        let due = expired || native.next_probe.is_none_or(|next| now >= next);
        if (prompted || expired) && due {
            native.next_probe = Some(now + PROBE_EVERY);
            let evidence = native::evidence_directory(upkeep, &current.request);
            native.last_look = expired;
            native.probing = Some(look(ctx, root, cloud, state, evidence, expired));
        } else if let Some(next) = native.next_probe {
            ctx.request_repaint_after(next.saturating_duration_since(now));
        }
    }

    /// Opens the agent panel and the desktop viewer of the test cloud on card `index`, and
    /// pastes the task once the agent has drawn its screen and gone quiet.
    fn prompt_test_agent(
        &mut self,
        ctx: &egui::Context,
        index: usize,
        ledger: &mut native::Ledger,
        native: &mut Native,
        now: Instant,
    ) {
        let card = self.cloud_prototype.groups.0[index].issue;
        if !native.desktop_shown
            && self
                .cloud_prototype
                .production
                .runtimes
                .get(&card)
                .is_some_and(|runtime| runtime.desktop.is_some())
        {
            native.desktop_shown = true;
            self.cloud_add_panel(ctx, card, PanelKind::Device, None);
        }
        let Some((panel, opened)) = native.agent.filter(|(panel, _)| self.board.panel(*panel).is_some()) else {
            if native.agent_after.is_none_or(|after| now >= after) {
                native.agent = self.open_test_agent(index).map(|panel| (panel, now));
                native.agent_after = Some(now + PROMPT_SETTLE);
            }
            ctx.request_repaint_after(PROMPT_SETTLE);
            return;
        };
        let Some(current) = ledger.current.as_ref() else { return };
        let Some(terminal) = self.board.panel_mut(panel) else {
            return;
        };
        if now.duration_since(opened) < PROMPT_SETTLE || terminal.had_recent_output_within(PROMPT_QUIET) {
            ctx.request_repaint_after(PROMPT_QUIET);
            return;
        }
        terminal.write_input(&repository_setup::pasted(&native::task(&current.request)));
        native.submit_at = Some(now + repository_setup::SUBMIT_DELAY);
    }

    /// The agent panel of the test cloud on card `index`, with the agent [`native::agent`] names.
    fn open_test_agent(&mut self, index: usize) -> Option<horizon_core::PanelId> {
        let workspace = self
            .board
            .workspace_id_by_local_id(&self.cloud_prototype.groups.0[index].workspace)?;
        let kind = self.cloud_prototype.groups.0[index]
            .remote
            .as_ref()
            .and_then(|launch| native::agent(&launch.profile))?;
        let mut options = PanelOptions {
            name: Some(AGENT.into()),
            name_is_custom: Some(true),
            kind,
            ..PanelOptions::default()
        };
        let opened = self
            .prepare_cloud_remote_panel(index, &mut options)
            .and_then(|()| self.create_cloud_member(index, options, workspace));
        opened
            .inspect_err(|error| tracing::warn!(%error, "could not open the agent panel of a native test"))
            .ok()
    }

    /// Sends the Enter that follows the pasted task. Returns whether it reached the agent
    /// panel now; only then the ledger records the task as given.
    fn submit_task(&mut self, upkeep: &str, ledger: &mut native::Ledger, native: &mut Native, now: Instant) -> bool {
        let (Some(at), Some((panel, _))) = (native.submit_at, native.agent) else {
            return false;
        };
        if now < at {
            return false;
        }
        native.submit_at = None;
        let Some(terminal) = self.board.panel_mut(panel) else {
            return false;
        };
        terminal.write_input(b"\r");
        let Some(current) = ledger.current.as_mut() else {
            return false;
        };
        current.prompted = true;
        let request = current.request.clone();
        native.dirty = true;
        ledger.report(Report::new(
            &request,
            Test::Running,
            "The agent on the test cloud runs the repository's native qualification. Watch it in the Upkeep workspace.",
        ));
        tracing::info!(upkeep, test = %request.label(), "gave the native test its task");
        true
    }

    /// Ends the test with `report` and closes the test cloud on card `card` as soon as it can.
    fn end_test(
        &mut self,
        ctx: &egui::Context,
        card: u32,
        ledger: &mut native::Ledger,
        native: &mut Native,
        report: Report,
    ) {
        tracing::info!(state = ?report.state, "a native test ended");
        native.forget_test();
        native.ending = Some(report);
        self.close_test_cloud(ctx, card, ledger, native);
    }

    /// Closes the test cloud on card `card` once no operation holds it, and records the report
    /// of its test then. The ledger keeps the cloud until its card is gone.
    fn close_test_cloud(&mut self, ctx: &egui::Context, card: u32, ledger: &mut native::Ledger, native: &mut Native) {
        if self
            .cloud_prototype
            .production
            .runtimes
            .get(&card)
            .is_some_and(Runtime::busy)
        {
            ctx.request_repaint_after(Duration::from_secs(1));
            return;
        }
        if let Some(report) = native.ending.take() {
            ledger.finish(report);
        }
        if let Some(cloud) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.issue == card)
            .and_then(|group| group.remote.as_ref())
            .map(|launch| launch.id.clone())
            && !ledger.closing.contains(&cloud)
        {
            ledger.closing.push(cloud.clone());
            native.close_tried.insert(cloud);
        }
        native.dirty = true;
        self.close_cloud_unasked(card, ctx);
    }

    /// Follows the test clouds that Horizon closes: forgets each one whose card is gone, and
    /// closes once more, once in each run of Horizon, one that a restart left open. A close
    /// that the person declined in the close dialog is not asked again. Returns whether one
    /// still has its card, also after a close that failed or waits in the dialog: no new test
    /// cloud starts until each one is gone.
    fn native_closing(&mut self, ctx: &egui::Context, ledger: &mut native::Ledger, native: &mut Native) -> bool {
        let mut gone = Vec::new();
        for cloud in ledger.closing.clone() {
            let Some(card) = self
                .cloud_prototype
                .groups
                .0
                .iter()
                .find(|group| group.remote.as_ref().is_some_and(|launch| launch.id == cloud))
                .map(|group| group.issue)
            else {
                gone.push(cloud);
                continue;
            };
            let busy = self
                .cloud_prototype
                .production
                .runtimes
                .get(&card)
                .is_some_and(Runtime::busy);
            if !busy && native.close_tried.insert(cloud) {
                self.close_cloud_unasked(card, ctx);
            }
        }
        if !gone.is_empty() {
            ledger.closing.retain(|cloud| !gone.contains(cloud));
            native.dirty = true;
        }
        !ledger.closing.is_empty()
    }

    /// Writes the ledger's reports to the Upkeep worker after they changed, where its agents
    /// read them.
    fn native_delivery(
        &mut self,
        ctx: &egui::Context,
        root: &Path,
        upkeep: &str,
        ledger: &native::Ledger,
        native: &mut Native,
        now: Instant,
    ) {
        if let Some(rx) = &native.delivering {
            match rx.try_recv() {
                Ok(Ok(())) => native.delivering = None,
                Ok(Err(error)) => {
                    tracing::warn!(cloud = %upkeep, %error, "could not deliver the native test reports");
                    native.delivering = None;
                    native.undelivered = true;
                    native.next_delivery = Some(now + RETRY);
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    native.delivering = None;
                    native.undelivered = true;
                    native.next_delivery = Some(now + RETRY);
                }
            }
        }
        if !native.undelivered {
            return;
        }
        if let Some(next) = native.next_delivery.filter(|next| now < *next) {
            ctx.request_repaint_after(next - now);
            return;
        }
        let Some(state) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.remote.as_ref().is_some_and(|launch| launch.id == upkeep))
            .and_then(|group| self.cloud_prototype.production.runtimes.get(&group.issue))
            .and_then(|runtime| {
                runtime
                    .state
                    .clone()
                    .filter(|state| runtime.stage == Some(Stage::Ready) && state.worker.is_some())
            })
        else {
            return;
        };
        native.undelivered = false;
        native.delivering = Some(deliver(ctx, root, upkeep.to_owned(), state, ledger.clone()));
    }
}

/// Forgets what the app held about the test that ended, so the next test starts with its own
/// panels, look times and task.
/// Takes the outcome of the check of the request that starts now: names its test cloud in the
/// ledger, or records why it does not start.
fn approval(ledger: &mut native::Ledger, native: &mut Native) {
    let Some(rx) = &native.approving else { return };
    let approved = match rx.try_recv() {
        Ok(approved) => approved,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err("The check of the request stopped.".into()),
    };
    native.approving = None;
    native.dirty = true;
    let Some(current) = ledger.current.as_mut() else { return };
    match approved {
        Ok((approved, cloud)) => {
            current.cloud = Some(cloud.clone());
            native.launching = Some((approved, cloud));
        }
        Err(error) => {
            let request = current.request.clone();
            ledger.finish(Report::new(&request, Test::Refused, error));
        }
    }
}

#[cfg(test)]
mod tests;

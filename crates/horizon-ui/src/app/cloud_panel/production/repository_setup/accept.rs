//! **Use this setup**. Closing the agent's panel only hangs up, so a thread first ends
//! the agent and everything in its process group, killed if they ignore the hang-up,
//! and reads the files outside `.horizon`; the dialog keeps drawing meanwhile. Then the
//! setup is read once more, checked against the proof that passed and committed.
use super::agent::AgentProcess;
use super::commit::{self, Outside};
use super::set_configuration;
use crate::app::cloud_panel::production::HorizonApp;
use horizon_core::cloud_runtime::repository::launch::Configuration;
use std::time::Duration;

/// What the thread of Use reports: whether the agent's processes ended, the process
/// again when they did not, and the files outside `.horizon` once they did.
pub(super) type Ending = (bool, Option<AgentProcess>, Option<Outside>);

impl HorizonApp {
    /// Starts Use: closes the agent's panel and leaves the waiting to a thread.
    pub(super) fn use_cloud_repository_setup(&mut self, ctx: &egui::Context) {
        let Some(panel) = self.cloud_prototype.production.setup_run.as_ref().map(|run| run.panel) else {
            return;
        };
        if self.board.panel(panel).is_some() {
            self.close_panel(panel);
        }
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        if run.ending() {
            return;
        }
        let agent = run.agent_process.take();
        let repository = run.repository.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let ended = agent.as_ref().is_none_or(AgentProcess::end);
            // Read only once nothing of the agent's runs, so no late write races it.
            let outside = ended.then(|| commit::outside(&repository));
            let _ = sender.send((ended, agent, outside));
            wake.request_repaint();
        });
        run.ending = Some(receiver);
        self.cloud_prototype.error = None;
    }

    /// Goes on with Use once its thread reported. Returns whether Use finished or
    /// refused this frame, so nothing else of the run is synced after it.
    pub(super) fn poll_cloud_setup_use(&mut self, ctx: &egui::Context) -> bool {
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return false;
        };
        let Some(ending) = &run.ending else {
            return false;
        };
        match ending.try_recv() {
            Ok((true, _, Some(outside))) => {
                run.ending = None;
                self.finish_using_cloud_repository_setup(ctx, outside);
                true
            }
            Ok((_, agent, _)) => {
                run.ending = None;
                run.agent_process = agent;
                self.cloud_prototype.error =
                    Some("The setup agent is still ending. Press Use this setup again in a moment.".into());
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                run.ending = None;
                false
            }
        }
    }

    /// Commits the files the agent wrote and reads them as the repository's settings,
    /// once the agent has ended: the setup is read once, must be the proved one, and is
    /// what is committed; files outside `.horizon` that changed are named first.
    fn finish_using_cloud_repository_setup(&mut self, ctx: &egui::Context, outside: Outside) {
        let Some(run) = self.cloud_prototype.production.setup_run.as_mut() else {
            return;
        };
        run.refresh_inputs_now();
        let setup = super::snapshot::Setup::read(&run.repository);
        let proved = run.proof.as_ref().and_then(|proof| proof.inputs);
        let current = setup
            .as_ref()
            .ok()
            .zip(run.proof_spec())
            .map(|(setup, spec)| setup.fingerprint(spec));
        let setup = match setup {
            Ok(setup) if run.finished() && current.is_some() && current == proved => setup,
            _ => {
                run.stopped = true;
                self.cloud_prototype.error = Some(
                    "The .horizon files changed after Horizon's proof, so they are not committed. Start the setup again to prove them."
                        .into(),
                );
                return;
            }
        };
        // The agent may change only .horizon. Anything else that changed during the
        // setup is named once; pressing Use again commits .horizon and leaves the rest.
        let changed = commit::changed(&run.outside, &outside);
        if !changed.is_empty() {
            run.outside = outside;
            self.cloud_prototype.error = Some(outside_changes_message(&changed));
            return;
        }
        let committed = match commit::commit(&run.repository, run.mode.commit_message(), &setup) {
            Ok(committed) => committed,
            Err(error) => {
                self.cloud_prototype.error = Some(format!("Could not commit .horizon: {error}"));
                return;
            }
        };
        if let Some(run) = self.cloud_prototype.production.setup_run.take() {
            run.discard();
        }
        // The setup was made and committed on HEAD, so the dialog reads HEAD again.
        self.cloud_prototype.production.revision.clear();
        set_configuration(&mut self.cloud_prototype.production, Configuration::Committed);
        self.read_cloud_profiles(ctx);
        if let commit::Committed::IndexBehind(error) = committed {
            self.cloud_prototype.error = Some(format!(
                "Committed .horizon, but your git index was not updated ({error}). Run `git reset -q -- .horizon` in the repository to refresh it."
            ));
        }
    }
}

/// What Use says when files outside `.horizon` changed while the setup ran.
fn outside_changes_message(changed: &[String]) -> String {
    let shown = changed.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    let more = match changed.len().saturating_sub(5) {
        0 => String::new(),
        more => format!(" and {more} more"),
    };
    format!(
        "Changed outside .horizon while the setup ran: {shown}{more}. Use this setup commits only .horizon on the current HEAD and leaves the rest as it is. Review the changes, then press Use this setup again."
    )
}

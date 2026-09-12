//! Explicit repository preparation, transient credential input and manual receipt checks.

mod paint;

use super::{Context, HorizonHome, InventoryAction, RemoteEnvironmentSummary, WakeOnDrop};
use horizon_core::{
    cloud_run::{CloudProvider, CloudWorkflowStore},
    remote_git_setup::{self as git, ConfiguredRemoteGitSetupRequest, PreparedRemoteGitSetup, RemoteGitCredentialMode},
    remote_github_credential::RepositoryPat,
    remote_provider_config::RemoteProviderConfig,
    remote_ssh_identity::RemoteSshIdentityStore,
};
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default)]
pub(super) struct RepositoryState {
    pending: Option<Pending>,
    confirmation: Option<(Scope, Box<PreparedRemoteGitSetup>)>,
    token: String,
    consent: bool,
    install: bool,
    notice: Option<String>,
    unknown: bool,
}

#[derive(Clone, PartialEq)]
struct Scope {
    expected: RemoteEnvironmentSummary,
    config: RemoteProviderConfig,
    owner: String,
}
type Current<'a> = (&'a RemoteEnvironmentSummary, &'a RemoteProviderConfig, &'a str);
impl Scope {
    fn request(&self) -> ConfiguredRemoteGitSetupRequest<'_> {
        ConfiguredRemoteGitSetupRequest {
            expected: &self.expected,
            client_session_id: &self.owner,
        }
    }
    fn matches(&self, current: Option<Current<'_>>) -> bool {
        current.is_some_and(|(expected, config, owner)| {
            self.expected == *expected && self.config == *config && self.owner == owner
        })
    }
}
struct Pending {
    receiver: Receiver<Result<Completion, Failure>>,
    scope: Scope,
    mutating: bool,
    discard: bool,
}
enum Work {
    Preview(RemoteGitCredentialMode),
    Submit(Box<PreparedRemoteGitSetup>, Option<String>),
    Inspect,
}
enum Completion {
    Preview(Box<PreparedRemoteGitSetup>),
    Submitted(git::ConfiguredRemoteGitSubmission),
    Observed(git::RemoteGitObservation),
}
enum Failure {
    Refused(&'static str),
    Configured(git::ConfiguredRemoteGitSetupError),
    Unconfirmed,
}
impl Failure {
    fn mutation_possible(&self) -> bool {
        use git::ConfiguredRemoteGitSetupError::{Credential, Git, OutcomeUnknown};
        matches!(
            self,
            Self::Unconfirmed | Self::Configured(OutcomeUnknown | Git(_) | Credential(_))
        )
    }
    fn message(self) -> String {
        match self {
            Self::Refused(message) => message.into(),
            Self::Configured(error) => error.to_string(),
            Self::Unconfirmed => "Repository response was lost or unexpected.".into(),
        }
    }
}

pub(super) fn supported(provider: CloudProvider) -> bool {
    cfg!(target_os = "linux")
        && matches!(
            provider,
            CloudProvider::LocalDocker | CloudProvider::RunPod | CloudProvider::Azure
        )
}

impl RepositoryState {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    fn cancel(&mut self) {
        self.confirmation = None;
        self.token.clear();
        self.consent = false;
    }
    pub(super) fn invalidate(&mut self) {
        self.cancel();
        self.notice = None;
        if let Some(pending) = &mut self.pending {
            pending.discard = true;
        }
    }
    fn launch(&mut self, home: &HorizonHome, ctx: &Context, scope: Scope, work: Work) {
        if self.is_pending() {
            return;
        }
        let mutating = matches!(work, Work::Submit(..));
        let (tx, receiver) = mpsc::sync_channel(1);
        let home = home.clone();
        let owned = scope.clone();
        let wake = WakeOnDrop(ctx.clone());
        let started = std::thread::Builder::new()
            .name("remote-repository-preparation".into())
            .spawn(move || {
                let _wake = wake;
                let result = execute(&home, &owned, work);
                let _ = tx.send(result);
            });
        self.notice = None;
        match started {
            Ok(_) => {
                self.pending = Some(Pending {
                    receiver,
                    scope,
                    mutating,
                    discard: false,
                });
            }
            Err(_) => self.notice = Some("Repository operation could not start; no retry was scheduled.".into()),
        }
        ctx.request_repaint();
    }
    fn drain(&mut self, current: Option<Current<'_>>) {
        let Some(pending) = self.pending.take() else { return };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pending = Some(pending);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(Failure::Unconfirmed),
        };
        let mutation_possible = pending.mutating && result.as_ref().err().is_none_or(Failure::mutation_possible);
        if pending.discard || !pending.scope.matches(current) {
            self.unknown |= mutation_possible;
            return;
        }
        match result {
            Ok(Completion::Preview(prepared)) if !pending.mutating => {
                self.confirmation = Some((pending.scope, prepared));
            }
            Ok(Completion::Observed(observation)) if !pending.mutating => {
                self.notice = Some(observation_text(observation).into());
            }
            Ok(Completion::Submitted(result)) if pending.mutating => {
                use horizon_core::remote_github_credential::RemoteCredentialInstallation;
                let credential = match result.credential {
                    Some(RemoteCredentialInstallation::Installed) => {
                        "Credential installed; GitHub permissions are not verified. "
                    }
                    Some(RemoteCredentialInstallation::Present) => {
                        "Credential already present and unchanged; GitHub permissions are not verified. "
                    }
                    None => "",
                };
                let progress = match result.submission {
                    git::RemoteGitSubmission::Submitted => {
                        "Git handoff submitted; preparation completion is not yet known."
                    }
                    git::RemoteGitSubmission::Observed(observation) => observation_text(observation),
                    git::RemoteGitSubmission::Unknown => {
                        self.unknown = true;
                        "Git handoff is unconfirmed."
                    }
                };
                self.notice = Some(format!("{credential}{progress}"));
            }
            result => {
                let failure = result.err().unwrap_or(Failure::Unconfirmed);
                self.unknown |= mutation_possible;
                self.notice = Some(failure.message());
            }
        }
    }
    fn action(&mut self, action: InventoryAction, home: &HorizonHome, ctx: &Context, scope: Scope) {
        if self.is_pending() || !supported(scope.expected.provider) {
            return;
        }
        match action {
            InventoryAction::PrepareRepository => {
                self.cancel();
                let mode = if self.install {
                    RemoteGitCredentialMode::InstallFirst
                } else {
                    RemoteGitCredentialMode::UseInstalled
                };
                self.launch(home, ctx, scope, Work::Preview(mode));
            }
            InventoryAction::InspectRepository => {
                self.cancel();
                self.launch(home, ctx, scope, Work::Inspect);
            }
            InventoryAction::ConfirmRepository => {
                let Some((confirmed, prepared)) = self.confirmation.take() else {
                    return;
                };
                let installing = prepared.credential_mode() == RemoteGitCredentialMode::InstallFirst;
                if confirmed != scope || installing && (!self.consent || RepositoryPat::new(&self.token).is_err()) {
                    self.cancel();
                    self.notice = Some("Confirmation or explicit credential consent is invalid.".into());
                    return;
                }
                let token = installing.then(|| std::mem::take(&mut self.token));
                self.cancel();
                self.launch(home, ctx, scope, Work::Submit(prepared, token));
            }
            InventoryAction::CancelRepository => {
                self.cancel();
                ctx.request_repaint();
            }
            _ => {}
        }
    }
}

fn execute(home: &HorizonHome, scope: &Scope, work: Work) -> Result<Completion, Failure> {
    let store = CloudWorkflowStore::open_read_only(home)
        .map_err(|_| Failure::Refused("Repository control storage is unavailable."))?;
    let identities = RemoteSshIdentityStore::new(home);
    match work {
        Work::Preview(mode) => git::prepare_configured_remote_git_setup(&store, &scope.config, scope.request(), mode)
            .map(Box::new)
            .map(Completion::Preview)
            .map_err(Failure::Configured),
        Work::Inspect => git::inspect_configured_remote_git_setup(&store, &identities, &scope.config, scope.request())
            .map(Completion::Observed)
            .map_err(Failure::Configured),
        Work::Submit(prepared, token) => {
            let borrowed = token
                .as_deref()
                .map(RepositoryPat::new)
                .transpose()
                .map_err(|_| Failure::Refused("Repository token is invalid."))?;
            git::submit_configured_remote_git_setup(
                &store,
                &identities,
                &scope.config,
                scope.request(),
                *prepared,
                borrowed.as_ref(),
            )
            .map(Completion::Submitted)
            .map_err(Failure::Configured)
        }
    }
}

fn observation_text(observation: git::RemoteGitObservation) -> &'static str {
    match (observation.state, observation.reason) {
        (git::RemoteGitState::Complete, None) => {
            "Original preparation complete. This receipt does not prove current task readiness."
        }
        (git::RemoteGitState::Absent, None) => "No preparation receipt observed; absence does not authorize replay.",
        (git::RemoteGitState::ClaimedUnknown, _) => {
            "Preparation was claimed; completion is unknown. No retry is scheduled."
        }
        _ => "Preparation failed or its retained checkout cannot be verified. No retry is scheduled.",
    }
}

pub(super) fn show(ui: &mut egui::Ui, state: &mut RepositoryState, enabled: bool, action: &mut InventoryAction) {
    paint::show(ui, state, enabled, action);
}

impl super::HorizonApp {
    pub(super) fn remote_repository_action(&mut self, action: InventoryAction, ctx: &Context) {
        let state = &mut self.remote_environments;
        let current = state
            .page
            .as_ref()
            .filter(|_| state.open)
            .and_then(|page| state.selected.and_then(|i| page.rows.get(i)))
            .zip(self.active_session.as_ref().filter(|session| session.persistent))
            .filter(|(row, session)| row.summary.owning_session_id == session.session_id)
            .map(|(row, session)| (&row.summary, &self.template_config.remote, session.session_id.as_str()));
        state.repository.drain(current);
        if state
            .repository
            .confirmation
            .as_ref()
            .is_some_and(|(expected, _)| !expected.matches(current))
        {
            state.repository.cancel();
        }
        if !matches!(
            action,
            InventoryAction::PrepareRepository
                | InventoryAction::InspectRepository
                | InventoryAction::ConfirmRepository
                | InventoryAction::CancelRepository
        ) {
            return;
        }
        if let Some((expected, config, owner)) = current {
            let scope = Scope {
                expected: expected.clone(),
                config: config.clone(),
                owner: owner.into(),
            };
            if state.pending.is_none()
                && !state.observation.is_pending()
                && !state.stop.is_pending()
                && !state.reconnect.is_pending()
                && !state.reopen.is_pending()
            {
                if matches!(
                    action,
                    InventoryAction::PrepareRepository
                        | InventoryAction::InspectRepository
                        | InventoryAction::ConfirmRepository
                ) {
                    state.stop.cancel_confirmation();
                    state.reopen.invalidate();
                    state.reconnect.invalidate();
                }
                state.repository.action(action, self.session_store.home(), ctx, scope);
            }
        } else if matches!(
            action,
            InventoryAction::PrepareRepository | InventoryAction::InspectRepository
        ) {
            state.repository.notice =
                Some("Open the owning persistent session before preparing or inspecting this repository.".into());
        }
    }
}

#[cfg(test)]
mod tests;

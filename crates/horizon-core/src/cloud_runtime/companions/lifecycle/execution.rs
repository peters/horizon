use super::{Action, Binding, Cancellation, Error, Event, Intent, Phase, Request, Result, Settings, Store, intent};
use crate::cloud_runtime::{
    CreateState, deployment, lifecycle,
    mutation::{Observer, State as Mutation},
    state::Deployment,
};
use intent::{Decision, Observation};
use std::cell::Cell;

/// `confirmed` means the owner confirmed this operation's cloud creation on the card.
pub(super) fn plan(
    store: &Store,
    binding: &Binding,
    intent: &Intent,
    state: Option<&Deployment>,
    confirmed: bool,
) -> Result<Decision> {
    let observation = match state {
        Some(state) if deletion_pending(store, state)? => Observation::DeletionPending,
        Some(state) => Observation::Existing(state),
        None => {
            for file in ["hetzner.json", "workspace-volume.json", "workspace-volume.required"] {
                if store.root().join(file).try_exists()? {
                    return Err(Error::Invalid(
                        "Provider journal exists without its deployment; reconcile it first",
                    ));
                }
            }
            Observation::Missing
        }
    };
    let decision = intent::decide(intent.action, binding, observation, intent.state, false);
    if !matches!(decision, Decision::Refuse(_))
        && intent.action == Action::EnsureReady
        && intent.state == intent::State::Submitted
        && let Some(state) = state
        && state.operation == CreateState::Prepared
        && !confirmed
        // A resume on a provider whose stop deletes the server clears that server's
        // fence; a volume a server held proves the cloud was created, so a retry
        // only reconnects. Every other prepared record needs creation confirmed.
        && !resumed_on_a_new_server(store, state)?
    {
        // A panel and a prepared record do not prove paid creation was approved.
        return Ok(Decision::Provision);
    }
    Ok(decision)
}

/// The resume an Ensure Ready still has to run when the provider confirms the worker
/// stopped: whatever it had started never applied.
pub(super) fn resume_to_continue(reconciled: &lifecycle::ReconciledDeployment) -> Option<Decision> {
    if !reconciled.confirmed_stopped() {
        return None;
    }
    Some(
        if horizon_cloud::provider::Description::of(&reconciled.state.profile).stopped
            == horizon_cloud::provider::StoppedCost::ServerDeleted
        {
            Decision::ResumeWithNewServer
        } else {
            Decision::ResumeThenReconnect
        },
    )
}

/// Whether a prepared record is a resume in progress rather than a cloud never
/// created: only a provider whose stop deletes the server clears its fence on resume,
/// and then a volume a server held proves the cloud existed.
fn resumed_on_a_new_server(store: &Store, state: &Deployment) -> Result<bool> {
    if horizon_cloud::provider::Description::of(&state.profile).stopped
        != horizon_cloud::provider::StoppedCost::ServerDeleted
    {
        return Ok(false);
    }
    deployment::hetzner::held_by_a_server(store.root())
}

fn deletion_pending(store: &Store, state: &Deployment) -> Result<bool> {
    if intent::refuses_deployment(state) {
        return Ok(true);
    }
    if state.profile.provider != horizon_cloud::hetzner::PROVIDER {
        return deployment::storage::deletion_pending(store, state);
    }
    let bytes = match std::fs::read(store.root().join("hetzner.json")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && state.operation == CreateState::Prepared => {
            return Ok(false);
        }
        Err(error) => return Err(error.into()),
    };
    let journal: horizon_cloud::hetzner::cloud::Journal = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
    Ok(journal.deleting || matches!(journal.volume, CreateState::Terminated { .. }))
}

pub(super) trait Backend {
    fn preflight(&mut self, _store: &Store, _decision: Decision) -> Result<()> {
        Ok(())
    }
    /// Whether the run that follows only inspects: its receipt reads `Inspecting`, so it
    /// must not start anything a crash would leave looking read-only.
    fn inspecting(&mut self, _inspecting: bool) {}
    fn run(&mut self, store: &Store, decision: Decision, action: Action) -> Result<Phase>;
    /// Whether the last `run` may have left a provider mutation unconfirmed. A run
    /// that failed without one, as in a local check or a provider check before any
    /// request, is a definite failure that a fresh explicit request may retry.
    fn mutation_uncertain(&self) -> bool {
        true
    }
    /// Whether the last run proved the earlier uncertain operation settled, as a
    /// durable resume it went on to reconnect; its own failures are then judged on
    /// their own evidence.
    fn uncertainty_settled(&self) -> bool {
        false
    }
    fn verify_access(&mut self, request: &Request<'_>) -> Result<Phase>;
}

pub(super) struct Live<'a> {
    pub settings: &'a Settings,
    pub cancel: &'a Cancellation,
    pub emit: &'a dyn Fn(Event),
    /// Whether the last run left a provider mutation pending.
    pub pending: bool,
    /// Whether the current run only inspects.
    pub inspecting: bool,
    /// Whether the last run proved a settled resume before reconnecting.
    pub settled: bool,
}

impl Backend for Live<'_> {
    fn preflight(&mut self, store: &Store, decision: Decision) -> Result<()> {
        self.cancel.check()?;
        if matches!(
            decision,
            Decision::Stop
                | Decision::ResumeThenReconnect
                | Decision::ResumeWithNewServer
                | Decision::Reconnect
                | Decision::ReconcileOnly
        ) {
            let state = store
                .load()?
                .ok_or(Error::Invalid("Companion deployment disappeared"))?;
            if state.profile.provider == horizon_cloud::hetzner::PROVIDER {
                self.settings
                    .hetzner
                    .as_ref()
                    .ok_or(Error::Invalid("Configure this cloud provider before executing"))?
                    .credential()?;
            } else {
                self.settings.credential()?;
            }
            if matches!(
                decision,
                Decision::Reconnect | Decision::ResumeThenReconnect | Decision::ResumeWithNewServer
            ) {
                deployment::connection_preflight(&state.cloud_id, &state.revision, &state.profile, self.settings)?;
            }
            if matches!(
                decision,
                Decision::Stop | Decision::ResumeThenReconnect | Decision::ResumeWithNewServer
            ) {
                state.refuse_pending_replacement()?;
                if state.profile.provider != horizon_cloud::hetzner::PROVIDER {
                    if !matches!(state.operation, CreateState::Bound { .. }) {
                        return Err(Error::Invalid(
                            "Reconcile a bound worker before changing its power state",
                        ));
                    }
                    let spec = state
                        .spec
                        .as_ref()
                        .ok_or(Error::Invalid("Missing worker specification"))?;
                    if spec.operation_id != state.cloud_id || spec.profile != state.profile {
                        return Err(Error::Invalid("Deployment and worker identities differ"));
                    }
                    if decision == Decision::Stop && state.requires_browserstack_release() {
                        crate::cloud_runtime::settings::validate_ssh_identity(&self.settings.ssh_identity_file)?;
                    }
                }
            }
        }
        Ok(())
    }
    fn inspecting(&mut self, inspecting: bool) {
        self.inspecting = inspecting;
    }

    fn run(&mut self, store: &Store, decision: Decision, action: Action) -> Result<Phase> {
        // Evidence for this run only: an earlier uncertain operation keeps the intent
        // uncertain, so this target only reconciles until that is settled.
        let pending = Cell::new(false);
        let observe = |next: Mutation| {
            let prior = if pending.get() {
                Mutation::Pending
            } else {
                Mutation::Settled
            };
            pending.set(next == Mutation::Pending);
            Ok(prior)
        };
        self.settled = false;
        let result = self.execute(store, decision, action, &observe);
        self.pending = pending.get();
        result
    }

    fn mutation_uncertain(&self) -> bool {
        self.pending
    }

    fn uncertainty_settled(&self) -> bool {
        self.settled
    }

    fn verify_access(&mut self, request: &Request<'_>) -> Result<Phase> {
        renew_released_grant(request)?;
        let snapshot = super::super::refresh(
            &super::super::Request {
                root: request.root.into(),
                owner: request.owner.clone(),
                context: Some(request.context.clone()),
                action: super::super::Action::Refresh,
                settings: self.settings.clone(),
            },
            self.cancel,
        )?;
        Ok(
            if snapshot.rows.iter().any(|row| {
                row.companion.alias == request.alias
                    && row.companion.selected
                    && row.companion.status == super::super::Status::Ready
                    && row.companion.access.is_some()
                    && row.error.is_none()
            }) {
                Phase::Ready
            } else {
                Phase::ReconcileRequired
            },
        )
    }
}

/// Whether a resume is durably settled and its reconnect not yet done: no stop is
/// requested, and either the bound worker runs without being ready, or a Hetzner
/// record whose fence Resume cleared keeps a volume a server has held.
pub(super) fn resume_settled(store: &Store, reconciled: &lifecycle::ReconciledDeployment) -> Result<bool> {
    let state = &reconciled.state;
    if state.stop_requested || state.worker_ready() {
        return Ok(false);
    }
    Ok(match &state.operation {
        CreateState::Bound { .. } => reconciled
            .report
            .worker
            .as_ref()
            .is_some_and(|worker| worker.status() == horizon_cloud::WorkerStatus::Running),
        CreateState::Prepared => resumed_on_a_new_server(store, state)?,
        _ => false,
    })
}

impl Live<'_> {
    /// Reconnects the target through the ordinary deployment, which never allocates a
    /// replacement for an existing worker.
    fn reconnect(&self, store: &Store, observe: Observer<'_>) -> Result<Phase> {
        let state = store
            .load()?
            .ok_or(Error::Invalid("Companion deployment disappeared"))?;
        // Both the refusal and execution use the same operation.lock. Deleted
        // records can never reach deploy's explicit redeploy/reopen path.
        if deletion_pending(store, &state)? {
            return Ok(Phase::Refused);
        }
        let request = deployment::Request::new(
            state.cloud_id,
            state.repository,
            state.revision,
            state.profile,
            store.root().into(),
            self.settings.clone(),
        );
        let state = deployment::deploy_locked(&request, &[], store, |_, _| Ok(()), self.cancel, self.emit, observe)?;
        Ok(if state.worker_ready() {
            Phase::Ready
        } else {
            Phase::ReconcileRequired
        })
    }

    fn execute(&mut self, store: &Store, decision: Decision, action: Action, observe: Observer<'_>) -> Result<Phase> {
        match decision {
            Decision::Refuse(_) => Ok(Phase::Refused),
            Decision::Provision => Ok(Phase::ConfirmationRequired),
            Decision::Reuse | Decision::VerifyAccess => Ok(Phase::Ready),
            Decision::AlreadyStopped => {
                // A workerless prepared record has nothing to stop; marking it would
                // strand a Hetzner resume whose fence is already cleared.
                if let Some(mut state) = store.load()?
                    && state.operation != CreateState::Prepared
                {
                    state.stop_requested = true;
                    store.save(&state)?;
                }
                Ok(Phase::Stopped)
            }
            Decision::Stop => {
                lifecycle::stop_locked(store, self.settings, self.cancel, observe)?;
                Ok(Phase::Stopped)
            }
            Decision::ResumeThenReconnect | Decision::ResumeWithNewServer | Decision::Reconnect => {
                if matches!(decision, Decision::ResumeThenReconnect | Decision::ResumeWithNewServer) {
                    if decision == Decision::ResumeWithNewServer {
                        let checked = lifecycle::reconcile_locked(store, self.settings, None, self.cancel)?;
                        super::receipt::released(store, &checked)?;
                        (self.emit)(Event::Output(
                            "Resuming allocates a new Hetzner server on the retained workspace volume".into(),
                        ));
                    }
                    lifecycle::resume_locked(store, self.settings, self.cancel, observe)?;
                }
                self.reconnect(store, observe)
            }
            Decision::ReconcileOnly => {
                let state = store
                    .load()?
                    .ok_or(Error::Invalid("Reconcile the missing companion deployment"))?;
                if state.spec.is_none() {
                    return Ok(Phase::ReconcileRequired);
                }
                let reconciled = lifecycle::reconcile_locked(store, self.settings, None, self.cancel)?;
                if intent::refuses_deployment(&reconciled.state) {
                    return Ok(Phase::Refused);
                }
                if action == Action::Stop && reconciled.confirmed_stopped() {
                    return Ok(Phase::Stopped);
                }
                // A crash after a resume settled and before its reconnect: the resume
                // is proven by the record, so the reconnect it led to runs now.
                // Only a re-entered operation, whose receipt is not `Inspecting`, may
                // reconnect: an inspection must stay read-only across a crash.
                if !self.inspecting && action == Action::EnsureReady && resume_settled(store, &reconciled)? {
                    self.settled = true;
                    return self.reconnect(store, observe);
                }
                // A crash before the resume this Ensure Ready authorized: the provider
                // confirms the worker still stopped, so the resume never applied and
                // runs now, then its reconnect.
                if !self.inspecting
                    && action == Action::EnsureReady
                    && let Some(resume) = resume_to_continue(&reconciled)
                {
                    self.settled = true;
                    return self.execute(store, resume, action, observe);
                }
                if matches!(
                    reconciled.report.outcome,
                    horizon_cloud::runpod::recovery::Outcome::Missing { .. }
                ) {
                    return Ok(Phase::Refused);
                }
                Ok(
                    if action == Action::EnsureReady
                        && reconciled.report.worker.is_some()
                        && reconciled.state.worker_ready()
                    {
                        Phase::Ready
                    } else {
                        Phase::ReconcileRequired
                    },
                )
            }
        }
    }
}

/// Only a provider-confirmed released server can change an established grant's
/// worker pin. Keep the same grant ID, source key, revision and worktree.
pub(super) fn renew_released_grant(request: &Request<'_>) -> Result<()> {
    let (source, mut state) = request.load()?;
    let binding = request.authorize(&state)?;
    let target = request.target_store(&binding)?;
    let deployed = target
        .load()?
        .ok_or(Error::Invalid("Companion deployment disappeared"))?;
    if !deployed.worker_ready() || deletion_pending(&target, &deployed)? {
        return Err(Error::Invalid("Companion is not ready for access verification"));
    }
    let grant = state
        .grants
        .get_mut(request.alias)
        .ok_or(Error::Invalid("Companion is no longer selected"))?;
    let worker = deployed
        .worker
        .as_ref()
        .ok_or(Error::Invalid("Companion worker disappeared"))?;
    if grant.target_worker.as_ref().is_some_and(|old| old != &worker.id)
        && deployed.profile.provider == horizon_cloud::hetzner::PROVIDER
        && super::receipt::load(target.root())?.is_some_and(|record| {
            grant
                .target_worker
                .as_ref()
                .is_some_and(|old| record.released_workers.contains(old))
        })
    {
        grant.target_worker = Some(worker.id.clone());
        grant.target_revoked = true;
        grant.access = None;
        source.save(&state)?;
    }
    Ok(())
}

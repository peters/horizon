//! Creating a companion cloud an agent asked for, only once the owner confirms it on
//! the source cloud's card. Until then nothing is reserved, bound or allocated, so an
//! unanswered request is forgotten when Horizon closes.
use super::{HorizonApp, Owner, retry_busy};
use horizon_core::{
    cloud_panel::{CloudGroups, CloudLaunch, Placement},
    cloud_runtime::{
        self, Cancellation,
        companions::{
            self, Context, Declaration, Target,
            intent::{Action, Binding, Origin},
            inventory::{self, Checkout},
            lifecycle,
        },
        settings::Settings,
        state::OperationId,
    },
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
};

/// At most this many directories are read when looking for a matching checkout.
const SEARCHED: usize = 32;
/// Declined operation IDs remembered so an agent's poll gets a definite answer.
const DECLINED: usize = 256;

#[derive(Default)]
pub(in crate::app::cloud_panel::production::companions) struct State {
    pending: Vec<Pending>,
    /// Operation IDs are scoped per source journal, so a decline names its source
    /// cloud and alias too.
    /// The target cloud is kept when its card was already added and stays unstarted.
    declined: VecDeque<Declined>,
    /// A source cloud and alias whose checkout the owner wants to choose.
    picker: Option<(String, String)>,
    actions: Vec<(String, String, Choice)>,
}

struct Declined {
    source: String,
    alias: String,
    id: OperationId,
    kept: Option<String>,
}

struct Pending {
    source: String,
    alias: String,
    owner: Owner,
    declaration: Declaration,
    id: OperationId,
    checkouts: Vec<PathBuf>,
    search: Option<Receiver<Vec<PathBuf>>>,
    chosen: Option<PathBuf>,
    error: Option<String>,
    step: Step,
    /// The cloud ID minted at the first Create, reused by every retry.
    cloud_id: Option<String>,
    /// The checkout, once the reservation is recorded.
    checkout: Option<Checkout>,
    /// The new cloud's card, once added.
    card: Option<u32>,
    /// A cloud already created but never started, rather than a missing one.
    existing: bool,
}

enum Step {
    Waiting,
    Reserving(Receiver<Result<Checkout, String>>),
    Selecting {
        target: String,
        receiver: Receiver<Result<Context, String>>,
    },
    /// Confirmed; starts once no uncheck of the source is still being saved, so a
    /// clicked uncheck always reaches the journal before any allocation.
    Starting {
        target: String,
        context: Box<Context>,
    },
    /// The recorded operation is being cancelled; the decline is final once it is.
    Declining(Receiver<Result<(), String>>),
}

enum Progress {
    Reserved(Result<Checkout, String>),
    Selected(Result<Context, String>),
    Declined(Result<(), String>),
    Start,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    Create,
    Decline,
    Browse,
}

impl State {
    fn find(&mut self, source: &str, alias: &str) -> Option<&mut Pending> {
        self.pending
            .iter_mut()
            .find(|pending| pending.source == source && pending.alias == alias)
    }

    /// The answer to a request about a companion awaiting creation, or `None` when
    /// the request is not about one.
    pub(super) fn answer(
        &self,
        source: &str,
        alias: &str,
        action: Option<Action>,
        id: OperationId,
    ) -> Option<Result<Value, String>> {
        let pending = self
            .pending
            .iter()
            .find(|pending| pending.source == source && pending.alias == alias);
        match (action, pending) {
            // A declined request stays declined, whether polled or sent again.
            (None | Some(Action::EnsureReady), _)
                if let Some(declined) = self
                    .declined
                    .iter()
                    .find(|d| d.source == source && d.alias == alias && d.id == id) =>
            {
                Some(Ok(json!({
                    "operation_id": id,
                    "action": "ensure_ready",
                    "cloud": source,
                    "alias": alias,
                    "target_cloud_id": declined.kept,
                    "phase": "refused",
                    "done": true,
                    "resend": false,
                    "message": if declined.kept.is_some() {
                        "The owner declined starting this companion cloud; its card stays, unstarted, for the owner to start or remove"
                    } else {
                        "The owner declined creating this companion cloud; nothing was created"
                    },
                })))
            }
            (None, Some(pending)) if pending.id == id => Some(Ok(pending.describe())),
            (Some(Action::EnsureReady), Some(pending)) => Some(Ok(pending.describe())),
            (Some(Action::Stop), Some(_)) => Some(Err(
                "cloud_companion_unavailable: this companion has no cloud to stop yet".into(),
            )),
            _ => None,
        }
    }

    /// Makes a decline final: the request leaves the card and its polls answer refused.
    fn finish_decline(&mut self, index: usize) {
        let pending = self.pending.remove(index);
        if self.declined.len() >= DECLINED {
            self.declined.pop_front();
        }
        self.declined.push_back(Declined {
            source: pending.source,
            alias: pending.alias,
            id: pending.id,
            kept: pending.card.and(pending.cloud_id),
        });
    }

    /// Discards every pending creation, as a session change does. Returns the sources
    /// whose refresh a creation in progress was holding.
    pub(in crate::app::cloud_panel::production::companions::agent) fn discard(&mut self) -> Vec<String> {
        self.actions.clear();
        self.picker = None;
        self.pending
            .drain(..)
            .filter(|pending| {
                matches!(
                    pending.step,
                    Step::Reserving(_) | Step::Selecting { .. } | Step::Starting { .. }
                )
            })
            .map(|pending| pending.source)
            .collect()
    }
}

impl Pending {
    fn waiting(&self) -> bool {
        matches!(self.step, Step::Waiting)
    }
}

impl HorizonApp {
    /// Records an agent's request to create a missing companion and answers it. The
    /// owner decides on the source cloud's card; repeated requests share its ID.
    pub(super) fn request_companion_creation(
        &mut self,
        owner: Owner,
        alias: &str,
        declaration: Declaration,
        id: OperationId,
    ) -> Value {
        let source = owner.cloud_id.clone();
        let creation = &mut self.cloud_prototype.production.companions.agent.creation;
        if let Some(pending) = creation.find(&source, alias) {
            return pending.describe();
        }
        let directories = self.checkout_candidates(&owner.scope.workspace_id);
        let (sender, receiver) = channel();
        let wanted = declaration.clone();
        std::thread::spawn(move || {
            let cancel = Cancellation::default();
            let mut found: Vec<PathBuf> = Vec::new();
            for directory in directories {
                if let Ok(checkout) = inventory::checkout(&directory, &wanted, &cancel)
                    && !found.contains(&checkout.repository)
                {
                    found.push(checkout.repository);
                }
            }
            let _ = sender.send(found);
        });
        let pending = Pending {
            source,
            alias: alias.to_owned(),
            owner,
            declaration,
            id,
            checkouts: Vec::new(),
            search: Some(receiver),
            chosen: None,
            error: None,
            step: Step::Waiting,
            cloud_id: None,
            checkout: None,
            card: None,
            existing: false,
        };
        let answer = pending.describe();
        self.cloud_prototype
            .production
            .companions
            .agent
            .creation
            .pending
            .push(pending);
        answer
    }

    pub(in crate::app) fn choose_companion_checkout(&mut self, source: &str, alias: &str, path: &Path) {
        if let Some(pending) = self
            .cloud_prototype
            .production
            .companions
            .agent
            .creation
            .find(source, alias)
            .filter(|pending| pending.waiting() && pending.checkout.is_none())
        {
            pending.chosen = Some(path.to_owned());
            pending.error = None;
        }
    }

    /// Applies the owner's choices and advances each confirmed creation.
    pub(super) fn poll_companion_creations(&mut self, ctx: &egui::Context) {
        let creation = &mut self.cloud_prototype.production.companions.agent.creation;
        for pending in &mut creation.pending {
            if let Some(Ok(found)) = pending.search.as_ref().map(Receiver::try_recv) {
                pending.search = None;
                if pending.chosen.is_none() && found.len() == 1 {
                    pending.chosen = found.first().cloned();
                }
                pending.checkouts = found;
            }
        }
        for (source, alias, choice) in std::mem::take(&mut creation.actions) {
            let creation = &mut self.cloud_prototype.production.companions.agent.creation;
            match choice {
                Choice::Decline => self.decline_companion(&source, &alias),
                Choice::Browse => creation.picker = Some((source, alias)),
                Choice::Create => self.reserve_companion(&source, &alias, ctx),
            }
        }
        self.open_companion_checkout_picker();
        let clearing = |source: &str| {
            self.cloud_prototype
                .production
                .companions
                .entries
                .get(source)
                .is_some_and(|entry| !entry.clearing.is_empty() || entry.job.is_some())
        };
        let mut steps = Vec::new();
        for (index, pending) in self
            .cloud_prototype
            .production
            .companions
            .agent
            .creation
            .pending
            .iter()
            .enumerate()
        {
            let progress = match &pending.step {
                Step::Waiting => None,
                Step::Reserving(receiver) => receiver.try_recv().ok().map(Progress::Reserved),
                Step::Selecting { receiver, .. } => receiver.try_recv().ok().map(Progress::Selected),
                Step::Declining(receiver) => receiver.try_recv().ok().map(Progress::Declined),
                Step::Starting { .. } => (!clearing(&pending.source)).then_some(Progress::Start),
            };
            steps.extend(progress.map(|progress| (index, progress)));
        }
        // Later indexes first, so removing one leaves the earlier indexes valid.
        for (index, progress) in steps.into_iter().rev() {
            let creation = &mut self.cloud_prototype.production.companions.agent.creation;
            match progress {
                Progress::Reserved(Ok(checkout)) => {
                    creation.pending[index].checkout = Some(checkout);
                    self.add_companion_cloud(index, ctx);
                }
                Progress::Selected(Ok(context)) => {
                    let pending = &mut creation.pending[index];
                    if let Step::Selecting { target, .. } = &pending.step {
                        pending.step = Step::Starting {
                            target: target.clone(),
                            context: Box::new(context),
                        };
                    }
                }
                Progress::Start => self.run_companion_creation(index, ctx),
                Progress::Declined(Ok(())) => creation.finish_decline(index),
                Progress::Declined(Err(error)) => {
                    let pending = &mut creation.pending[index];
                    pending.step = Step::Waiting;
                    pending.error = Some(format!("The decline was not saved: {error}; decline again"));
                }
                Progress::Reserved(Err(error)) | Progress::Selected(Err(error)) => {
                    self.fail_companion_creation(index, error);
                }
            }
        }
        if self
            .cloud_prototype
            .production
            .companions
            .agent
            .creation
            .pending
            .iter()
            .any(|pending| !pending.waiting() || pending.search.is_some())
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    /// Declines a waiting request. Nothing recorded yet: final at once. Otherwise the
    /// recorded operation is cancelled first, and the decline is final only once that
    /// is saved, so a restart cannot offer a declined request again.
    fn decline_companion(&mut self, source: &str, alias: &str) {
        let creation = &mut self.cloud_prototype.production.companions.agent.creation;
        let Some(index) = creation
            .pending
            .iter()
            .position(|pending| pending.source == source && pending.alias == alias && pending.waiting())
        else {
            return;
        };
        if creation.pending[index].cloud_id.is_none() {
            creation.finish_decline(index);
            return;
        }
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let groups = self.cloud_prototype.groups.clone();
        let pending = &mut creation.pending[index];
        let (owner, alias, id) = (pending.owner.clone(), pending.alias.clone(), pending.id);
        let (sender, receiver) = channel();
        pending.error = None;
        pending.step = Step::Declining(receiver);
        std::thread::spawn(move || {
            let _ = sender.send(cancel(&root, &owner, &groups, &alias, id));
        });
    }

    fn open_companion_checkout_picker(&mut self) {
        if self.dir_picker.is_some() {
            return;
        }
        let creation = &mut self.cloud_prototype.production.companions.agent.creation;
        let Some((source, alias)) = creation.picker.take() else {
            return;
        };
        let seed = creation
            .find(&source, &alias)
            .and_then(|pending| pending.chosen.clone());
        self.dir_picker = Some(crate::dir_picker::DirPicker::with_seed(
            crate::dir_picker::DirPickerPurpose::CompanionCheckout { source, alias },
            seed.as_deref(),
        ));
    }

    /// Owner confirmed: reads the checkout, then reserves a fresh cloud ID bound to it
    /// and records the agent's operation, still without allocating anything.
    fn reserve_companion(&mut self, source: &str, alias: &str, ctx: &egui::Context) {
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let groups = self.cloud_prototype.groups.clone();
        let agent = &mut self.cloud_prototype.production.companions.agent;
        let Some(index) = agent.creation.pending.iter().position(|pending| {
            pending.source == source && pending.alias == alias && pending.waiting() && pending.chosen.is_some()
        }) else {
            return;
        };
        agent.hold(source);
        let pending = &mut agent.creation.pending[index];
        pending.error = None;
        // A retry after the reservation was recorded continues from the card.
        if pending.checkout.is_some() {
            self.add_companion_cloud(index, ctx);
            return;
        }
        let Some(directory) = pending.chosen.clone() else {
            return;
        };
        let cloud_id = pending.cloud_id.get_or_insert_with(cloud_runtime::new_id).clone();
        let (owner, declaration, id) = (pending.owner.clone(), pending.declaration.clone(), pending.id);
        let alias = alias.to_owned();
        let (sender, receiver) = channel();
        pending.step = Step::Reserving(receiver);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let reserved = reserve(
                &root,
                &owner,
                &groups,
                (&alias, &declaration),
                &directory,
                &cloud_id,
                id,
            );
            let _ = sender.send(reserved);
            ctx.request_repaint();
        });
    }

    /// Adds the reserved cloud as an ordinary cloud card and saves its prepared record,
    /// then selects it for the source and records the owner's confirmation.
    fn add_companion_cloud(&mut self, index: usize, ctx: &egui::Context) {
        let pending = &self.cloud_prototype.production.companions.agent.creation.pending[index];
        let (owner, alias, id) = (pending.owner.clone(), pending.alias.clone(), pending.id);
        let (Some(cloud_id), Some(checkout)) = (pending.cloud_id.clone(), pending.checkout.clone()) else {
            return;
        };
        let (card, profile_name) = (pending.card, pending.declaration.profile.clone());
        let added = (|| {
            let card = if let Some(card) = card {
                card
            } else {
                let launch = CloudLaunch {
                    deployment_started: false,
                    id: cloud_id.clone(),
                    revision: checkout.revision,
                    profile_name,
                    placement: Placement::default().for_profile(checkout.profile.gpu),
                    profile: checkout.profile,
                };
                let card = self
                    .add_cloud_group(
                        alias.clone(),
                        owner.scope.workspace_id.clone(),
                        checkout.repository,
                        launch,
                        Vec::new(),
                    )
                    .map_err(|error| error.to_string())?;
                self.cloud_prototype.production.companions.agent.creation.pending[index].card = Some(card);
                card
            };
            self.prepare_production_deployment(card)
                .ok_or("The new companion cloud could not be saved; its card shows why")?;
            Ok::<_, String>(cloud_id)
        })();
        let target = match added {
            Ok(target) => target,
            Err(error) => {
                self.fail_companion_creation(index, error);
                return;
            }
        };
        let Some(root) = self.cloud_prototype.root.clone() else {
            return;
        };
        let groups = self.cloud_prototype.groups.clone();
        let (sender, receiver) = channel();
        self.cloud_prototype.production.companions.agent.creation.pending[index].step = Step::Selecting {
            target: target.clone(),
            receiver,
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(select_and_confirm(&root, &owner, &groups, &alias, &target, id));
            ctx.request_repaint();
        });
    }

    /// Runs the confirmed operation on the new cloud's card, which allocates its first worker.
    fn run_companion_creation(&mut self, index: usize, ctx: &egui::Context) {
        let pending = &mut self.cloud_prototype.production.companions.agent.creation.pending[index];
        let Step::Starting { target, context } = std::mem::replace(&mut pending.step, Step::Waiting) else {
            return;
        };
        let (source, alias, id) = (pending.source.clone(), pending.alias.clone(), pending.id);
        match self.execute_on_card(&source, &target, (&alias, false), id, *context, ctx) {
            // The execution keeps the source's hold until it reports back.
            Ok(()) => {
                self.cloud_prototype
                    .production
                    .companions
                    .agent
                    .creation
                    .pending
                    .remove(index);
            }
            Err(error) => self.fail_companion_creation(index, error),
        }
    }

    /// Pauses a confirmed creation that could not go on. What is already recorded, the
    /// reservation and the card, stays, so the owner's Retry continues from there and
    /// Decline cancels the unstarted operation.
    fn fail_companion_creation(&mut self, index: usize, error: String) {
        let pending = &mut self.cloud_prototype.production.companions.agent.creation.pending[index];
        pending.step = Step::Waiting;
        pending.error = Some(error);
        let source = pending.source.clone();
        self.release_companion_source(&source);
    }
}

mod offers;
mod steps;
mod view;

use steps::{cancel, reserve, select_and_confirm};

#[cfg(test)]
mod tests;

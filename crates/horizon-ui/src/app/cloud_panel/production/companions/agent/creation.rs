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
    declined: VecDeque<(String, String, OperationId)>,
    /// A source cloud and alias whose checkout the owner wants to choose.
    picker: Option<(String, String)>,
    actions: Vec<(String, String, Choice)>,
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
                if self
                    .declined
                    .iter()
                    .any(|(s, a, declined)| s == source && a == alias && *declined == id) =>
            {
                Some(Ok(json!({
                    "operation_id": id,
                    "action": "ensure_ready",
                    "cloud": source,
                    "alias": alias,
                    "target_cloud_id": null,
                    "phase": "refused",
                    "done": true,
                    "resend": false,
                    "message": "The owner declined creating this companion cloud; nothing was created",
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

    /// Forgets a declined request. Returns it when its operation was already recorded,
    /// so the caller cancels that unstarted operation.
    fn decline(&mut self, source: &str, alias: &str) -> Option<Pending> {
        let index = self
            .pending
            .iter()
            .position(|pending| pending.source == source && pending.alias == alias && pending.waiting())?;
        let pending = self.pending.remove(index);
        if self.declined.len() >= DECLINED {
            self.declined.pop_front();
        }
        self.declined
            .push_back((pending.source.clone(), pending.alias.clone(), pending.id));
        pending.cloud_id.is_some().then_some(pending)
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

    /// Asks the owner to start a checked companion whose cloud was created but never
    /// started, as a creation whose card already exists. `None` when its card is not
    /// open here, and the agent is told to have it started from its card.
    pub(super) fn request_companion_start(
        &mut self,
        source: &str,
        alias: &str,
        operation: &lifecycle::Operation,
        context: &Context,
    ) -> Option<Value> {
        let target = &operation.intent.target_cloud_id;
        let group = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.remote.as_ref().is_some_and(|launch| &launch.id == target))?;
        let launch = group.remote.as_ref()?;
        let owner = self
            .cloud_prototype
            .production
            .companions
            .entries
            .get(source)?
            .owner
            .clone();
        let declaration = context.declarations.get(alias)?.clone();
        let creation = &mut self.cloud_prototype.production.companions.agent.creation;
        if let Some(pending) = creation.find(source, alias) {
            return Some(pending.describe());
        }
        let pending = Pending {
            source: source.to_owned(),
            alias: alias.to_owned(),
            owner,
            declaration,
            id: operation.intent.operation_id,
            checkouts: Vec::new(),
            search: None,
            chosen: Some(group.cwd.clone()),
            error: None,
            step: Step::Waiting,
            cloud_id: Some(target.clone()),
            checkout: Some(Checkout {
                repository: group.cwd.clone(),
                revision: launch.revision.clone(),
                profile: launch.profile.clone(),
            }),
            card: Some(group.issue),
            existing: true,
        };
        let answer = pending.describe();
        creation.pending.push(pending);
        Some(answer)
    }

    /// Directories in the workspace that may be checkouts: its own, its panels' and
    /// its clouds'. Each is read off the UI thread.
    fn checkout_candidates(&self, workspace: &str) -> Vec<PathBuf> {
        let mut directories: Vec<PathBuf> = Vec::new();
        let id = self.board.workspace_id_by_local_id(workspace);
        let cwd = id.and_then(|id| self.board.workspace(id)).and_then(|w| w.cwd.clone());
        let panels = self
            .board
            .panels
            .iter()
            .filter(|panel| Some(panel.workspace_id) == id)
            .filter_map(|panel| panel.launch_cwd.clone());
        let clouds = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .filter(|group| group.workspace == workspace)
            .map(|group| group.cwd.clone());
        for directory in cwd.into_iter().chain(panels).chain(clouds) {
            if !directories.contains(&directory) && directories.len() < SEARCHED {
                directories.push(directory);
            }
        }
        directories
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
                Choice::Decline => {
                    if let (Some(pending), Some(root)) =
                        (creation.decline(&source, &alias), self.cloud_prototype.root.clone())
                    {
                        let groups = self.cloud_prototype.groups.clone();
                        std::thread::spawn(move || cancel(&root, &pending.owner, &groups, &pending.alias, pending.id));
                    }
                }
                Choice::Browse => creation.picker = Some((source, alias)),
                Choice::Create => self.reserve_companion(&source, &alias, ctx),
            }
        }
        self.open_companion_checkout_picker();
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
            match &pending.step {
                Step::Waiting => {}
                Step::Reserving(receiver) => {
                    if let Ok(result) = receiver.try_recv() {
                        steps.push((index, Some(result), None));
                    }
                }
                Step::Selecting { receiver, .. } => {
                    if let Ok(result) = receiver.try_recv() {
                        steps.push((index, None, Some(result)));
                    }
                }
            }
        }
        // Later indexes first, so removing one leaves the earlier indexes valid.
        for (index, reserved, selected) in steps.into_iter().rev() {
            if let Some(result) = reserved {
                match result {
                    Ok(checkout) => {
                        self.cloud_prototype.production.companions.agent.creation.pending[index].checkout =
                            Some(checkout);
                        self.add_companion_cloud(index, ctx);
                    }
                    Err(error) => self.fail_companion_creation(index, error),
                }
            } else if let Some(result) = selected {
                self.run_companion_creation(index, result, ctx);
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
    fn run_companion_creation(&mut self, index: usize, result: Result<Context, String>, ctx: &egui::Context) {
        let pending = &self.cloud_prototype.production.companions.agent.creation.pending[index];
        let Step::Selecting { target, .. } = &pending.step else {
            return;
        };
        let (source, alias, id, target) = (
            pending.source.clone(),
            pending.alias.clone(),
            pending.id,
            target.clone(),
        );
        let started =
            result.and_then(|context| self.execute_on_card(&source, &target, (&alias, false), id, context, ctx));
        match started {
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

/// Idempotent for the same cloud ID, checkout and operation, so a retry after a
/// partial failure records nothing twice.
fn reserve(
    root: &Path,
    owner: &Owner,
    groups: &CloudGroups,
    (alias, declaration): (&str, &Declaration),
    directory: &Path,
    cloud_id: &str,
    id: OperationId,
) -> Result<Checkout, String> {
    let cancel = Cancellation::default();
    let checkout = inventory::checkout(directory, declaration, &cancel).map_err(|error| error.to_string())?;
    let context = inventory::prepare(owner, groups, &cancel).map_err(|error| error.to_string())?;
    if context.declarations.get(alias) != Some(declaration) {
        return Err("The companion's declaration changed; the agent must ask again".into());
    }
    let request = lifecycle::Request {
        root,
        owner,
        context: &context,
        alias,
    };
    let target = Target {
        scope: owner.scope.clone(),
        cloud_id: cloud_id.to_owned(),
        declaration: declaration.clone(),
    };
    let binding = Binding::new(owner, alias, target, checkout.repository.clone(), Origin::Reserved)
        .map_err(|error| error.to_string())?;
    // A retry finds the binding and operation already recorded: the target then has a
    // claim, so binding again is refused while submitting again returns the operation.
    let recorded = retry_busy(|| {
        let bound = lifecycle::bind(&request, binding.clone());
        lifecycle::submit(&request, Action::EnsureReady, id).or_else(|error| bound.and(Err(error)))
    })
    .map_err(|error| error.to_string())?;
    if recorded.intent.target_cloud_id != cloud_id {
        return Err("This companion is already bound to another cloud; nothing was reserved".into());
    }
    Ok(checkout)
}

fn select_and_confirm(
    root: &Path,
    owner: &Owner,
    groups: &CloudGroups,
    alias: &str,
    target: &str,
    id: OperationId,
) -> Result<Context, String> {
    let cancel = Cancellation::default();
    let context = inventory::prepare(owner, groups, &cancel).map_err(|error| error.to_string())?;
    let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
    // The selection is saved before any SSH work; the new cloud has no worker to reach yet.
    let selected = retry_busy(|| {
        companions::refresh(
            &companions::Request {
                root: root.to_owned(),
                owner: owner.clone(),
                context: Some(context.clone()),
                action: companions::Action::Select {
                    alias: alias.to_owned(),
                    target_cloud_id: target.to_owned(),
                },
                settings: settings.clone(),
            },
            &cancel,
        )
    });
    if let Err(error) = selected {
        tracing::info!(%error, "selected the new companion cloud before it has a worker");
    }
    retry_busy(|| {
        lifecycle::confirm_creation(
            &lifecycle::Request {
                root,
                owner,
                context: &context,
                alias,
            },
            id,
        )
    })
    .map_err(|error| error.to_string())?;
    Ok(context)
}

fn cancel(root: &Path, owner: &Owner, groups: &CloudGroups, alias: &str, id: OperationId) {
    let Ok(context) = inventory::prepare(owner, groups, &Cancellation::default()) else {
        return;
    };
    let request = lifecycle::Request {
        root,
        owner,
        context: &context,
        alias,
    };
    // Nothing to cancel when the failure came before the operation was recorded.
    let _ = retry_busy(|| lifecycle::cancel_submission(&request, id));
}

mod view;

#[cfg(test)]
mod tests;

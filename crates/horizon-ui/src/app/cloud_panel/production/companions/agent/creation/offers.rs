//! Requests the owner decides on the source cloud's card besides a plain creation: a
//! checked cloud that never started, and a reservation Horizon closed before finishing.
use super::{Action, Checkout, Context, HorizonApp, PathBuf, Pending, SEARCHED, Step, Value, lifecycle};

impl HorizonApp {
    /// Asks the owner to start a checked companion whose cloud was created but never
    /// started, or to restore its recoverable reserved card before starting it.
    /// `None` when neither offer is available.
    pub(in crate::app::cloud_panel::production::companions::agent) fn request_companion_start(
        &mut self,
        source: &str,
        alias: &str,
        operation: &lifecycle::Operation,
        context: &Context,
    ) -> Option<Value> {
        let target = &operation.intent.target_cloud_id;
        let Some(group) = self
            .cloud_prototype
            .groups
            .0
            .iter()
            .find(|group| group.remote.as_ref().is_some_and(|launch| &launch.id == target))
        else {
            return self.request_companion_recovery(source, alias, operation, context);
        };
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
            recorded: true,
        };
        let answer = pending.describe();
        creation.pending.push(pending);
        Some(answer)
    }

    /// Offers a reservation again when its card is missing: Horizon closed before adding
    /// it, or it was removed while only prepared. The operation waits, and the owner's
    /// Retry adds the card again from the checkout the binding recorded.
    pub(in crate::app::cloud_panel::production::companions::agent) fn request_companion_recovery(
        &mut self,
        source: &str,
        alias: &str,
        operation: &lifecycle::Operation,
        context: &Context,
    ) -> Option<Value> {
        let target = &operation.intent.target_cloud_id;
        if operation.intent.action != Action::EnsureReady
            || !operation.intent.state.pending()
            || self
                .cloud_prototype
                .groups
                .0
                .iter()
                .any(|group| group.remote.as_ref().is_some_and(|launch| &launch.id == target))
        {
            return None;
        }
        let root = self.cloud_prototype.root.clone()?;
        let owner = self
            .cloud_prototype
            .production
            .companions
            .entries
            .get(source)?
            .owner
            .clone();
        let declaration = context.declarations.get(alias)?.clone();
        if let Some(pending) = self
            .cloud_prototype
            .production
            .companions
            .agent
            .creation
            .find(source, alias)
        {
            return Some(pending.describe());
        }
        let request = lifecycle::Request {
            root: &root,
            owner: &owner,
            context,
            alias,
        };
        // Its card may also have been removed while still only prepared; a record that
        // could own a worker or provider resources is never offered again.
        if !lifecycle::card_recoverable(&request).unwrap_or(false) {
            return None;
        }
        let checkout = lifecycle::bound_checkout(&request).ok().flatten()?;
        let pending = Pending {
            source: source.to_owned(),
            alias: alias.to_owned(),
            owner,
            declaration,
            id: operation.intent.operation_id,
            checkouts: vec![checkout.clone()],
            search: None,
            chosen: Some(checkout),
            error: None,
            step: Step::Waiting,
            cloud_id: Some(target.clone()),
            checkout: None,
            card: None,
            existing: false,
            recorded: true,
        };
        let answer = pending.describe();
        self.cloud_prototype
            .production
            .companions
            .agent
            .creation
            .pending
            .push(pending);
        Some(answer)
    }

    /// Directories in the workspace that may be checkouts: its own, its panels' and
    /// its clouds'. Each is read off the UI thread.
    pub(super) fn checkout_candidates(&self, workspace: &str) -> Vec<PathBuf> {
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
}

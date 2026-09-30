//! The background steps of a confirmed creation: reserving the cloud, then selecting
//! it and recording the confirmation. Each is idempotent for a retry.
use super::{
    Binding, Cancellation, Checkout, CloudGroups, Context, Declaration, OperationId, Origin, Owner, Path, Settings,
    Snapshot, State, Step, Target, cloud_runtime, companions, inventory, lifecycle, retry_busy,
};
use std::sync::mpsc::TryRecvError;

/// Idempotent for the same cloud ID, checkout and operation, so a retry after a
/// partial failure records nothing twice.
pub(super) fn reserve(
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
    // The prompt may have waited while the owner added a matching cloud; only this
    // request's own reserved cloud may match, as on a Retry.
    if context.inventory.iter().any(|target| {
        target.cloud_id != context.source.cloud_id
            && target.cloud_id != cloud_id
            && target.declaration.matches(declaration)
    }) {
        return Err(
            "A matching cloud now exists in this workspace; check it on this card instead of creating another".into(),
        );
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
    // One journal write binds the fresh identity and records the agent's operation. A
    // retry returns that operation; another cloud or checkout is refused, writing nothing.
    retry_busy(|| lifecycle::reserve(&request, binding.clone(), id)).map_err(|error| error.to_string())?;
    Ok(checkout)
}

/// The context a confirmed creation runs with, and the source's companions as the
/// selection left them when it returned them.
pub(super) type Selected = Box<(Context, Option<Snapshot>)>;

pub(super) fn select_and_confirm(
    root: &Path,
    owner: &Owner,
    groups: &CloudGroups,
    alias: &str,
    target: &str,
    id: OperationId,
    select: bool,
) -> Result<Selected, String> {
    let cancel = Cancellation::default();
    let context = inventory::prepare(owner, groups, &cancel).map_err(|error| error.to_string())?;
    let settings = Settings::load(&root.join("settings.json")).map_err(|error| error.to_string())?;
    // The selection is saved before any SSH work; the new cloud has no worker to reach yet.
    // A cloud that already existed is confirmed only under the selection the owner
    // still holds: selecting it again here could undo an uncheck made meanwhile.
    let selected = if select {
        retry_busy(|| {
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
        })
        .map(Some)
    } else {
        Ok(None)
    };
    let snapshot = match selected {
        Ok(snapshot) => snapshot,
        Err(error) => {
            tracing::info!(%error, "selected the new companion cloud before it has a worker");
            None
        }
    };
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
    Ok(Box::new((context, snapshot)))
}

/// Cancels a recorded, unstarted operation for a decline. Nothing recorded is nothing
/// to cancel; an operation that already started cannot be declined.
pub(super) fn cancel(
    root: &Path,
    owner: &Owner,
    groups: &CloudGroups,
    alias: &str,
    id: OperationId,
) -> Result<(), String> {
    let context = inventory::prepare(owner, groups, &Cancellation::default()).map_err(|error| error.to_string())?;
    let request = lifecycle::Request {
        root,
        owner,
        context: &context,
        alias,
    };
    match retry_busy(|| lifecycle::cancel_submission(&request, id)) {
        Err(cloud_runtime::Error::Invalid("Companion is not bound" | "Unknown companion operation")) | Ok(()) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

impl State {
    /// Discards every pending creation, as a session change does, keeping its running
    /// background step until that reports. Returns the sources whose refresh a creation
    /// in progress was holding.
    pub(in crate::app::cloud_panel::production::companions::agent) fn discard(&mut self) -> Vec<String> {
        self.actions.clear();
        self.picker = None;
        // A declined operation ID is only final within its session's journal.
        self.declined.clear();
        let mut held = Vec::new();
        for pending in self.pending.drain(..) {
            if matches!(
                pending.step,
                Step::Reserving(_) | Step::Selecting { .. } | Step::Starting { .. }
            ) {
                held.push(pending.source);
            }
            if matches!(
                pending.step,
                Step::Reserving(_) | Step::Selecting { .. } | Step::Declining(_)
            ) {
                self.draining.push(pending.step);
            }
        }
        held
    }

    /// Whether every step a session change left running has reported, so its journal
    /// writes, a decline's among them, are durable before Horizon closes.
    pub(in crate::app::cloud_panel::production::companions) fn drained(&mut self) -> bool {
        self.draining.retain(|step| match step {
            Step::Reserving(receiver) => matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
            Step::Selecting { receiver, .. } => matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
            Step::Declining(receiver) => matches!(receiver.try_recv(), Err(TryRecvError::Empty)),
            Step::Waiting | Step::Starting { .. } => false,
        });
        self.draining.is_empty()
    }
}

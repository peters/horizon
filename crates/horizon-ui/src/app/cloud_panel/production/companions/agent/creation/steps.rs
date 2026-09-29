//! The background steps of a confirmed creation: reserving the cloud, then selecting
//! it and recording the confirmation. Each is idempotent for a retry.
use super::{
    Binding, Cancellation, Checkout, CloudGroups, Context, Declaration, OperationId, Origin, Owner, Path, Settings,
    Snapshot, Target, cloud_runtime, companions, inventory, lifecycle, retry_busy,
};

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
) -> Result<Selected, String> {
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
    let snapshot = match selected {
        Ok(snapshot) => Some(snapshot),
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
    match retry_busy(|| lifecycle::status(&request, id)) {
        Err(cloud_runtime::Error::Invalid("Companion is not bound" | "Unknown companion operation")) => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(operation) if operation.intent.state == companions::intent::State::Submitted => {
            retry_busy(|| lifecycle::cancel_submission(&request, id)).map_err(|error| error.to_string())
        }
        Ok(operation) if operation.intent.state.pending() => {
            Err("The companion cloud is already starting; it can no longer be declined".into())
        }
        Ok(_) => Ok(()),
    }
}

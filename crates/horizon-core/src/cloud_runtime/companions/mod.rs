//! Local companion authorization and SSH reconciliation. No provider lifecycle operations.
pub mod inventory;
mod journal;
mod reconcile;
#[cfg(all(test, unix))]
mod tests;
mod transport;

use super::{Cancellation, Error, Result, settings::Settings};
pub use horizon_cloud::companions::{Declaration, Scope, Target};
use horizon_cloud::companions::{Selection, candidates};
use horizon_cloud_protocol::companion::VERSION;
pub use horizon_cloud_protocol::companion::{Catalog, Companion, Status};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Owner {
    pub scope: Scope,
    pub cloud_id: String,
}

#[derive(Clone, Debug)]
pub struct Context {
    pub source: Target,
    pub declarations: BTreeMap<String, Declaration>,
    pub inventory: Vec<Target>,
}

#[derive(Clone, Debug, Default)]
pub enum Action {
    #[default]
    Refresh,
    Select {
        alias: String,
        target_cloud_id: String,
    },
    Clear {
        alias: String,
    },
}

pub struct Request {
    pub root: PathBuf,
    pub owner: Owner,
    /// Missing or invalid repository configuration disables new grants but still permits revocation.
    pub context: Option<Context>,
    pub action: Action,
    pub settings: Settings,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub companion: Companion,
    pub candidates: Vec<Target>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub catalog: Catalog,
    pub rows: Vec<Row>,
    pub publication_error: Option<String>,
    pub notice: Option<String>,
}

/// # Errors
/// Refuses malformed ownership, conflicting controllers and corrupt journals before SSH changes.
pub fn refresh(request: &Request, cancel: &Cancellation) -> Result<Snapshot> {
    let mut transport = transport::Live::new(&request.root, &request.settings, cancel);
    refresh_with_transport(request, cancel, &mut transport)
}

fn refresh_with_transport(
    request: &Request,
    cancel: &Cancellation,
    transport: &mut impl transport::Transport,
) -> Result<Snapshot> {
    cancel.check()?;
    let journal = journal::Store::open(&request.root, &request.owner)?;
    let result = execute(
        &journal,
        &request.owner,
        request.context.as_ref(),
        &request.action,
        transport,
    );
    cancel.check()?;
    result
}

fn execute(
    journal: &journal::Store,
    owner: &Owner,
    context: Option<&Context>,
    action: &Action,
    transport: &mut impl transport::Transport,
) -> Result<Snapshot> {
    let mut state = journal.load()?;
    let moved = state.owner != *owner;
    if moved {
        for grant in state.grants.values_mut() {
            grant.selected = false;
        }
    } else {
        apply_action(&mut state, context, action)?;
    }
    journal.save(&state)?;
    let mut snapshot = reconcile::run(journal, &mut state, if moved { None } else { context }, transport)?;
    if moved {
        if state.grants.is_empty() {
            state.owner = owner.clone();
            journal.save(&state)?;
        }
        snapshot.notice = Some(
            "Cloud ownership changed; previous grants are being revoked. Select companions again after cleanup.".into(),
        );
    }
    Ok(snapshot)
}

fn apply_action(state: &mut journal::State, context: Option<&Context>, action: &Action) -> Result<()> {
    if let Some(context) = context {
        if context.source.scope != state.owner.scope || context.source.cloud_id != state.owner.cloud_id {
            return Err(Error::Invalid("Companion context belongs to another cloud"));
        }
        horizon_cloud::companions::validate_declarations(&context.declarations)
            .map_err(|_| Error::Invalid("Invalid companion declarations"))?;
    }
    match action {
        Action::Refresh => {}
        Action::Clear { alias } => {
            if let Some(grant) = state.grants.get_mut(alias) {
                grant.selected = false;
            }
        }
        Action::Select { alias, target_cloud_id } => {
            let context = context.ok_or(Error::Invalid("Companion configuration is unavailable"))?;
            let declaration = context
                .declarations
                .get(alias)
                .ok_or(Error::Invalid("Unknown companion alias"))?;
            let matching = candidates(&context.source, declaration, &context.inventory);
            let mut matching = matching
                .into_iter()
                .filter(|target| &target.cloud_id == target_cloud_id);
            let target = matching
                .next()
                .ok_or(Error::Invalid("Companion target is unavailable in this workspace"))?;
            if matching.next().is_some() {
                return Err(Error::Invalid("Companion target identity is ambiguous"));
            }
            let selection = Selection::new(&context.source, alias, target)
                .map_err(|_| Error::Invalid("Invalid companion selection"))?;
            if let Some(existing) = state.grants.get(alias) {
                if existing.selected && existing.selection == selection {
                    return Ok(());
                }
                return Err(Error::Invalid(
                    "Revoke the existing companion grant before selecting another target",
                ));
            }
            if state.grants.len() >= 64 {
                return Err(Error::Invalid(
                    "Finish pending companion revocations before adding more",
                ));
            }
            state.grants.insert(
                alias.clone(),
                journal::Grant {
                    selection,
                    target: target.clone(),
                    id: super::new_id(),
                    selected: true,
                    source_worker: None,
                    target_worker: None,
                    revision: None,
                    source_disconnected: true,
                    target_revoked: true,
                    access: None,
                },
            );
        }
    }
    Ok(())
}

fn catalog(source: &str, rows: &[Row]) -> Catalog {
    Catalog {
        version: VERSION,
        source_cloud_id: source.into(),
        observed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_secs()),
        companions: rows.iter().map(|row| row.companion.clone()).collect(),
    }
}

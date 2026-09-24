use super::{
    Catalog, Companion, Context, Error, Result, Row, Snapshot, Status, candidates, catalog,
    journal::{Grant, State, Store},
    transport::Transport,
};
use horizon_cloud_protocol::companion::{Access, Request, Response};
use std::collections::BTreeMap;

pub(super) fn run(
    store: &Store,
    state: &mut State,
    context: Option<&Context>,
    transport: &mut impl Transport,
) -> Result<Snapshot> {
    let mut rows = declaration_rows(context);
    for alias in state.grants.keys().cloned().collect::<Vec<_>>() {
        let mut grant = state
            .grants
            .get(&alias)
            .cloned()
            .ok_or(Error::Invalid("Missing companion grant"))?;
        let declaration_row = rows.get(&alias).cloned();
        let row = rows.entry(alias.clone()).or_insert_with(|| Row {
            companion: Companion {
                alias: alias.clone(),
                repository: grant.target.declaration.repository.clone(),
                profile: grant.target.declaration.profile.clone(),
                target_cloud_id: None,
                selected: false,
                status: Status::Changed,
                access: None,
            },
            candidates: Vec::new(),
            error: None,
        });
        row.companion
            .repository
            .clone_from(&grant.target.declaration.repository);
        row.companion.profile.clone_from(&grant.target.declaration.profile);
        row.companion.target_cloud_id = Some(grant.target.cloud_id.clone());
        row.companion.selected = grant.selected;
        let valid = selection_status(&grant, context, &alias);
        if !grant.selected || valid != Status::Ready {
            let result = revoke(store, state, &alias, &mut grant, transport);
            row.companion.status = if grant.source_disconnected && grant.target_revoked {
                valid
            } else {
                Status::RevocationPending
            };
            if let Err(error) = result {
                row.error = Some(error.to_string());
            }
            if !grant.selected && grant.source_disconnected && grant.target_revoked {
                state.grants.remove(&alias);
                store.save(state)?;
                if let Some(declaration_row) = declaration_row {
                    rows.insert(alias, declaration_row);
                } else {
                    rows.remove(&alias);
                }
            }
            continue;
        }
        match connect(store, state, &alias, &mut grant, transport) {
            Ok((status, access)) => {
                row.companion.status = status;
                row.companion.access = access;
                if status == Status::Changed {
                    if let Err(error) = revoke(store, state, &alias, &mut grant, transport) {
                        row.error = Some(error.to_string());
                    }
                    if !grant.source_disconnected || !grant.target_revoked {
                        row.companion.status = Status::RevocationPending;
                    }
                }
            }
            Err(error) => {
                row.companion.status = Status::Unreachable;
                row.companion.access.clone_from(&grant.access);
                row.error = Some(error.to_string());
            }
        }
    }
    let rows: Vec<_> = rows.into_values().collect();
    let catalog = catalog(&state.owner.cloud_id, &rows);
    catalog.validate().map_err(Error::Invalid)?;
    let publication_error = publish(transport, &catalog).err().map(|error| error.to_string());
    Ok(Snapshot {
        catalog,
        rows,
        publication_error,
        notice: None,
    })
}

fn declaration_rows(context: Option<&Context>) -> BTreeMap<String, Row> {
    let mut rows = BTreeMap::new();
    if let Some(context) = context {
        for (alias, declaration) in &context.declarations {
            let targets = candidates(&context.source, declaration, &context.inventory)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let status = match targets.len() {
                0 => Status::Missing,
                1 => Status::Unselected,
                _ => Status::Ambiguous,
            };
            rows.insert(
                alias.clone(),
                Row {
                    companion: Companion {
                        alias: alias.clone(),
                        repository: declaration.repository.clone(),
                        profile: declaration.profile.clone(),
                        target_cloud_id: None,
                        selected: false,
                        status,
                        access: None,
                    },
                    candidates: targets,
                    error: None,
                },
            );
        }
    }
    rows
}

fn selection_status(grant: &Grant, context: Option<&Context>, alias: &str) -> Status {
    let Some(context) = context else { return Status::Changed };
    let Some(declaration) = context.declarations.get(alias) else {
        return Status::Changed;
    };
    match grant
        .selection
        .resolve(&context.source, alias, declaration, &context.inventory)
    {
        Ok(target)
            if target.cloud_id == grant.target.cloud_id
                && target.scope == grant.target.scope
                && target.declaration.matches(&grant.target.declaration) =>
        {
            Status::Ready
        }
        Err(horizon_cloud::companions::SelectionError::Missing) => Status::Missing,
        Err(horizon_cloud::companions::SelectionError::Ambiguous) => Status::Ambiguous,
        _ => Status::Changed,
    }
}

fn persist(store: &Store, state: &mut State, alias: &str, grant: &Grant) -> Result<()> {
    state.grants.insert(alias.into(), grant.clone());
    store.save(state)
}

fn connect(
    store: &Store,
    state: &mut State,
    alias: &str,
    grant: &mut Grant,
    transport: &mut impl Transport,
) -> Result<(Status, Option<Access>)> {
    let source_id = state.owner.cloud_id.clone();
    let target = transport.worker(&grant.target.cloud_id)?;
    let source = transport.worker(&source_id)?;
    if source
        .as_ref()
        .is_some_and(|worker| grant.source_worker.as_ref().is_some_and(|id| id != &worker.id))
        || target.as_ref().is_some_and(|worker| {
            grant.target_worker.as_ref().is_some_and(|id| id != &worker.id)
                || grant
                    .revision
                    .as_ref()
                    .is_some_and(|revision| revision != &worker.revision)
        })
    {
        return Ok((Status::Changed, None));
    }
    if let Some(source) = &source
        && grant.source_worker.is_none()
    {
        grant.source_worker = Some(source.id.clone());
        grant.source_disconnected = true;
    }
    if let Some(target) = &target {
        if grant.target_worker.is_none() {
            grant.target_worker = Some(target.id.clone());
            grant.target_revoked = true;
        }
        grant.revision = Some(target.revision.clone());
    }
    // Remember observed identities even while stopped or before the other worker exists.
    persist(store, state, alias, grant)?;
    let Some(target) = target else {
        return Ok((Status::Missing, None));
    };
    if target.status != Status::Ready {
        return Ok((target.status, None));
    }
    let Some(source) = source else {
        return Ok((Status::Unavailable, None));
    };
    if source.status != Status::Ready {
        return Ok((Status::Unavailable, None));
    }
    let address = target
        .address
        .ok_or(Error::Invalid("Companion target has no SSH endpoint"))?;
    grant.source_disconnected = false;
    grant.target_revoked = false;
    // Arm cleanup durably before the first potentially successful remote operation.
    persist(store, state, alias, grant)?;
    let Response::Identity { public_key } = transport.call(
        &source_id,
        &Request::Identity {
            grant: grant.id.clone(),
        },
    )?
    else {
        return Err(Error::Invalid("Expected companion source identity"));
    };
    let Response::Authorized { host_key, worktree } = transport.call(
        &grant.target.cloud_id,
        &Request::Authorize {
            grant: grant.id.clone(),
            public_key,
            revision: target.revision,
        },
    )?
    else {
        return Err(Error::Invalid("Expected companion authorization"));
    };
    let expected = format!("/workspace/companions/worktrees/{}", grant.id);
    if worktree != expected {
        return Err(Error::Invalid("Companion worktree identity differs"));
    }
    let Response::Connected { ssh_alias, worktree } = transport.call(
        &source_id,
        &Request::Connect {
            grant: grant.id.clone(),
            alias: alias.into(),
            host: address.ip(),
            port: address.port(),
            host_key,
        },
    )?
    else {
        return Err(Error::Invalid("Expected companion connection"));
    };
    if ssh_alias != format!("companion-{alias}") || worktree != expected {
        return Err(Error::Invalid("Companion connection identity differs"));
    }
    let access = Access {
        grant: grant.id.clone(),
        ssh_alias,
        worktree,
    };
    grant.access = Some(access.clone());
    persist(store, state, alias, grant)?;
    Ok((Status::Ready, Some(access)))
}

fn revoke(
    store: &Store,
    state: &mut State,
    alias: &str,
    grant: &mut Grant,
    transport: &mut impl Transport,
) -> Result<()> {
    let source_id = state.owner.cloud_id.clone();
    if grant.access.take().is_some() {
        persist(store, state, alias, grant)?;
    }
    let mut failure = None;
    if !grant.source_disconnected {
        let result = revoke_end(
            transport,
            &source_id,
            grant.source_worker.as_deref(),
            &Request::Disconnect {
                grant: grant.id.clone(),
            },
        );
        match result {
            Ok(()) => {
                grant.source_disconnected = true;
                persist(store, state, alias, grant)?;
            }
            Err(error) => failure = Some(error),
        }
    }
    if !grant.target_revoked {
        let result = revoke_end(
            transport,
            &grant.target.cloud_id,
            grant.target_worker.as_deref(),
            &Request::Revoke {
                grant: grant.id.clone(),
            },
        );
        match result {
            Ok(()) => {
                grant.target_revoked = true;
                persist(store, state, alias, grant)?;
            }
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(()), Err)
}

fn revoke_end(transport: &mut impl Transport, cloud: &str, pinned: Option<&str>, request: &Request) -> Result<()> {
    let Some(pinned) = pinned else { return Ok(()) };
    let worker = transport
        .worker(cloud)?
        .ok_or(Error::Invalid("Revocation pending: worker is unavailable"))?;
    if worker.id != pinned || worker.status != Status::Ready {
        return Err(Error::Invalid(
            "Revocation pending: the original worker must be reachable",
        ));
    }
    let expected = if matches!(request, Request::Disconnect { .. }) {
        Response::Disconnected
    } else {
        Response::Revoked
    };
    if transport.call(cloud, request)? != expected {
        return Err(Error::Invalid("Revocation was not confirmed"));
    }
    Ok(())
}

fn publish(transport: &mut impl Transport, catalog: &Catalog) -> Result<()> {
    if transport
        .worker(&catalog.source_cloud_id)?
        .is_none_or(|worker| worker.status != Status::Ready)
    {
        return Err(Error::Invalid(
            "Companion discovery will publish when the source cloud is running",
        ));
    }
    transport.publish(&catalog.source_cloud_id, catalog)
}

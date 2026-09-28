//! A rebuild on a provider that cannot report a server's image
//! (`provider::Rebuild::NewServer`): the requested replacement releases the server,
//! keeps the workspace volume, and commits the new image with the server fence
//! cleared in one save, so the reconnect that follows creates a new server on it.
//! Every step can be repeated after an interruption: the release is journaled by
//! the provider, and nothing is committed until the server is proven gone.
use super::{Boundary, Deployment, Error, Event, NOTHING_PENDING, Result, Server, Stage, Store, activity};

/// What cancelling a requested rebuild did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cancelled {
    /// The server was never released, so the cloud is as it was before the rebuild.
    Untouched,
    /// The server was released; the reconnect creates a new one on the recorded image.
    Released,
}

/// Moves a worker whose replacement is requested onto the new image: releases its
/// server, then commits the image with the fence cleared. The sessions relaunch on
/// the new server once the reconnect finds it ready.
pub(super) fn switch(server: &dyn Server, store: &Store, state: &mut Deployment, emit: &dyn Fn(Event)) -> Result<()> {
    // The whole journal is checked before the server is touched.
    state.clone().commit_replacement()?;
    emit(Event::stage(Stage::Replace));
    emit(activity("Releasing the server; the workspace volume stays"));
    server.release(store, state)?;
    server.checkpoint(Boundary::Released)?;
    let mut committed = state.clone();
    committed.commit_replacement()?;
    server.reopen(store, &mut committed)?;
    *state = committed;
    server.checkpoint(Boundary::Rebound)
}

/// Cancels a requested rebuild. Before the release began the server is untouched,
/// so the journal simply returns to its built image and is dropped. Otherwise the
/// release is finished and the worker keeps its recorded image on a new server.
pub(super) fn cancel(
    server: &dyn Server,
    store: &Store,
    state: &mut Deployment,
    emit: &dyn Fn(Event),
) -> Result<Cancelled> {
    // The whole journal is checked before the server is touched.
    state.replacement_worker()?;
    let operation = state
        .image_replacement
        .as_ref()
        .ok_or(Error::Invalid(NOTHING_PENDING))?
        .operation;
    if !server.released(store, state)? {
        state.refuse_replacement(Stage::Ready)?;
        state.discard_replacement()?;
        store.save(state)?;
        emit(Event::Output(
            "Image rebuild cancelled before the server was released; the cloud keeps its server and image".into(),
        ));
        return Ok(Cancelled::Untouched);
    }
    emit(Event::stage(Stage::Replace));
    emit(activity("Finishing the server's release; the workspace volume stays"));
    server.release(store, state)?;
    server.checkpoint(Boundary::Released)?;
    let mut kept = state.clone();
    kept.image_replacement = None;
    kept.stage = Stage::Readiness;
    // The sessions relaunch on the new server, as after a rebuild.
    kept.session_restart = Some(operation);
    server.reopen(store, &mut kept)?;
    *state = kept;
    server.checkpoint(Boundary::Reverted)?;
    emit(Event::Output(
        "Image rebuild cancelled; a new server starts on the previous image".into(),
    ));
    Ok(Cancelled::Released)
}

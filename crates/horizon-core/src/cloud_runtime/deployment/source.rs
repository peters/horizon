//! Committed source: validated and packed before allocation, transferred to the ready worker.
use super::{Connection, Deployment, Event, Result, Runner, Stage, Store, repository};
use std::path::{Path, PathBuf};

pub(super) struct Packed {
    pack: PathBuf,
    auxiliary: Option<PathBuf>,
}

pub(super) fn pack(state: &Deployment, pack_root: &Path, runner: &Runner<'_>) -> Result<Packed> {
    let pack = pack_root.join("source.pack");
    let mut auxiliary = None;
    if !state.source_ready {
        repository::validate_tree(&state.repository, &state.revision, runner)?;
        repository::pack(&state.repository, &state.revision, &pack, runner)?;
        auxiliary = Some(repository::auxiliary(
            &state.repository,
            &state.revision,
            pack_root,
            runner,
        )?);
    }
    Ok(Packed { pack, auxiliary })
}

pub(super) fn transfer(
    connection: &Connection,
    store: &Store,
    state: &mut Deployment,
    packed: Packed,
    runner: &Runner<'_>,
    emit: &dyn Fn(Event),
) -> Result<()> {
    if !state.source_ready {
        state.stage = Stage::Worktrees;
        store.save(state)?;
        emit(Event::stage(state.stage));
        connection.transfer(&packed.pack, &state.revision, runner)?;
        if let Some(auxiliary) = packed.auxiliary {
            connection.transfer_material(&auxiliary, runner)?;
        }
    }
    Ok(())
}

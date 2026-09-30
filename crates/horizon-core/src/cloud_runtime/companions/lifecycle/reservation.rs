//! Serialize reservations before consulting durable bindings across source journals.
use super::{Binding, Error, Request, Result, journal, receipt};
use std::{io::Read, path::Path};

pub(super) fn lock(root: &Path) -> Result<receipt::ExecutionLock> {
    receipt::execution_lock(&root.join("companion-reservations"))
}

pub(super) fn check(request: &Request<'_>, binding: &Binding, source: &journal::State) -> Result<()> {
    check_source(request, binding, source)?;
    for entry in std::fs::read_dir(request.root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let file = match std::fs::File::open(entry.path().join("companions.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 256 * 1024 {
            return Err(Error::Invalid("Companion journal is too large"));
        }
        let state: journal::State = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
        if entry.file_name() != std::ffi::OsStr::new(&state.owner.cloud_id) {
            return Err(Error::Invalid("Companion journal ownership differs from its directory"));
        }
        if state.owner.scope != request.owner.scope || state.owner.cloud_id == request.owner.cloud_id {
            continue;
        }
        let store = journal::Store::open(request.root, &state.owner)?;
        let state = store.load()?;
        check_source(request, binding, &state)?;
    }
    Ok(())
}

fn check_source(request: &Request<'_>, binding: &Binding, state: &journal::State) -> Result<()> {
    if state.owner.scope != request.owner.scope {
        return Ok(());
    }
    for (alias, other) in state.intents.bindings() {
        if (state.owner == *request.owner && alias == request.alias)
            || other.target().cloud_id == binding.target().cloud_id
            || !other.target().declaration.matches(&binding.target().declaration)
        {
            continue;
        }
        let pending = (other.origin() == super::intent::Origin::Reserved
            || state.intents.pending_on(&other.target().cloud_id))
            && !state.intents.settled_on(&other.target().cloud_id);
        let target = request.target_store(other)?;
        let exists = target
            .load()?
            .is_some_and(|record| !super::intent::refuses_deployment(&record));
        if pending || exists {
            return Err(Error::Invalid(
                "Another companion with the same repository and profile is being created or already exists; use that cloud once it is ready",
            ));
        }
    }
    Ok(())
}

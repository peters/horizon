use super::{Error, Owner, Result, Selection, Target};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    version: u32,
    pub owner: Owner,
    pub grants: BTreeMap<String, Grant>,
    #[serde(default, skip_serializing_if = "super::intent::Journal::is_empty")]
    pub intents: super::intent::Journal,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Grant {
    pub selection: Selection,
    pub target: Target,
    pub id: String,
    pub selected: bool,
    pub source_worker: Option<String>,
    pub target_worker: Option<String>,
    pub revision: Option<String>,
    pub source_disconnected: bool,
    pub target_revoked: bool,
    /// Whether the source dropped the grant's key after the target revoked it. A
    /// grant from a journal before this field has not, so it passes through forget.
    #[serde(default)]
    pub source_forgotten: bool,
    #[serde(default)]
    pub access: Option<horizon_cloud_protocol::companion::Access>,
}

impl Grant {
    /// Whether nothing of the grant is left on either worker.
    pub(super) const fn cleaned_up(&self) -> bool {
        self.source_disconnected && self.target_revoked && self.source_forgotten
    }
}

pub(super) struct Store {
    root: PathBuf,
    owner: Owner,
    lock: File,
}

impl Store {
    pub fn open(root: &Path, owner: &Owner) -> Result<Self> {
        if [&owner.cloud_id, &owner.scope.session_id, &owner.scope.workspace_id]
            .into_iter()
            .any(|id| !horizon_cloud::valid_id(id))
        {
            return Err(Error::Invalid("Invalid companion owner"));
        }
        crate::session_store::require_directory_durability()?;
        let root = super::super::state::cloud_directory(root, &owner.cloud_id)?;
        std::fs::create_dir_all(&root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("companions.lock"))?;
        lock.try_lock().map_err(|_| Error::Busy)?;
        Ok(Self {
            root,
            owner: owner.clone(),
            lock,
        })
    }

    pub fn load(&self) -> Result<State> {
        let path = self.root.join("companions.json");
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(State {
                    version: 1,
                    owner: self.owner.clone(),
                    grants: BTreeMap::new(),
                    intents: super::intent::Journal::default(),
                });
            }
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 256 * 1024 {
            return Err(Error::Invalid("Companion journal is too large"));
        }
        let state: State = serde_json::from_slice(&bytes).map_err(|_| Error::Json)?;
        if state.version != 1
            || state.owner.cloud_id != self.owner.cloud_id
            || state.grants.len() > 64
            || !horizon_cloud::valid_id(&state.owner.scope.session_id)
            || !horizon_cloud::valid_id(&state.owner.scope.workspace_id)
        {
            return Err(Error::Invalid("Companion journal ownership or version differs"));
        }
        let mut ids = BTreeSet::new();
        for (alias, grant) in &state.grants {
            let source = Target {
                scope: state.owner.scope.clone(),
                cloud_id: state.owner.cloud_id.clone(),
                declaration: grant.target.declaration.clone(),
            };
            if Selection::new(&source, alias, &grant.target).as_ref() != Ok(&grant.selection) {
                return Err(Error::Invalid("Persisted companion selection differs from its grant"));
            }
            if grant.access.as_ref().is_some_and(|access| {
                access.grant != grant.id
                    || access.ssh_alias != format!("companion-{alias}")
                    || access.worktree != format!("/workspace/companions/worktrees/{}", grant.id)
                    || grant.source_worker.is_none()
                    || grant.target_worker.is_none()
                    || grant.revision.is_none()
                    || grant.source_disconnected
                    || grant.target_revoked
            }) {
                return Err(Error::Invalid("Invalid persisted companion connection"));
            }
            if !horizon_cloud::valid_id(&grant.id)
                || !ids.insert(&grant.id)
                || (!grant.source_disconnected && grant.source_worker.is_none())
                || (!grant.target_revoked && (grant.target_worker.is_none() || grant.revision.is_none()))
                || [&grant.source_worker, &grant.target_worker]
                    .into_iter()
                    .flatten()
                    .any(|id| !horizon_cloud::valid_id(id))
            {
                return Err(Error::Invalid("Invalid persisted companion grant"));
            }
        }
        state.intents.validate(&state.owner)?;
        Ok(state)
    }

    pub fn save(&self, state: &State) -> Result<()> {
        state.intents.validate(&state.owner)?;
        let bytes = serde_json::to_vec(state).map_err(|_| Error::Json)?;
        if bytes.len() > 256 * 1024 {
            return Err(Error::Invalid("Companion journal is too large"));
        }
        let mut pending = tempfile::NamedTempFile::new_in(&self.root)?;
        pending.write_all(&bytes)?;
        pending.flush()?;
        pending.as_file().sync_all()?;
        pending
            .persist(self.root.join("companions.json"))
            .map_err(|error| error.error)?;
        #[cfg(unix)]
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.lock.unlock();
    }
}

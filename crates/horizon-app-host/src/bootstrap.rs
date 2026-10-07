//! Machine-local actor selection for standalone native MCP hosts.
use crate::{Error, Result, actor::Actor};
#[cfg(unix)]
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
#[cfg(unix)]
use uuid::Uuid;

/// Client configuration is private host input, never a project contract or MCP argument.
#[cfg(unix)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Client {
    version: u32,
    owner: Uuid,
    project: PathBuf,
    state: PathBuf,
    provider: String,
    tunnel_binary: PathBuf,
    tunnel_sha256: String,
}

pub struct Host {
    pub actor: Arc<Actor>,
    pub evidence: PathBuf,
    pub reports: PathBuf,
}

#[cfg(unix)]
fn client(path: &Path) -> Result<Client> {
    use std::{fs::File, io::Read, os::unix::fs::MetadataExt};
    if !path.is_absolute() {
        return Err(Error::Unavailable);
    }
    let file = crate::project::open(
        &File::open("/").map_err(|_| Error::Unavailable)?,
        path.strip_prefix("/").map_err(|_| Error::Unavailable)?,
    )?;
    let metadata = file.metadata().map_err(|_| Error::Unavailable)?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
        return Err(Error::Unavailable);
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    if bytes.len() > 65536 {
        return Err(Error::Unavailable);
    }
    let client: Client = serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
    if client.version != 1
        || client.owner.is_nil()
        || client.provider.is_empty()
        || client.provider.len() > 128
        || !client.project.is_absolute()
        || !client.state.is_absolute()
        || !client.tunnel_binary.is_absolute()
        || client.project.canonicalize().map_err(|_| Error::Unavailable)? != client.project
    {
        return Err(Error::Unavailable);
    }
    Ok(client)
}

#[cfg(unix)]
fn child(root: &horizon_app_process::storage::Directory, name: &str) -> Result<()> {
    if root.child(name).is_err() {
        root.create_child(name)?;
    }
    root.child(name)?;
    Ok(())
}

#[cfg(unix)]
fn evidence_ready(root: &horizon_app_process::storage::Directory, name: &str) -> Result<()> {
    child(root, name)?;
    if !root.child(name)?.is_empty()? {
        return Err(Error::CleanupUncertain);
    }
    Ok(())
}

#[cfg(unix)]
fn claim(
    client: &Client,
    journal: Arc<horizon_app_runtime::journal::Journal>,
    state: &horizon_app_process::storage::Directory,
    evidence: &str,
    before_claim: impl FnOnce(),
) -> Result<Arc<horizon_app_runtime::journal::execution::Workspace>> {
    before_claim();
    let workspace = Arc::new(horizon_app_runtime::journal::execution::Workspace::open(
        journal,
        client.owner,
        &client.project,
    )?);
    evidence_ready(state, evidence)?;
    Ok(workspace)
}

#[cfg(unix)]
fn selected(
    path: &Path,
) -> Result<(
    Client,
    horizon_app_runtime::account::Account,
    Arc<horizon_app_runtime::journal::Journal>,
)> {
    use horizon_app_runtime::{account::Account, journal::Journal};
    use horizon_core::{
        Config,
        remote_browser_credential::{CredentialStores, KeyringCredentialStore, SessionCredentialStore},
    };
    let client = client(path)?;
    horizon_app_process::storage::Directory::open(&client.state)?;
    horizon_app_provider::tunnel::VerifiedBinary::capture(&client.tunnel_binary, &client.tunnel_sha256)?;
    let config_path = Config::resolve_path(None).ok_or(Error::Unavailable)?;
    let text = std::fs::read_to_string(config_path).map_err(|_| Error::Unavailable)?;
    let config = Config::from_yaml(&text).map_err(|_| Error::Unavailable)?;
    let profile = config
        .browser
        .remote
        .providers
        .get(&client.provider)
        .ok_or(Error::Unavailable)?;
    let session = SessionCredentialStore::new();
    let keyring = KeyringCredentialStore::open().map_err(|_| horizon_app_runtime::Error::CredentialsUnavailable)?;
    let account = Account::capture(
        profile,
        &CredentialStores {
            session: &session,
            os_keychain: Some(&keyring),
            environment: None,
        },
    )?;
    let journal = Arc::new(Journal::open(&client.state, &account)?);
    Ok((client, account, journal))
}

/// # Errors
/// Requires a private machine-local client file, existing private shared state, a pinned
/// tunnel binary and the configured provider credential store. Retained work still holds admission.
#[cfg(unix)]
pub fn open(path: &Path) -> Result<Host> {
    use crate::local::{Configuration, Local};
    use horizon_app_process::storage::Directory;
    use horizon_app_testing::contract::Contract;
    let (client, account, journal) = selected(path)?;
    let state = Directory::open(&client.state)?;
    let owner_name = client.owner.simple().to_string();
    child(&state, &owner_name)?;
    let owner_state = state.child(&owner_name)?;
    let owner_path = client.state.join(owner_name);
    let local_name = "00000000000000000000000000000001";
    let evidence_name = "00000000000000000000000000000002";
    let reports_name = "00000000000000000000000000000003";
    child(&owner_state, local_name)?;
    let workspace = claim(&client, journal, &owner_state, evidence_name, || ())?;
    child(&owner_state, reports_name)?;
    let agents = crate::project::read(&workspace.root_directory()?, Path::new("AGENTS.md"))?;
    let contract = Contract::from_agents(&agents)?;
    let worker = std::env::current_exe().map_err(|_| Error::Unavailable)?;
    let local = Arc::new(Local::new(
        Arc::clone(&workspace),
        &account,
        &client.project,
        Configuration {
            process_worker: worker.clone(),
            tunnel_worker: worker,
            tunnel_binary: client.tunnel_binary,
            tunnel_sha256: client.tunnel_sha256,
            state: owner_path.join(local_name),
        },
    )?);
    state.matches_path(&client.state)?;
    Ok(Host {
        actor: Arc::new(Actor::new(workspace, &account, local, contract)?),
        evidence: owner_path.join(evidence_name),
        reports: owner_path.join(reports_name),
    })
}

/// # Errors
/// Reconcile only this trusted client's exact journal-owned resources while holding its workspace lease.
/// Unknown/lost allocations remain held. Native sessions close before local services and uploads.
#[cfg(unix)]
pub fn reconcile(path: &Path) -> Result<Vec<horizon_app_runtime::journal::Operation>> {
    use horizon_app_runtime::journal::{
        Kind,
        execution::Workspace,
        recovery::{Recovery, Resolution},
    };
    let (client, account, journal) = selected(path)?;
    let mut workspace = Workspace::open(journal, client.owner, &client.project)?;
    let provider = workspace.provider(&account)?;
    let root = client
        .state
        .join(client.owner.simple().to_string())
        .join("00000000000000000000000000000001");
    let mut pending = workspace.journal().pending(client.owner)?;
    pending.sort_by_key(|operation| match operation.kind {
        Kind::Session => 0,
        Kind::Run | Kind::Tunnel => 1,
        Kind::Upload => 2,
    });
    for operation in &pending {
        if operation.pending_resources == 0 {
            close_undispatched(&workspace, operation, &root)?;
            continue;
        }
        if matches!(operation.kind, Kind::Run | Kind::Tunnel) {
            crate::local::confirm_receipt(&workspace, operation.id, &root.join(operation.id.simple().to_string()))?;
            continue;
        }
        workspace
            .journal()
            .recover_owned(client.owner, operation.id, |resource| {
                let failed = || horizon_app_runtime::Error::ReconciliationRequired;
                match resource {
                    Recovery::AllocationReserved => (),
                    Recovery::Allocated { reference } => {
                        let session = provider.session(reference).map_err(|_| failed())?;
                        provider.release_session(&session).map_err(|_| failed())?;
                    }
                    Recovery::AllocationIntent { operation } => {
                        let session = provider
                            .find_session(operation)
                            .map_err(|_| failed())?
                            .ok_or_else(failed)?;
                        provider.release_session(&session).map_err(|_| failed())?;
                    }
                    Recovery::Uploaded { reference } => {
                        let app = horizon_app_provider::api::UploadedApp::from_owned_reference(reference)
                            .map_err(|_| failed())?;
                        provider.delete_app(&app).map_err(|_| failed())?;
                    }
                    Recovery::UploadIntent { operation, .. } => {
                        let app = provider
                            .find_upload(operation)
                            .map_err(|_| failed())?
                            .ok_or_else(failed)?;
                        provider.delete_app(&app).map_err(|_| failed())?;
                    }
                    _ => return Err(failed()),
                }
                Ok(Resolution::ConfirmedClosed)
            })?;
    }
    workspace.finish_reconciliation()?;
    pending
        .iter()
        .map(|operation| {
            workspace
                .journal()
                .status(client.owner, operation.id)
                .map_err(Error::from)
        })
        .collect()
}

#[cfg(unix)]
fn close_undispatched(
    workspace: &horizon_app_runtime::journal::execution::Workspace,
    operation: &horizon_app_runtime::journal::Operation,
    root: &Path,
) -> Result<()> {
    use horizon_app_runtime::journal::{Kind, Phase};
    // Exclusive workspace ownership plus a validated empty ledger proves that no
    // process/upload/allocation dispatch intent was ever committed for this record.
    if operation.pending_resources != 0 || !matches!(operation.phase, Phase::Preparing | Phase::Uncertain) {
        return Err(horizon_app_runtime::Error::ReconciliationRequired.into());
    }
    if matches!(operation.kind, Kind::Run | Kind::Tunnel) {
        let root = horizon_app_process::storage::Directory::open(root)?;
        let name = operation.id.simple().to_string();
        root.retire_empty_child(&name).map_err(|_| Error::CleanupUncertain)?;
    }
    workspace.journal().confirm_released(workspace.owner(), operation.id)?;
    Ok(())
}

#[cfg(not(unix))]
pub fn open(_path: &Path) -> Result<Host> {
    Err(Error::Unavailable)
}

#[cfg(not(unix))]
pub fn reconcile(_path: &Path) -> Result<Vec<horizon_app_runtime::journal::Operation>> {
    Err(Error::Unavailable)
}

#[cfg(all(test, unix))]
mod tests;

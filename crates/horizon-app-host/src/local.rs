//! Bounded foreground commands and tunnel guardians bound to the exclusive workspace journal.
use crate::lifecycle::{Lock as HostLock, Operation as HostOperation, Reason as HostReason};
use crate::{Error, Result};
use horizon_app_process::{Event, Request, client::Process};
use horizon_app_provider::api::BrowserStack;
use horizon_app_provider::tunnel::{LocalPort, Status, VerifiedBinary};
use horizon_app_provider::tunnel_guard::{GuardedTunnel, Request as TunnelRequest};
use horizon_app_runtime::journal::{
    Kind,
    execution::Workspace,
    recovery::{Recovery, Resolution},
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub mod recovery;

/// Trusted bundled executable and pinned tunnel configuration, never MCP/project arguments.
pub struct Configuration {
    pub process_worker: PathBuf,
    pub tunnel_worker: PathBuf,
    pub tunnel_binary: PathBuf,
    pub tunnel_sha256: String,
    pub state: PathBuf,
}

#[derive(Deserialize)]
pub(super) struct Receipt {
    operation: Uuid,
    guardian_pid: u32,
    #[serde(default)]
    child_pid: Option<u32>,
    #[serde(default)]
    boot_id: Option<Uuid>,
    complete: bool,
}
pub(crate) fn confirm_receipt(workspace: &Workspace, id: Uuid, state: &Path) -> Result<()> {
    confirm_receipt_with_reboot(workspace, id, state, None)
}

pub(crate) fn confirm_receipt_with_reboot(
    workspace: &Workspace,
    id: Uuid,
    state: &Path,
    confirmation: Option<&recovery::RebootConfirmation>,
) -> Result<()> {
    workspace.journal().recover_owned(workspace.owner(), id, |recovery| {
        let directory = horizon_app_process::storage::Directory::open(state)
            .map_err(|_| horizon_app_runtime::Error::ReconciliationRequired)?;
        let receipt: Receipt = directory
            .receipt()
            .map_err(|_| horizon_app_runtime::Error::ReconciliationRequired)?;
        let identity = match recovery {
            Recovery::LocalIntent => !receipt.operation.is_nil() && receipt.guardian_pid != 0,
            Recovery::LocalProcess {
                operation,
                guardian_pid,
            } => receipt.operation == operation && receipt.guardian_pid == guardian_pid,
            _ => false,
        };
        if !identity {
            return Err(horizon_app_runtime::Error::ReconciliationRequired);
        }
        if !receipt.complete {
            recovery::confirm_reboot(&directory, id, &receipt, confirmation)?;
        }
        Ok(Resolution::ConfirmedClosed)
    })?;
    Ok(())
}

enum Guardian {
    Process(Process),
    Tunnel(GuardedTunnel),
}
impl Guardian {
    fn close(&mut self) -> Result<()> {
        match self {
            Self::Process(process) => process.close().map_err(Error::from),
            Self::Tunnel(tunnel) => tunnel.close().map_err(Error::from),
        }
    }
}
pub struct Lease {
    workspace: Arc<Workspace>,
    pub(crate) id: Uuid,
    deadline: Instant,
    guardian: Mutex<Guardian>,
    complete: AtomicBool,
}
impl Lease {
    /// # Errors
    /// Refuses unconfirmed guardian or journal cleanup.
    pub fn close(&self) -> Result<()> {
        if self.complete.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut guardian = self.guardian.lock().map_err(|_| Error::host_lock(HostLock::Guardian))?;
        if self.complete.load(Ordering::Acquire) {
            return Ok(());
        }
        // A journal failure must not prevent stopping this exactly retained guardian.
        let recorded = self.workspace.journal().uncertain(self.workspace.owner(), self.id);
        let stopped = guardian.close();
        if recorded.is_err() || stopped.is_err() {
            return Err(Error::CleanupUncertain);
        }
        self.workspace
            .journal()
            .confirm_released(self.workspace.owner(), self.id)?;
        self.complete.store(true, Ordering::Release);
        Ok(())
    }
}

/// One host actor. Independent lanes retain independent guardians; no process credentials are inherited.
pub struct Local {
    workspace: Arc<Workspace>,
    provider: Arc<BrowserStack>,
    configuration: Configuration,
    state_directory: horizon_app_process::storage::Directory,
    root: PathBuf,
}
impl Local {
    /// # Errors
    /// The private root and bundled worker paths are trusted host configuration.
    pub fn new(
        workspace: Arc<Workspace>,
        account: &horizon_app_runtime::account::Account,
        root: &Path,
        configuration: Configuration,
    ) -> Result<Self> {
        let root = root
            .canonicalize()
            .map_err(|error| Error::host_io(HostOperation::Guardian, &error))?;
        let provider = workspace.provider(account)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let expected = workspace
                .root_directory()?
                .metadata()
                .map_err(|error| Error::host_io(HostOperation::Guardian, &error))?;
            let selected = std::fs::File::open(&root)
                .and_then(|file| file.metadata())
                .map_err(|error| Error::host_io(HostOperation::Guardian, &error))?;
            if (expected.dev(), expected.ino()) != (selected.dev(), selected.ino()) {
                return Err(horizon_app_runtime::Error::OwnershipRefused.into());
            }
        }
        let state_directory = horizon_app_process::storage::Directory::open(&configuration.state)?;
        Ok(Self {
            workspace,
            provider,
            configuration,
            state_directory,
            root,
        })
    }
    pub(crate) fn belongs_to(&self, workspace: &Arc<Workspace>) -> bool {
        Arc::ptr_eq(&self.workspace, workspace)
    }
    fn directory(&self, id: Uuid) -> Result<PathBuf> {
        self.state_directory.create_child(&id.simple().to_string())?;
        let path = self.configuration.state.join(id.simple().to_string());
        horizon_app_process::storage::Directory::open(&path)?;
        Ok(path)
    }
    fn abort_local(&self, id: Uuid, error: Error) -> Error {
        // Only call before the guardian spawn API: no child or provider mutation exists.
        if self
            .workspace
            .journal()
            .confirm_released(self.workspace.owner(), id)
            .is_err()
        {
            Error::CleanupUncertain
        } else {
            error
        }
    }
    fn operation(&self, kind: Kind, lifetime: Duration) -> Result<(Uuid, Instant, PathBuf)> {
        if lifetime.is_zero() || lifetime > Duration::from_mins(30) {
            return Err(horizon_app_runtime::Error::OperationInvalid.into());
        }
        let deadline = Instant::now() + lifetime;
        if self
            .workspace
            .journal()
            .pending(self.workspace.owner())?
            .iter()
            .filter(|op| matches!(op.kind, Kind::Run | Kind::Tunnel))
            .count()
            >= 64
        {
            return Err(Error::host(HostOperation::Guardian, HostReason::LimitExceeded));
        }
        let mut completed = self
            .workspace
            .journal()
            .completed(self.workspace.owner())?
            .into_iter()
            .filter(|op| matches!(op.kind, Kind::Run | Kind::Tunnel))
            .collect::<Vec<_>>();
        completed.sort_by_key(|op| (op.created_seconds, op.id));
        for operation in completed.iter().take(completed.len().saturating_sub(32)) {
            self.state_directory.retire_child(&operation.id.simple().to_string())?;
            self.workspace.journal().retire(self.workspace.owner(), operation.id)?;
        }
        let operation = self.workspace.start(kind, lifetime)?;
        let state = self
            .directory(operation.id)
            .map_err(|error| self.abort_local(operation.id, error))?;
        self.workspace
            .journal()
            .local_intent(self.workspace.owner(), operation.id)
            .map_err(|error| self.abort_local(operation.id, error.into()))?;
        Ok((operation.id, deadline, state))
    }
    fn retain(&self, id: Uuid, deadline: Instant, guardian: Guardian) -> Result<Lease> {
        let lease = Lease {
            workspace: Arc::clone(&self.workspace),
            id,
            deadline,
            guardian: Mutex::new(guardian),
            complete: AtomicBool::new(false),
        };
        if Instant::now() >= deadline {
            lease.close().map_err(|_| Error::LocalCleanupUncertain(id))?;
            return Err(horizon_app_runtime::Error::OperationExpired.into());
        }
        Ok(lease)
    }
    /// # Errors
    /// `argv` is a validated authorized contract command, selected internally by the host.
    pub fn process(
        &self,
        argv: Vec<String>,
        kind: horizon_app_process::Kind,
        startup: Duration,
        lifetime: Duration,
    ) -> Result<Lease> {
        let (id, deadline, state) = self.operation(Kind::Run, lifetime)?;
        let prepared: Result<Request> = (|| {
            Ok(Request::new(
                &self.root,
                &state,
                argv,
                kind,
                startup.as_secs(),
                deadline.saturating_duration_since(Instant::now()).as_secs(),
            )?
            .bind_root(&self.workspace.root_directory()?)?)
        })();
        let request = prepared.map_err(|error| self.abort_local(id, error))?;
        let process = Process::start(&self.configuration.process_worker, request, |operation, pid| {
            self.workspace
                .journal()
                .local_started(self.workspace.owner(), id, operation, pid)
                .map_err(|_| horizon_app_process::Error::StateUnavailable)?;
            if Instant::now() >= deadline {
                return Err(horizon_app_process::Error::Timeout);
            }
            Ok(())
        });
        match process {
            Ok(process) => self.retain(id, deadline, Guardian::Process(process)),
            Err(error) => {
                if confirm_receipt(&self.workspace, id, &state).is_err() {
                    return Err(Error::LocalCleanupUncertain(id));
                }
                Err(error.into())
            }
        }
    }
    /// # Errors
    /// The only forwarded ports are already-resolved declared loopback services.
    pub(crate) fn tunnel(&self, ports: Vec<LocalPort>, lifetime: Duration) -> Result<Lease> {
        let (id, deadline, state) = self.operation(Kind::Tunnel, lifetime)?;
        let prepared: Result<TunnelRequest> = (|| {
            Ok(TunnelRequest {
                worker: self.configuration.tunnel_worker.clone(),
                state: state.clone(),
                binary: VerifiedBinary::capture(&self.configuration.tunnel_binary, &self.configuration.tunnel_sha256)?,
                ports,
                operation: id,
                lifetime: deadline.saturating_duration_since(Instant::now()),
            })
        })();
        let request = prepared.map_err(|error| self.abort_local(id, error))?;
        let tunnel = self.provider.guarded_tunnel(request, |_, pid| {
            self.workspace
                .journal()
                .local_started(self.workspace.owner(), id, id, pid)
                .map_err(|_| horizon_app_provider::Error::TunnelGuardFailed)?;
            if Instant::now() >= deadline {
                return Err(horizon_app_provider::Error::TunnelStartFailed);
            }
            Ok(())
        });
        match tunnel {
            Ok(tunnel) => self.retain(id, deadline, Guardian::Tunnel(tunnel)),
            Err(error) => {
                if confirm_receipt(&self.workspace, id, &state).is_err() {
                    return Err(Error::LocalCleanupUncertain(id));
                }
                Err(error.into())
            }
        }
    }
    /// # Errors
    /// Startup recovery uses private positive receipts; absence or incomplete cleanup remains held.
    pub fn recover(&self, id: Uuid) -> Result<()> {
        confirm_receipt(
            &self.workspace,
            id,
            &self.configuration.state.join(id.simple().to_string()),
        )
    }
}
impl Lease {
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.id
    }
    /// # Errors
    /// Refuses expired operations, invalid events or a missed read deadline.
    pub fn next(&self, timeout: Duration) -> Result<Event> {
        let mut guardian = self.guardian.lock().map_err(|_| Error::host_lock(HostLock::Guardian))?;
        let timeout = timeout.min(self.deadline.saturating_duration_since(Instant::now()));
        if timeout.is_zero() || self.complete.load(Ordering::Acquire) {
            return Err(horizon_app_runtime::Error::OperationExpired.into());
        }
        match &mut *guardian {
            Guardian::Process(process) => Ok(process.next(timeout)?),
            Guardian::Tunnel(_) => Err(Error::host(HostOperation::Guardian, HostReason::WrongGuardianKind)),
        }
    }
    pub(crate) fn tunnel_status(&self) -> Result<Status> {
        let mut guardian = self.guardian.lock().map_err(|_| Error::host_lock(HostLock::Guardian))?;
        if self.complete.load(Ordering::Acquire) || Instant::now() >= self.deadline {
            return Err(horizon_app_runtime::Error::OperationExpired.into());
        }
        match &mut *guardian {
            Guardian::Tunnel(tunnel) => Ok(tunnel.status()?),
            Guardian::Process(_) => Err(Error::host(HostOperation::Guardian, HostReason::WrongGuardianKind)),
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

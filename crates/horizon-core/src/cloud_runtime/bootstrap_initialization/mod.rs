//! Owning-host first initialization. Retained journals can recover or clean up,
//! but only this invocation's direct provider witnesses may send Initialize.
mod inspection;
mod record;
use super::{
    Cancellation,
    bootstrap_recovery::{
        self,
        connection::{Binding, Snapshot},
    },
    command::Runner,
    owner::Owner,
    ssh::Connection,
};
use horizon_cloud::{
    CloudError, CreateState, WorkerSpec,
    runpod::{RunPod, volumes},
};
use horizon_cloud_protocol::{
    OperationId, SharingMode,
    bootstrap::{BootstrapOutcome, BootstrapPayload, RecoveryReceipt, Startup},
};
pub use inspection::inspect;
use record::{FileBinding, Phase, Record, Signed};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Allocation initialization is missing, conflicting or uncertain")]
    Invalid,
    #[error("First initialization cannot be repeated; recover or explicitly clean up the anchored allocation")]
    Unresolved,
    #[error(transparent)]
    Owner(#[from] super::owner::Error),
    #[error(transparent)]
    Recovery(#[from] bootstrap_recovery::Error),
    #[error(transparent)]
    Provider(#[from] CloudError),
    #[error(transparent)]
    Transport(#[from] super::Error),
    #[error("Allocation initialization local I/O failed")]
    Io(#[from] std::io::Error),
}

/// Explicit machine-local request. No defaults or repository credential lookup.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub worker: WorkerSpec,
    pub sharing: SharingMode,
    pub credential_file: PathBuf,
    pub identity_file: PathBuf,
    /// A new dedicated pin file under an existing private directory.
    pub known_hosts: PathBuf,
}

/// The Owner's canonical lock remains held through creation and signed completion.
/// A failed invocation must use resume or cleanup; calling create again never
/// repeats provider creation or manufactures a new first-initialization witness.
/// The timeout bounds post-create SSH startup; provider calls and synchronous
/// durability checks retain their own bounds. Expiry prevents further SSH sends.
/// # Errors
/// Refuses existing journals, changed keys/accounts, uncertain saves or responses,
/// unqualified runtime identities and non-pristine worker state. No automatic cleanup.
pub fn create(
    owner: &mut Owner,
    request: &Request,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<RecoveryReceipt> {
    let timeout = timeout.min(Duration::from_secs(3600));
    let runner = runner(cancel);
    let (account, credential, identity) = bindings(request, &runner)?;
    if Record::load(owner)?.is_some() || bootstrap_recovery::is_anchored(owner)? {
        return Err(Error::Unresolved);
    }
    request.worker.validate_request()?;
    if request.worker.profile.gpu || request.worker.startup_metadata.is_some() || timeout.is_zero() {
        return Err(Error::Invalid);
    }
    verify_new_pin_path(&request.known_hosts)?;
    let provider = RunPod::new(credential);
    let volume_spec = provider.workspace_volume_spec(&request.worker, cancel)?;
    let mut record = Record {
        version: 1,
        request: request.clone(),
        account,
        identity,
        spec: request.worker.clone(),
        volume_spec,
        volume: volumes::State::Prepared,
        worker: CreateState::Prepared,
        startup: None,
        phase: Phase::Creating,
        requested: false,
        cleanup_receipt: None,
        initialize: None,
        abandon: None,
    };
    record.save(owner)?;
    let volume = provider.create_fresh_volume(&record.volume_spec.clone(), cancel, |next| {
        record.volume = next.clone();
        persist_provider(&record, owner)
    })?;
    let startup = Startup {
        version: 1,
        controller: owner.binding()?,
        token: OperationId::generate(),
        sharing: request.sharing,
        worker_operation: record.spec.operation_id.clone(),
        volume_id: volume.volume().id.clone(),
        data_center_id: volume.volume().data_center_id.clone(),
    };
    record.spec.startup_metadata = Some(horizon_cloud::StartupMetadata::new(
        serde_json::to_string(&startup).map_err(|_| Error::Invalid)?,
    )?);
    record.startup = Some(startup.clone());
    record.save(owner)?;
    let allocation = volume.create_worker(&record.spec.clone(), cancel, |next| {
        record.worker = next.clone();
        persist_provider(&record, owner)
    })?;
    let deadline = Instant::now() + timeout;
    loop {
        cancel.check()?;
        if Instant::now() >= deadline {
            return Err(Error::Unresolved);
        }
        if allocation.observe(cancel)?.ssh_address().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let attachment = allocation.confirm(cancel)?;
    let address = attachment.worker().ssh_address().ok_or(Error::Invalid)?;
    let target = target(&record, address)?;
    enroll(&record, &target, &runner, deadline)?;
    initialize(owner, &mut record, &target, &runner, deadline)?;
    complete(owner, &mut record, &target, cancel, deadline)
}

/// Read retained authority and send only the exact pre-anchored Recover request.
/// # Errors
/// Never retries first initialization, allocates resources or treats absence as fresh.
pub fn resume(
    owner: &mut Owner,
    request: &Request,
    cancel: &Cancellation,
    timeout: Duration,
) -> Result<RecoveryReceipt> {
    let deadline = Instant::now() + timeout.min(Duration::from_secs(3600));
    let runner = runner(cancel);
    let (account, credential, identity) = bindings(request, &runner)?;
    let mut record = Record::load(owner)?.ok_or(Error::Invalid)?;
    record.verify(owner, request, &account, &identity)?;
    if !matches!(record.phase, Phase::Requested | Phase::Completed) {
        return Err(Error::Unresolved);
    }
    bootstrap_recovery::require_existing(owner, &target(&record, ([127, 0, 0, 1], 22).into())?)?;
    let target = inspect_target(&RunPod::new(credential), &record, cancel)?;
    complete(owner, &mut record, &target, cancel, deadline)
}

/// Explicitly abandon this pre-admission allocation and delete only its anchored
/// resources. After Requested, a durable terminal worker receipt is mandatory;
/// a missing or unreachable marker never grants new deletion authority.
/// # Errors
/// Retains intent across uncertainty and refuses changed account, worker or volume.
pub fn cleanup(owner: &mut Owner, request: &Request, cancel: &Cancellation, timeout: Duration) -> Result<()> {
    let runner = runner(cancel);
    let (account, credential, identity) = bindings(request, &runner)?;
    let mut record = Record::load(owner)?.ok_or(Error::Invalid)?;
    record.verify(owner, request, &account, &identity)?;
    let provider = RunPod::new(credential);
    cleanup_with(
        owner,
        &mut record,
        &mut |record| inspect_target(&provider, record, cancel),
        &mut |connection, request| {
            Ok(runner.private_exchange(
                &mut connection.pinned_command("horizon-cloud-worker abandon-bootstrap"),
                request,
                timeout.min(Duration::from_secs(60)),
            )?)
        },
        &mut |owner, record| delete(&provider, owner, record, cancel),
    )
}

fn cleanup_with(
    owner: &mut Owner,
    record: &mut Record,
    resolve: &mut impl FnMut(&Record) -> Result<bootstrap_recovery::Target>,
    exchange: &mut impl FnMut(&Connection, &[u8]) -> Result<Vec<u8>>,
    delete: &mut impl FnMut(&mut Owner, &mut Record) -> Result<()>,
) -> Result<()> {
    if record.phase == Phase::Deleted {
        return Ok(());
    }
    if matches!(
        record.phase,
        Phase::Requested | Phase::Completed | Phase::AbandonRequested
    ) {
        let target = resolve(record)?;
        let payload = BootstrapPayload::Abandon {
            startup: target.startup.clone(),
            worker_id: target.worker_id.clone(),
        };
        if record.abandon.is_none() {
            record.abandon = Some(Signed::new(
                owner,
                &target.startup,
                &target.worker_id,
                &payload,
                BootstrapOutcome::Abandoned,
            )?);
            record.phase = Phase::AbandonRequested;
            record.save(owner)?;
        }
        let signed = record.abandon.as_ref().ok_or(Error::Invalid)?;
        let snapshot = Snapshot::capture(&target)?;
        // Validates the original anchored pins and identity without a Recover send.
        if bootstrap_recovery::require_existing(owner, &target)? != snapshot.binding {
            return Err(Error::Invalid);
        }
        let bytes = exchange(
            &snapshot.connection,
            signed.request(&target, &payload, BootstrapOutcome::Abandoned)?,
        )?;
        signed.confirm(&bytes)?;
        record.cleanup_receipt = Some(signed.receipt().clone());
    }
    // This save is the durable deletion intent. Once confirmed, retries no longer
    // need a live worker; all subsequent mutations still verify exact provider IDs.
    record.phase = Phase::DeleteConfirmed;
    record.save(owner)?;
    delete(owner, record)?;
    record.phase = Phase::Deleted;
    record.save(owner)
}

fn delete(provider: &RunPod, owner: &mut Owner, record: &mut Record, cancel: &Cancellation) -> Result<()> {
    let mut worker = record.worker.clone();
    if worker == CreateState::Requested {
        // Requested is a reconciliation-only provider state; never pass Prepared.
        provider.ensure(
            &record.spec.clone(),
            &mut worker,
            cancel,
            |next| {
                record.worker = next.clone();
                persist_provider(record, owner)
            },
            |_| {},
        )?;
    }
    if matches!(worker, CreateState::Bound { .. } | CreateState::Terminated { .. }) {
        provider.terminate(&record.spec.clone(), &mut worker, cancel, |next| {
            record.worker = next.clone();
            persist_provider(record, owner)
        })?;
    }
    let mut volume = record.volume.clone();
    provider.terminate_volume(&record.volume_spec.clone(), &mut volume, cancel, |next| {
        record.volume = next.clone();
        persist_provider(record, owner)
    })?;
    Ok(())
}

fn initialize(
    owner: &mut Owner,
    record: &mut Record,
    target: &bootstrap_recovery::Target,
    runner: &Runner<'_>,
    deadline: Instant,
) -> Result<()> {
    initialize_with(
        owner,
        record,
        target,
        &mut || remaining(deadline),
        &mut |connection, request, timeout| {
            Ok(runner.private_exchange(
                &mut connection.pinned_command("horizon-cloud-worker initialize-allocation"),
                request,
                timeout.min(Duration::from_secs(60)),
            )?)
        },
    )
}

fn initialize_with(
    owner: &mut Owner,
    record: &mut Record,
    target: &bootstrap_recovery::Target,
    budget: &mut impl FnMut() -> Result<Duration>,
    exchange: &mut impl FnMut(&Connection, &[u8], Duration) -> Result<Vec<u8>>,
) -> Result<()> {
    budget()?;
    let snapshot = Snapshot::capture(target)?;
    if !record.identity.matches(&snapshot.binding) {
        return Err(Error::Invalid);
    }
    let hosts = std::fs::read_to_string(&snapshot.connection.known_hosts)?;
    let fields: Vec<_> = hosts.split_whitespace().collect();
    if fields.len() != 3 || fields[1] != "ssh-ed25519" {
        return Err(Error::Invalid);
    }
    let payload = BootstrapPayload::Initialize {
        startup: target.startup.clone(),
        worker_id: target.worker_id.clone(),
        host_key: format!("{} {}", fields[1], fields[2]),
    };
    if bootstrap_recovery::anchor(owner, target)? != snapshot.binding {
        return Err(Error::Invalid);
    }
    record.initialize = Some(Signed::new(
        owner,
        &target.startup,
        &target.worker_id,
        &payload,
        BootstrapOutcome::Initializing,
    )?);
    record.phase = Phase::Prepared;
    record.save(owner)?;
    budget()?;
    record.requested = true;
    record.phase = Phase::Requested;
    record.save(owner)?;
    let signed = record.initialize.as_ref().ok_or(Error::Invalid)?;
    let bytes = exchange(
        &snapshot.connection,
        signed.request(target, &payload, BootstrapOutcome::Initializing)?,
        budget()?,
    )?;
    signed.confirm(&bytes)?;
    // The verified initialization response is anchored before recovery begins.
    record.save(owner)
}

fn complete(
    owner: &mut Owner,
    record: &mut Record,
    target: &bootstrap_recovery::Target,
    cancel: &Cancellation,
    deadline: Instant,
) -> Result<RecoveryReceipt> {
    remaining(deadline)?;
    bootstrap_recovery::require_existing(owner, target)?;
    let receipt = bootstrap_recovery::recover_until(owner, target, cancel, deadline)?;
    record.phase = Phase::Completed;
    record.save(owner)?;
    Ok(receipt)
}

fn bindings(request: &Request, runner: &Runner<'_>) -> Result<(FileBinding, horizon_cloud::Credential, FileBinding)> {
    let (account, credential) = FileBinding::credential(&request.credential_file)?;
    let (identity, public) = FileBinding::identity(&request.identity_file, runner)?;
    if request
        .worker
        .public_key
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ")
        != public
    {
        return Err(Error::Invalid);
    }
    Ok((account, credential, identity))
}
fn runner(cancel: &Cancellation) -> Runner<'_> {
    Runner {
        cancel,
        emit: &|_| {},
        secrets: Vec::new(),
    }
}
fn persist_provider(record: &Record, owner: &mut Owner) -> std::result::Result<(), CloudError> {
    record.save(owner).map_err(|_| CloudError::Persistence)
}
fn target(record: &Record, address: std::net::SocketAddr) -> Result<bootstrap_recovery::Target> {
    let CreateState::Bound { worker_id } = &record.worker else {
        return Err(Error::Invalid);
    };
    Ok(bootstrap_recovery::Target {
        startup: record.startup.clone().ok_or(Error::Invalid)?,
        worker_id: worker_id.clone(),
        connection: Connection {
            host: address.ip().to_string(),
            port: address.port(),
            identity: record.request.identity_file.clone(),
            known_hosts: record.request.known_hosts.clone(),
            host_key_alias: format!("horizon-cloud-{worker_id}"),
        },
    })
}
fn inspect_target(provider: &RunPod, record: &Record, cancel: &Cancellation) -> Result<bootstrap_recovery::Target> {
    let CreateState::Bound { worker_id } = &record.worker else {
        return Err(Error::Invalid);
    };
    let worker = provider.inspect(worker_id, cancel)?.ok_or(Error::Unresolved)?;
    worker.verify(&record.spec)?;
    target(record, worker.ssh_address().ok_or(Error::Unresolved)?)
}
fn verify_new_pin_path(path: &std::path::Path) -> Result<()> {
    let parent = path.parent().ok_or(Error::Invalid)?;
    if !path.is_absolute()
        || parent.canonicalize()? != parent
        || path.file_name().is_none()
        || std::fs::symlink_metadata(path).is_ok()
    {
        return Err(Error::Invalid);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = parent.metadata()?;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}
fn remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(Error::Unresolved)
    } else {
        Ok(remaining)
    }
}

fn enroll(record: &Record, target: &bootstrap_recovery::Target, runner: &Runner<'_>, deadline: Instant) -> Result<()> {
    remaining(deadline)?;
    let (identity, bytes) = FileBinding::capture(&record.request.identity_file)?;
    if identity != record.identity {
        return Err(Error::Invalid);
    }
    let mut key = tempfile::NamedTempFile::new()?;
    key.write_all(&bytes)?;
    let parent = pin_directory(record.request.known_hosts.parent().ok_or(Error::Invalid)?)?;
    let pin = tempfile::NamedTempFile::new_in(record.request.known_hosts.parent().ok_or(Error::Invalid)?)?;
    let pin = pin
        .persist_noclobber(&record.request.known_hosts)
        .map_err(|error| error.error)?;
    pin.sync_all()?;
    let mut connection = target.connection.clone();
    key.path().clone_into(&mut connection.identity);
    let mut arguments = connection.args();
    arguments.splice(
        0..0,
        [
            "-o".into(),
            "HostKeyAlgorithms=ssh-ed25519".into(),
            "-o".into(),
            "UpdateHostKeys=no".into(),
        ],
    );
    let mut pinned = None;
    let output = loop {
        runner.cancel.check()?;
        let current = pin_bytes(&record.request.known_hosts, &pin)?;
        if pinned.as_ref().is_some_and(|saved| *saved != current) {
            return Err(Error::Invalid);
        }
        let remaining = remaining(deadline)?;
        let result = runner.private_exchange(
            Command::new("ssh")
                .args(&arguments)
                .arg("cat /run/sshd/horizon-allocation/runtime.json"),
            &[],
            remaining.min(Duration::from_secs(60)),
        );
        let current = pin_bytes(&record.request.known_hosts, &pin)?;
        if pinned.as_ref().is_some_and(|saved| *saved != current) {
            return Err(Error::Invalid);
        }
        if !current.is_empty() {
            pinned = Some(current);
        }
        if let Ok(output) = result {
            break output;
        }
        runner.cancel.check()?;
        std::thread::sleep(Duration::from_millis(250).min(deadline.saturating_duration_since(Instant::now())));
    };
    let runtime: serde_json::Value = serde_json::from_slice(&output).map_err(|_| Error::Invalid)?;
    let startup = &target.startup;
    if runtime
        != serde_json::json!({"version":1,"startup":startup,"worker_id":target.worker_id,"volume_id":startup.volume_id,"data_center_id":startup.data_center_id,"worker_operation":startup.worker_operation})
    {
        return Err(Error::Invalid);
    }
    let _ = Binding::capture(target)?;
    let parent_path = record.request.known_hosts.parent().ok_or(Error::Invalid)?;
    verify_pin_parent(parent_path, &parent)?;
    pin_bytes(&record.request.known_hosts, &pin)?;
    pin.sync_all()?;
    parent.sync_all()?;
    pin_bytes(&record.request.known_hosts, &pin)?;
    verify_pin_parent(parent_path, &parent)?;
    Ok(())
}

fn pin_directory(path: &std::path::Path) -> Result<std::fs::File> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags, open};
        Ok(std::fs::File::from(
            open(
                path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(std::io::Error::from)?,
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::Invalid)
    }
}

fn verify_pin_parent(path: &std::path::Path, retained: &std::fs::File) -> Result<()> {
    let current = pin_directory(path)?.metadata()?;
    let original = retained.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (current.dev(), current.ino()) != (original.dev(), original.ino()) {
            return Err(Error::Invalid);
        }
    }
    #[cfg(not(unix))]
    let _ = (current, original);
    Ok(())
}

fn pin_bytes(path: &std::path::Path, retained: &std::fs::File) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    let verify = || -> Result<()> {
        let current = std::fs::symlink_metadata(path)?;
        if !current.is_file() {
            return Err(Error::Invalid);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let original = retained.metadata()?;
            if (current.dev(), current.ino()) != (original.dev(), original.ino()) {
                return Err(Error::Invalid);
            }
        }
        #[cfg(not(unix))]
        let _ = retained;
        Ok(())
    };
    verify()?;
    let (_, bytes) = bootstrap_recovery::connection::read_empty(path)?;
    verify()?;
    Ok(bytes)
}

#[cfg(all(test, unix))]
mod tests;

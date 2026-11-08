//! The one browser VM of this Horizon: what was rented, where it is reachable, and the
//! rental, stop, resume and teardown that the provider runtime performs.
//!
//! The machine is a Hetzner server that runs the image of `packaging/browser-vm` with its
//! state volume at `/state`. A person signs in to the sites once on the display of the
//! machine through the SSH route of its host; the volume keeps those sign-ins across a
//! stop and a resume. A delete removes the server and the volume.
//!
//! Every function here runs off the UI thread. The record is saved before each provider
//! request it guards, so an interrupted run is resumed or deleted, never repeated blindly.
//!
//! While the machine runs, Horizon keeps the ssh entry that reaches it: one host block in the
//! ssh configuration of the person, which names the key the machine was rented with, and the
//! host keys of the machine. Every route that runs `ssh <target>` then reaches the machine
//! without a copy of a key and without a manual host-key step.
use std::path::{Path, PathBuf};

use horizon_cloud::{
    Cancellation, CreateState, Progress,
    browser_vm::{self, Fences, Request},
    hetzner::{
        Hetzner,
        cloud::{Journal, Records, Stop, StopRecords},
    },
    host::RegistryLogin,
};
use serde::{Deserialize, Serialize};

pub mod local;

use super::{Error, Result, settings::Settings};

mod ssh_entry;

/// The state volume a first rent creates, in GB. The image and its browser profiles
/// need far less, and the volume can grow later.
pub const DEFAULT_VOLUME_GB: u16 = 20;
/// The VNC address of the machine's display on the loopback of its host. The Device
/// panel reaches it through that host's SSH route, so it is never published openly.
pub const VNC_ADDRESS: &str = "127.0.0.1:5900";
/// The server type prefix this machine uses. The shared-core quota of a Hetzner account is
/// exhausted and the image needs a real machine, so a browser VM is a dedicated `ccx`
/// server (decision 9 of issue 1780).
pub const SERVER_TYPE_PREFIX: &str = "ccx";
const RECORD: &str = "browser-vm.json";

/// How far the browser VM has come.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Nothing was ever rented.
    #[default]
    Idle,
    Deploying,
    /// The machine runs, and a person can sign in on its display.
    Ready,
    Stopping,
    /// The server is gone and the state volume, with its sign-ins, is kept.
    Stopped,
    Resuming,
    Deleting,
    /// The server and the state volume are gone.
    Deleted,
    /// The last run stopped on an error. The record still names what exists.
    Failed,
}

impl Phase {
    /// Whether a provider run of another phase may start now.
    #[must_use]
    pub fn idle(self) -> bool {
        !matches!(self, Self::Deploying | Self::Stopping | Self::Resuming | Self::Deleting)
    }
}

/// Where the browser VM runs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// A machine rented from Hetzner.
    #[default]
    Hetzner,
    /// A container of the image on this computer, with both ports on the loopback.
    Local,
}

/// What a person asks for when a browser VM is rented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deploy {
    /// Where the machine runs. A local deploy names no server type, location or volume.
    pub provider: Provider,
    /// An immutable reference of the browser VM image, `name@sha256:<64 hex>`.
    pub image: String,
    /// The dedicated `ccx` server type, such as `ccx13`.
    pub server_type: String,
    /// The location of the state volume, which fixes where the machine runs.
    pub location: String,
    pub volume_gb: u16,
}

/// The machine as the record and the tools name it.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub phase: Phase,
    /// Where the machine runs.
    pub provider: Provider,
    /// The VNC address of the display as a Device panel takes it on this computer, when the
    /// machine is a container here: a loopback address with no ssh route in front of it.
    pub vnc_address: Option<String>,
    /// The SSH target that every route names: the alias `horizon-browser-vm` while the managed
    /// ssh entry exists, else `root@<address>`. The settings entry `browser_host` names it.
    pub ssh_target: Option<String>,
    /// Why the managed ssh entry is missing, or what the machine is not yet trusted for, when
    /// that is the case. The run itself succeeded when the machine is in [`Phase::Ready`].
    pub ssh_note: Option<String>,
    pub image: Option<String>,
    pub server_type: Option<String>,
    pub location: Option<String>,
    pub volume_gb: Option<u16>,
    /// The server type's hourly price in euros, net of VAT.
    pub hourly_net: Option<f64>,
    /// Why the last run failed, if it did.
    pub error: Option<String>,
}

impl Status {
    /// The SSH host of the machine: the alias while the managed entry exists, else the address
    /// behind the `root@` target. A Device panel names the host and the user separately.
    #[must_use]
    pub fn ssh_host(&self) -> Option<&str> {
        self.ssh_target.as_deref().map(|target| match target.split_once('@') {
            Some((_, host)) => host,
            None => target,
        })
    }
}

/// What the record of the browser VM keeps: the journal of the provider runtime and what
/// the tools report, in one file.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub provider: Provider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vnc_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_gb: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hourly_net: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The fence for the server, as the provider runtime records it.
    #[serde(default = "prepared")]
    pub operation: CreateState,
    /// The volume, the location, the key and the stop, as the provider runtime records them.
    #[serde(default)]
    pub journal: Journal,
}

fn prepared() -> CreateState {
    CreateState::Prepared
}

impl Default for Record {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            provider: Provider::Hetzner,
            vnc_address: None,
            image: None,
            server_type: None,
            location: None,
            volume_gb: None,
            ssh_target: None,
            ssh_note: None,
            server_id: None,
            hourly_net: None,
            error: None,
            operation: CreateState::Prepared,
            journal: Journal::default(),
        }
    }
}

impl Record {
    /// The record at `root`, or the record of a Horizon that never rented a browser VM.
    /// # Errors
    /// Reports an unreadable or malformed record rather than starting afresh on it:
    /// a machine may still exist, and only a delete removes it.
    pub fn load(root: &Path) -> Result<Self> {
        match std::fs::read(root.join(RECORD)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| Error::Json),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Saves the record, synced with its directory, so a run that is interrupted reads as
    /// the phase it reached.
    /// # Errors
    /// Reports a record that could not be made durable.
    pub fn save(&self, root: &Path) -> Result<()> {
        std::fs::create_dir_all(root)?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| Error::Json)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.as_file().sync_all()?;
        file.persist(root.join(RECORD)).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    }

    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            phase: self.phase,
            provider: self.provider,
            vnc_address: self.vnc_address.clone(),
            ssh_target: self.ssh_target.clone(),
            ssh_note: self.ssh_note.clone(),
            image: self.image.clone(),
            server_type: self.server_type.clone(),
            location: self.location.clone(),
            volume_gb: self.volume_gb,
            hourly_net: self.hourly_net,
            error: self.error.clone(),
        }
    }
}

/// The browser VM as this Horizon recorded it.
/// # Errors
/// As [`Record::load`].
pub fn status(root: &Path) -> Result<Status> {
    Ok(Record::load(root)?.status())
}

/// What a run of the browser VM asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Deploy(Deploy),
    Resume,
    Stop,
    Delete,
}

impl Operation {
    fn phase(&self) -> Phase {
        match self {
            Self::Deploy(_) => Phase::Deploying,
            Self::Resume => Phase::Resuming,
            Self::Stop => Phase::Stopping,
            Self::Delete => Phase::Deleting,
        }
    }
}

/// Checks a run against the cloud settings of this machine and records the phase it
/// starts, without touching the provider.
///
/// A phase that a run left in progress is stale, because one Horizon runs one provider
/// run at a time: an explicit `Deploy`, `Stop` or `Delete` takes it over, which is how an
/// interrupted run is finished. A read never does.
/// # Errors
/// Refuses a request the Hetzner settings do not allow, and records that could not be
/// made durable.
pub fn begin(root: &Path, operation: &Operation) -> Result<Status> {
    let mut record = Record::load(root)?;
    match operation {
        Operation::Deploy(deploy) => {
            if record.phase == Phase::Stopped {
                return Err(Error::Invalid(
                    "The browser VM is stopped; resume it, so the sign-ins of its profiles stay",
                ));
            }
            // A machine that never existed, or one whose delete finished, starts from a
            // clean record: its server fence and journal name resources that are gone, and
            // a provider run would refuse them.
            if matches!(record.phase, Phase::Idle | Phase::Deleted) {
                record.operation = CreateState::Prepared;
                record.journal = Journal::default();
            }
            // A record that still owns a machine keeps its provider: a deploy of the other
            // provider would overwrite it, and the machine it owns would keep billing with no
            // record that names it.
            if !matches!(record.phase, Phase::Idle | Phase::Deleted) && record.provider != deploy.provider {
                return Err(Error::Invalid(match record.provider {
                    Provider::Hetzner => {
                        "The browser VM of this Horizon is a rented machine; stop and delete it before a deploy on this computer"
                    }
                    Provider::Local => {
                        "The browser VM of this Horizon is a container on this computer; stop and delete it before a deploy that rents a machine"
                    }
                }));
            }
            record.provider = deploy.provider;
            record.vnc_address = None;
            match deploy.provider {
                // The container of this computer needs no provider settings and no machine
                // of its own: Horizon owns its name, its image and its ports.
                Provider::Local => {
                    record.image = Some(local::DEFAULT_IMAGE.to_owned());
                    record.server_type = None;
                    record.location = None;
                    record.volume_gb = None;
                }
                Provider::Hetzner => {
                    admission(root, deploy)?;
                    record.image = Some(deploy.image.clone());
                    record.server_type = Some(deploy.server_type.clone());
                    record.location = Some(deploy.location.clone());
                    record.volume_gb = Some(deploy.volume_gb);
                }
            }
        }
        Operation::Resume => {
            let deploy = recorded_deploy(&record)?;
            if deploy.provider == Provider::Local {
                record.vnc_address = None;
            } else {
                admission(root, &deploy)?;
                browser_vm::prepare_resume(
                    &mut record.operation,
                    &mut record.journal,
                    record.phase == Phase::Stopped,
                )?;
            }
        }
        Operation::Stop | Operation::Delete => {
            if record.phase == Phase::Idle {
                return Err(Error::Invalid(match operation {
                    Operation::Stop => "No browser VM was ever rented; there is nothing to stop",
                    _ => "No browser VM was ever rented; there is nothing to delete",
                }));
            }
        }
    }
    record.phase = operation.phase();
    record.error = None;
    // The note of the machine that was recorded says nothing about the run that starts now.
    record.ssh_note = None;
    record.save(root)?;
    Ok(record.status())
}

/// Carries the recorded run out and records the phase it ends in. `Stop` and `Delete`
/// are given no progress callback: the provider runtime reports their steps as none.
///
/// `home` is the directory that holds the ssh configuration of the person. This run writes
/// the managed entry there and removes it again when the machine is gone; `None` leaves every
/// route at the plain address of the machine.
/// # Errors
/// Reports provider and persistence failures. A failed run keeps its record, so it is
/// retried or deleted.
pub fn run(
    root: &Path,
    home: Option<&Path>,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Progress),
) -> Result<Status> {
    let record = Record::load(root)?;
    // A container on this computer has no provider journal, no server and no volume record:
    // every phase of it is one call to Docker.
    if record.provider == Provider::Local {
        return run_local(root, home, record);
    }
    match record.phase {
        Phase::Deploying | Phase::Resuming => {
            let deploy = recorded_deploy(&record)?;
            let admitted = admission(root, &deploy)?;
            place(admitted, home, cancel, progress, Saved::new(root, record))
        }
        Phase::Stopping => {
            let client = client(root)?;
            let mut saved = Saved::new(root, record);
            progress(Progress::Requesting);
            let (mut operation, mut journal) = saved.parts();
            let stopped = browser_vm::stop(
                &client,
                &mut Fences {
                    operation: &mut operation,
                    journal: &mut journal,
                },
                &mut saved,
                cancel,
            );
            saved.keep_journal(&journal);
            let stopped = saved.finish(stopped);
            saved.settled(stopped, Phase::Stopped, home)
        }
        Phase::Deleting => {
            let client = client(root)?;
            let mut saved = Saved::new(root, record);
            let (mut operation, mut journal) = saved.parts();
            let deleted = browser_vm::delete(
                &client,
                &mut Fences {
                    operation: &mut operation,
                    journal: &mut journal,
                },
                &mut saved,
                cancel,
            );
            saved.keep_journal(&journal);
            saved.record.operation = operation;
            saved.record.journal = journal;
            let deleted = saved.finish(deleted);
            saved.settled(deleted, Phase::Deleted, home)
        }
        Phase::Ready | Phase::Stopped | Phase::Deleted | Phase::Failed | Phase::Idle => Err(Error::Invalid(
            "No browser VM run is recorded; begin one before running it",
        )),
    }
}

/// Carries out one phase of a container on this computer.
///
/// A local machine is `LocalBrowserVm` of this crate: a start writes the managed ssh entry and
/// reports the loopback address of the display, a stop keeps the state volume, and a delete
/// removes the container and keeps the volume, because the volume holds the sign-ins.
fn run_local(root: &Path, home: Option<&Path>, mut record: Record) -> Result<Status> {
    let vm = local_driver(root, home);
    let result = match record.phase {
        Phase::Deploying | Phase::Resuming => vm.up(None).map(|route| {
            record.phase = Phase::Ready;
            record.ssh_target = Some(route.ssh_target);
            record.ssh_note = route.note;
            record.vnc_address = Some(vm.vnc_address());
        }),
        Phase::Stopping => vm.down().map(|()| record.phase = Phase::Stopped),
        Phase::Deleting => vm.down().map(|()| record.phase = Phase::Deleted),
        Phase::Ready | Phase::Stopped | Phase::Deleted | Phase::Failed | Phase::Idle => {
            return Err(Error::Invalid(
                "No browser VM run is recorded; begin one before running it",
            ));
        }
    };
    match result {
        Ok(()) => record.error = None,
        Err(error) => {
            record.phase = Phase::Failed;
            record.error = Some(error.to_string());
        }
    }
    record.save(root)?;
    Ok(record.status())
}

/// The driver of the container of this Horizon. `root` is the cloud root, which holds the
/// driver under `browser-vm`, the same folder that `horizon --browser-vm-local` uses, so both
/// callers reach one container with one key.
fn local_driver(root: &Path, home: Option<&Path>) -> local::LocalBrowserVm {
    local::LocalBrowserVm::new(&root.join("browser-vm"), home)
}

#[cfg(all(test, unix))]
mod local_tests {
    use super::local_driver;
    use std::path::Path;

    #[test]
    fn the_tool_and_the_command_share_the_key_of_the_container() {
        // `horizon --browser-vm-local` builds its driver at `<cloud>/browser-vm`, and this
        // module must reach the same folder, or the two callers make two keys for one
        // container and the ssh entry of one names a key the other refuses.
        let cloud = Path::new("/home/person/.horizon/cloud");
        let command = crate::cloud_runtime::browser_vm::local::LocalBrowserVm::new(&cloud.join("browser-vm"), None);
        assert_eq!(
            local_driver(cloud, None).key_path(),
            command.key_path(),
            "one key for the container of both callers"
        );
        assert_eq!(
            local_driver(cloud, None).key_path(),
            cloud.join("browser-vm/browser-vm-local/id_ed25519")
        );
    }
}

/// The request the record names, so a resume rents the same machine and volume.
fn recorded_deploy(record: &Record) -> Result<Deploy> {
    if record.provider == Provider::Local {
        return Ok(Deploy {
            provider: Provider::Local,
            image: local::DEFAULT_IMAGE.to_owned(),
            server_type: String::new(),
            location: String::new(),
            volume_gb: 0,
        });
    }
    Ok(Deploy {
        provider: Provider::Hetzner,
        image: record
            .image
            .clone()
            .ok_or(Error::Invalid("No browser VM was ever rented; deploy one first"))?,
        server_type: record
            .server_type
            .clone()
            .ok_or(Error::Invalid("The record names no server type; deploy a browser VM"))?,
        location: record
            .location
            .clone()
            .ok_or(Error::Invalid("The record names no location; deploy a browser VM"))?,
        volume_gb: record.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
    })
}

/// Places the recorded machine through the shared rental code and records what it is, and the
/// ssh entry that reaches it.
fn place(
    admitted: Admitted,
    home: Option<&Path>,
    cancel: &Cancellation,
    progress: &mut dyn FnMut(Progress),
    mut saved: Saved<'_>,
) -> Result<Status> {
    let Admitted {
        client,
        request,
        login,
        identity,
    } = admitted;
    let (mut operation, mut journal) = saved.parts();
    let placed = browser_vm::deploy(
        &client,
        &request,
        login,
        Fences {
            operation: &mut operation,
            journal: &mut journal,
        },
        &mut saved,
        cancel,
        progress,
    );
    saved.keep_journal(&journal);
    saved.record.operation = operation;
    saved.record.journal = journal;
    if let Ok(machine) = &placed {
        let (target, note) = ssh_entry::machine_route(home, saved.root, &machine.ssh_target, &identity);
        saved.record.ssh_target = Some(target);
        saved.record.ssh_note = note;
        saved.record.server_id = Some(machine.server_id.clone());
        saved.record.hourly_net = machine.hourly_net;
    }
    let placed = saved.finish(placed);
    saved.settled(placed, Phase::Ready, home)
}

/// The settings of this machine, checked before any request is made: the client, the request,
/// the login of a private image, and the identity that opens the machine.
struct Admitted {
    client: Hetzner,
    request: Request,
    login: Option<RegistryLogin>,
    identity: PathBuf,
}

fn admission(root: &Path, deploy: &Deploy) -> Result<Admitted> {
    if !deploy.server_type.starts_with(SERVER_TYPE_PREFIX) {
        return Err(Error::Invalid(
            "A browser VM needs a dedicated ccx server type; the shared cores of a Hetzner account are exhausted, and the image needs a real machine",
        ));
    }
    if !(10..=10_240).contains(&deploy.volume_gb) {
        return Err(Error::Invalid(
            "A browser VM state volume must be between 10 and 10,240 GB",
        ));
    }
    let settings = Settings::load(&root.join("settings.json"))?;
    let hetzner = settings.hetzner.as_ref().ok_or(Error::Invalid(
        "Add a hetzner section to the cloud settings before renting a browser VM",
    ))?;
    hetzner.validate()?;
    if !hetzner.locations_for(None)?.contains(&deploy.location) {
        return Err(Error::Invalid(
            "The browser VM location must be one the Hetzner settings allow",
        ));
    }
    let login = super::deployment::hetzner::pull_login(hetzner, settings.registries.as_ref(), &deploy.image)?;
    let request = Request {
        image: deploy.image.clone(),
        server_type: deploy.server_type.clone(),
        location: deploy.location.clone(),
        volume_gb: deploy.volume_gb,
        public_key: public_key(&settings)?,
    };
    // The spec is built here, so a request the provider would refuse names its reason
    // before the record claims a run.
    request.spec()?;
    Ok(Admitted {
        client: Hetzner::new(hetzner.credential()?),
        request,
        login,
        identity: settings.ssh_identity_file.clone(),
    })
}

fn client(root: &Path) -> Result<Hetzner> {
    let settings = Settings::load(&root.join("settings.json"))?;
    let hetzner = settings.hetzner.as_ref().ok_or(Error::Invalid(
        "Add a hetzner section to the cloud settings before renting a browser VM",
    ))?;
    Ok(Hetzner::new(hetzner.credential()?))
}

/// The public key that opens the machine: the one beside the identity the cloud settings
/// name, which is the pair the person signs in with.
fn public_key(settings: &Settings) -> Result<String> {
    let mut path = settings.ssh_identity_file.clone().into_os_string();
    path.push(".pub");
    let key = std::fs::read_to_string(PathBuf::from(path))?;
    let key = key.trim();
    if !horizon_cloud::valid_public_key(key) {
        return Err(Error::Invalid(
            "The SSH public key beside the identity is not a supported Ed25519 key; Cloud settings can make a new pair",
        ));
    }
    Ok(key.to_owned())
}

/// The record, saved durably before each provider request it guards.
struct Saved<'a> {
    root: &'a Path,
    record: Record,
    failed: Option<Error>,
}

impl<'a> Saved<'a> {
    fn new(root: &'a Path, record: Record) -> Self {
        Self {
            root,
            record,
            failed: None,
        }
    }

    /// The fences the provider runtime takes by reference, copied out of the record. The
    /// record itself is then free to be saved through [`Records`].
    fn parts(&self) -> (CreateState, Journal) {
        (self.record.operation.clone(), self.record.journal.clone())
    }

    /// Keeps the journal the provider runtime left in place, as its last record saved it.
    fn keep_journal(&mut self, journal: &Journal) {
        self.record.journal = journal.clone();
    }

    fn save(&self) -> Result<()> {
        self.record.save(self.root)
    }

    /// Saves the phase that `result` ends in, or the failure, and reports the status. A run
    /// that ends in a phase without a machine removes the ssh entry of that machine, so no
    /// route names a host that is gone.
    fn settled<T>(mut self, result: Result<T>, phase: Phase, home: Option<&Path>) -> Result<Status> {
        match result {
            Ok(_) => {
                self.record.phase = phase;
                self.record.error = None;
                if phase != Phase::Ready {
                    self.record.ssh_target = None;
                    self.record.server_id = None;
                    self.record.ssh_note = ssh_entry::withdraw(home, self.root);
                }
            }
            Err(error) => {
                self.record.phase = Phase::Failed;
                self.record.error = Some(error.to_string());
            }
        }
        let status = self.record.status();
        self.record.save(self.root)?;
        Ok(status)
    }

    /// The result of a provider call made with these records, with a failed save reported
    /// as itself rather than as the provider layer's generic refusal.
    fn finish<T>(&mut self, result: std::result::Result<T, horizon_cloud::CloudError>) -> Result<T> {
        result.map_err(|error| match (error, self.failed.take()) {
            (horizon_cloud::CloudError::Persistence, Some(failed)) => failed,
            (error, _) => error.into(),
        })
    }

    fn keep(&mut self, saved: Result<()>) -> std::result::Result<(), horizon_cloud::CloudError> {
        saved.map_err(|error| {
            self.failed = Some(error);
            horizon_cloud::CloudError::Persistence
        })
    }
}

impl Records for Saved<'_> {
    fn journal(&mut self, journal: &Journal) -> std::result::Result<(), horizon_cloud::CloudError> {
        self.record.journal = journal.clone();
        let saved = self.save();
        self.keep(saved)
    }

    fn operation(&mut self, operation: &CreateState) -> std::result::Result<(), horizon_cloud::CloudError> {
        self.record.operation = operation.clone();
        let saved = self.save();
        self.keep(saved)
    }
}

impl StopRecords for Saved<'_> {
    fn stop(&mut self, stop: Stop) -> std::result::Result<(), horizon_cloud::CloudError> {
        self.record.phase = match stop {
            Stop::Stopping => Phase::Stopping,
            Stop::Stopped => Phase::Stopped,
        };
        let saved = self.save();
        self.keep(saved)
    }
}

#[cfg(test)]
mod tests;

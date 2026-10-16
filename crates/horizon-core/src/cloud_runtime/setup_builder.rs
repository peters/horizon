//! The rented builder of setups: a Hetzner server with Docker that Horizon rents while
//! setups build on it and deletes when none does. Its login key and its SSH host key are
//! made here for it alone and kept in a private folder of the cloud root, and a journal
//! beside them names the server from before its create request until it is proven gone,
//! so a builder that a crash or a closed Horizon left behind is found and deleted.
use super::{
    Cancellation, Error, Result,
    docker_host::{Access, Host, Pinned},
    settings::Settings,
};
use horizon_cloud::{
    CreateState,
    docker_host::{Docker, Engine},
    hetzner::{
        Hetzner,
        builder::{self, HostKey},
        catalog::Catalog,
        servers::Placement,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

/// The builder's name as a Docker host, which proofs and their leftovers record.
pub const NAME: &str = horizon_cloud::docker_host::RENTED_BUILDER;
/// The folder of the builder's keys and Docker configuration in the cloud root.
const FOLDER: &str = "setup-builder";
/// The journal in the cloud root.
const JOURNAL: &str = "setup-builder.json";
/// The lock in the cloud root that the process owning the builder holds.
const LOCK: &str = "setup-builder.lock";
/// How long a claim waits for a lock that another process holds only for a moment.
const CLAIM_WINDOW: Duration = Duration::from_secs(1);
/// A server that a create request made is listed within seconds; one not listed this
/// long after the request was never made.
const UNRESOLVED_FOR: Duration = Duration::from_mins(10);
/// The least a builder has, so it builds a large repository faster than a laptop.
const MIN_CORES: u32 = 8;
const MIN_MEMORY_GB: f64 = 16.0;
const MIN_DISK_GB: u32 = 160;
/// The server types and places tried, cheapest first.
const MAX_PLACEMENTS: usize = 6;
/// How long a new server may take until its Docker answers over SSH.
const READY_WITHIN: Duration = Duration::from_mins(5);
/// Where the docker-ce image installs docker.
const DOCKER: &str = "/usr/bin/docker";

/// A rented builder that answers.
#[derive(Clone, Debug, PartialEq)]
pub struct Builder {
    /// The builder as a Docker host, reached only with its own keys.
    pub host: Host,
    pub server_type: String,
    pub location: String,
    /// The server and its IPv4 address per started hour, in euro before tax.
    pub hourly_eur: f64,
}

/// Where renting a builder is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Choosing,
    Creating,
    Starting,
}

#[derive(Debug, Deserialize, Serialize)]
struct Journal {
    operation_id: String,
    state: CreateState,
    /// When the create request went out, in seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_at: Option<u64>,
    /// The fingerprint of the Hetzner token the builder was rented with: a token of
    /// another project finds no server, which must not count as one deleted.
    binding: String,
}

/// This process's claim on the builder of one cloud root: only the process that holds it
/// rents or deletes that builder, so another Horizon or a gate on the same root never
/// deletes a builder that proofs still use. The claim ends with the process.
pub struct Owner {
    _lock: std::fs::File,
    root: PathBuf,
}

impl Owner {
    /// The cloud root whose builder this is.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Claims the builder of `root`; `None` while another process holds the claim.
/// # Errors
/// When the lock file cannot be opened or locked.
pub fn claim(root: &Path) -> Result<Option<Owner>> {
    std::fs::create_dir_all(root)?;
    // The key paths SSH reads must be absolute.
    let root = &std::fs::canonicalize(root)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(root.join(LOCK))?;
    // A process this one starts holds the lock's descriptor from its fork until its exec,
    // so a claim that was let go a moment ago can look held for a few milliseconds.
    let deadline = Instant::now() + CLAIM_WINDOW;
    loop {
        match file.try_lock() {
            Ok(()) => {
                return Ok(Some(Owner {
                    _lock: file,
                    root: root.to_owned(),
                }));
            }
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}

/// Whether this machine can rent a builder: it has a Hetzner binding.
#[must_use]
pub fn available(settings: &Settings) -> bool {
    settings.hetzner.is_some()
}

/// Whether a builder may still exist: its journal is in `root`.
#[must_use]
pub fn left_behind(root: &Path) -> bool {
    root.join(JOURNAL).exists()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Rents a builder with the Hetzner binding of `settings` and waits until its Docker
/// answers. A builder whose renting fails is deleted again before this returns.
/// # Errors
/// Without a Hetzner binding, while an earlier builder may still exist, when no server
/// type in the allowed locations is large enough, and on provider and SSH failures.
pub fn rent(
    settings: &Settings,
    owner: &Owner,
    cancel: &Cancellation,
    mut stage: impl FnMut(Stage),
) -> Result<Builder> {
    let root = owner.root.as_path();
    let hetzner = settings
        .hetzner
        .as_ref()
        .ok_or(Error::Invalid("Connect Hetzner in Cloud settings to rent a builder"))?;
    if left_behind(root) {
        return Err(Error::Invalid("An earlier rented builder is still being deleted"));
    }
    stage(Stage::Choosing);
    // One read of the token, so the journal names the project the server is made in.
    let (credential, binding) = hetzner.credential_with_fingerprint()?;
    let client = Hetzner::new(credential);
    let (placements, hourly) = placements(&client.catalog(cancel)?, &hetzner.locations);
    if placements.is_empty() {
        return Err(Error::Invalid(
            "No Hetzner server type with 8 cores, 16 GB and 160 GB is offered in the allowed locations",
        ));
    }
    let folder = root.join(FOLDER);
    let operation_id = format!("setup-builder-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let mut create_state = CreateState::Prepared;
    write_journal(root, (&operation_id, &binding), &create_state, None)?;
    let mut requested_at = None;
    let rented = (|| {
        let keys = Keys::make(&folder)?;
        // Key paths SSH cannot take refuse here, before a server is billed.
        host(&keys, std::net::Ipv4Addr::new(192, 0, 2, 1)).validate()?;
        stage(Stage::Creating);
        let host_private = zeroize::Zeroizing::new(std::fs::read_to_string(&keys.host)?);
        let host_public = std::fs::read_to_string(keys.host.with_extension("pub"))?;
        let login_public = std::fs::read_to_string(keys.login.with_extension("pub"))?;
        let server = client.rent_builder(
            &builder::Request {
                operation_id: &operation_id,
                placements: &placements,
                login_key: login_public.trim(),
                host_key: HostKey {
                    private: &host_private,
                    public: &host_public,
                },
            },
            &mut create_state,
            cancel,
            |state| {
                if *state == CreateState::Requested {
                    requested_at = requested_at.or_else(|| Some(now()));
                }
                write_journal(root, (&operation_id, &binding), state, requested_at)
                    .map_err(|_| horizon_cloud::CloudError::Transport)
            },
        )?;
        let ip = server
            .public_net
            .ipv4
            .as_ref()
            .map(|ipv4| ipv4.ip)
            .ok_or(Error::Invalid("The rented builder has no IPv4 address"))?;
        write_private(&keys.known_hosts, &format!("{ip} {}\n", host_public.trim()))?;
        let host = host(&keys, ip);
        stage(Stage::Starting);
        wait_ready(&host, &folder.join("docker-config"), cancel)?;
        let hourly_eur = hourly
            .iter()
            .find(|(placement, _)| {
                placement.server_type == server.server_type.name && placement.location == server.location.name
            })
            .map_or(0.0, |(_, hourly)| *hourly);
        Ok(Builder {
            host,
            server_type: server.server_type.name.clone(),
            location: server.location.name.clone(),
            hourly_eur,
        })
    })();
    // Nothing of a failed renting is left: the server and its key go, then the journal.
    rented.map_err(
        |error: Error| match give_back(settings, owner, &Cancellation::default()) {
            Ok(_) => error,
            Err(cleanup) => Error::Cleanup {
                primary: Box::new(error),
                cleanup: Box::new(cleanup),
            },
        },
    )
}

/// Deletes the builder the journal of the owner's root names, with its login key, proves
/// both gone and removes its folder and journal. `Ok(false)` when there is no journal.
/// # Errors
/// Without a Hetzner binding, on provider failures and while a create request may still
/// make a server; the journal then stays for the next try.
pub fn give_back(settings: &Settings, owner: &Owner, cancel: &Cancellation) -> Result<bool> {
    let root = owner.root.as_path();
    let path = root.join(JOURNAL);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let journal: Journal = serde_json::from_str(&text).map_err(|_| Error::Json)?;
    let hetzner = settings.hetzner.as_ref().ok_or(Error::Invalid(
        "Connect Hetzner in Cloud settings again to delete the rented builder",
    ))?;
    let (credential, binding) = hetzner.credential_with_fingerprint()?;
    return_with(root, journal, (&Hetzner::new(credential), &binding), cancel)
}

/// Deletes the builder `journal` names with `client`, whose token has the fingerprint
/// `binding`, and removes its folder and the journal.
fn return_with(
    root: &Path,
    mut journal: Journal,
    (client, binding): (&Hetzner, &str),
    cancel: &Cancellation,
) -> Result<bool> {
    // Before the create request no server can exist, so any token may clear the journal.
    if binding != journal.binding && journal.state != CreateState::Prepared {
        // Another token. One of the same project, as after a rotation, still lists the
        // builder's server or its key; one of another project lists neither, and that
        // proves nothing, so the journal stays.
        let listed = !client.find_servers(&journal.operation_id, cancel)?.is_empty()
            || !client.find_ssh_keys(&journal.operation_id, cancel)?.is_empty();
        if !listed {
            return Err(Error::Invalid(
                "The Hetzner token in Cloud settings does not list the rented builder. Connect the project it was rented in; if you deleted its server and key yourself, delete setup-builder.json in the Cloud settings folder",
            ));
        }
        binding.clone_into(&mut journal.binding);
        write_journal(
            root,
            (&journal.operation_id, binding),
            &journal.state,
            journal.requested_at,
        )?;
    }
    let operation_id = journal.operation_id.clone();
    let binding = journal.binding.clone();
    let requested_at = journal.requested_at;
    let settled = requested_at.is_some_and(|at| now().saturating_sub(at) >= UNRESOLVED_FOR.as_secs());
    client.return_builder(&operation_id, &mut journal.state, settled, cancel, |state| {
        write_journal(root, (&operation_id, &binding), state, requested_at)
            .map_err(|_| horizon_cloud::CloudError::Transport)
    })?;
    match std::fs::remove_dir_all(root.join(FOLDER)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    std::fs::remove_file(root.join(JOURNAL))?;
    Ok(true)
}

/// The placements to try, cheapest first, with each one's price per hour.
fn placements(catalog: &Catalog, locations: &[String]) -> (Vec<Placement>, Vec<(Placement, f64)>) {
    let mut offers: Vec<(Placement, f64, bool)> = catalog
        .offers
        .iter()
        .filter(|offer| {
            locations.contains(&offer.location)
                && offer.cores >= MIN_CORES
                && offer.memory_gb >= MIN_MEMORY_GB
                && offer.disk_gb >= MIN_DISK_GB
        })
        .filter_map(|offer| {
            let ipv4 = catalog.ipv4_hour_eur.get(&offer.location)?;
            Some((
                Placement {
                    server_type: offer.server_type.clone(),
                    location: offer.location.clone(),
                },
                offer.hourly_eur + ipv4,
                offer.available,
            ))
        })
        .collect();
    // Listed as orderable first, then the cheapest.
    offers.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.total_cmp(&b.1)));
    offers.truncate(MAX_PLACEMENTS);
    let hourly: Vec<(Placement, f64)> = offers
        .into_iter()
        .map(|(placement, hourly, _)| (placement, hourly))
        .collect();
    (hourly.iter().map(|(placement, _)| placement.clone()).collect(), hourly)
}

/// The builder at `ip` as a Docker host, reached only with its own keys.
fn host(keys: &Keys, ip: std::net::Ipv4Addr) -> Host {
    Host {
        name: NAME.to_owned(),
        access: Access::Ssh,
        address: Some(format!("root@{ip}")),
        docker: Some(DOCKER.to_owned()),
        pinned: Some(Pinned {
            identity: keys.login.clone(),
            known_hosts: keys.known_hosts.clone(),
        }),
    }
}

/// The builder's key files in its private folder.
struct Keys {
    login: PathBuf,
    host: PathBuf,
    known_hosts: PathBuf,
}

impl Keys {
    /// New Ed25519 keys without passphrase in a new private `folder`.
    fn make(folder: &Path) -> Result<Self> {
        match std::fs::remove_dir_all(folder) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        std::fs::create_dir_all(folder)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
        }
        let keys = Self {
            login: folder.join("login"),
            host: folder.join("host"),
            known_hosts: folder.join("known_hosts"),
        };
        for (path, comment) in [
            (&keys.login, "horizon-setup-builder-login"),
            (&keys.host, "horizon-setup-builder-host"),
        ] {
            let status = Command::new("ssh-keygen")
                .env_remove("SSH_AUTH_SOCK")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
                .arg(path)
                .stdin(std::process::Stdio::null())
                .status()
                .map_err(|_| Error::Invalid("ssh-keygen is missing; install OpenSSH to rent a builder"))?;
            if !status.success() {
                return Err(Error::Command("ssh-keygen"));
            }
        }
        Ok(keys)
    }
}

/// Waits until the builder's Docker answers over SSH with its pinned keys.
fn wait_ready(host: &Host, local_config: &Path, cancel: &Cancellation) -> Result<()> {
    let engine = Engine {
        host,
        local_config: local_config.to_owned(),
        cancel,
        engine_id: std::sync::Mutex::new(None),
    };
    let deadline = Instant::now() + READY_WITHIN;
    let version = [
        "version".to_owned(),
        "--format".to_owned(),
        "{{.Server.Version}}".to_owned(),
    ];
    loop {
        cancel.check()?;
        if engine.docker(&version, Duration::from_secs(20)).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Invalid(
                "The rented builder's Docker did not answer within 5 minutes",
            ));
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

fn write_journal(
    root: &Path,
    (operation_id, binding): (&str, &str),
    state: &CreateState,
    requested_at: Option<u64>,
) -> Result<()> {
    let journal = Journal {
        operation_id: operation_id.to_owned(),
        state: state.clone(),
        requested_at,
        binding: binding.to_owned(),
    };
    let text = serde_json::to_string_pretty(&journal).map_err(|_| Error::Json)?;
    write_private(&root.join(JOURNAL), &text)
}

/// Writes `text` to `path` for this user alone, through a temporary file synced first,
/// and syncs the folder after the rename, so a power loss keeps the new text.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    if let Some(folder) = path.parent() {
        std::fs::File::open(folder)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;

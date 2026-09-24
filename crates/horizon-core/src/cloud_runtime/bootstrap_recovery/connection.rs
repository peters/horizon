use super::{Error, Result, Target};
use crate::cloud_runtime::ssh::Connection;
use horizon_cloud_protocol::bootstrap::Startup;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::{
    fs::File,
    io::{Read, Write},
    net::IpAddr,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::cloud_runtime) struct Binding {
    pub startup: Startup,
    pub worker_id: String,
    identity: PathBuf,
    known_hosts: PathBuf,
    identity_hash: [u8; 32],
    host_pin_hash: [u8; 32],
}

struct Material {
    binding: Binding,
    key: Zeroizing<Vec<u8>>,
    hosts: Zeroizing<Vec<u8>>,
}

/// Original credential paths can change while SSH starts. Retain private copies
/// of the exact verified bytes until the transport has exited.
pub(in crate::cloud_runtime) struct Snapshot {
    pub binding: Binding,
    pub connection: Connection,
    _key: tempfile::NamedTempFile,
    _hosts: tempfile::NamedTempFile,
    _directory: tempfile::TempDir,
}

impl Snapshot {
    pub fn capture(target: &Target) -> Result<Self> {
        let material = Material::read(target)?;
        let directory = tempfile::Builder::new().prefix("horizon-recovery-").tempdir()?;
        let mut key = tempfile::NamedTempFile::new_in(directory.path())?;
        key.write_all(&material.key)?;
        let mut hosts = tempfile::NamedTempFile::new_in(directory.path())?;
        hosts.write_all(&material.hosts)?;
        Ok(Self {
            binding: material.binding,
            connection: Connection {
                host: target.connection.host.clone(),
                port: target.connection.port,
                identity: key.path().to_owned(),
                known_hosts: hosts.path().to_owned(),
                host_key_alias: format!("horizon-cloud-{}", target.worker_id),
            },
            _key: key,
            _hosts: hosts,
            _directory: directory,
        })
    }
}

impl Binding {
    pub fn matches_identity(&self, path: &Path, hash: &[u8; 32]) -> bool {
        self.identity == path && self.identity_hash == *hash
    }
    pub fn capture(target: &Target) -> Result<Self> {
        Ok(Material::read(target)?.binding)
    }
}

impl Material {
    fn read(target: &Target) -> Result<Self> {
        target.startup.validate().map_err(|_| Error::Invalid)?;
        let connection = &target.connection;
        if !horizon_cloud::valid_id(&target.worker_id)
            || connection.host.parse::<IpAddr>().is_err()
            || connection.port == 0
            || connection.host_key_alias != format!("horizon-cloud-{}", target.worker_id)
        {
            return Err(Error::Invalid);
        }
        let (identity, key) = read(&connection.identity, true)?;
        let (known_hosts, source_hosts) = read(&connection.known_hosts, false)?;
        let hosts = exact_pins(&source_hosts, &connection.host_key_alias)?;
        // Use OpenSSH's parser to check algorithm names, base64 and complete key
        // fields before an unusable pin can become part of an anchored request.
        let cancel = super::Cancellation::default();
        let runner = super::Runner {
            cancel: &cancel,
            emit: &|_| {},
            secrets: Vec::new(),
        };
        let fingerprints = runner
            .private_exchange(
                Command::new("ssh-keygen").args(["-l", "-f", "-"]),
                &hosts,
                Duration::from_secs(5),
            )
            .map_err(|_| Error::Invalid)?;
        let validated = std::str::from_utf8(&fingerprints)
            .map_err(|_| Error::Invalid)?
            .lines()
            .count();
        let expected = std::str::from_utf8(&hosts).map_err(|_| Error::Invalid)?.lines().count();
        if validated != expected {
            return Err(Error::Invalid);
        }
        // Recovery supports noninteractive, unencrypted private identities.
        // Validate captured bytes, never a mutable source path or agent fallback.
        public_identity(&key, &runner)?;
        Ok(Self {
            binding: Binding {
                startup: target.startup.clone(),
                worker_id: target.worker_id.clone(),
                identity,
                known_hosts,
                identity_hash: Sha256::digest(&key).into(),
                host_pin_hash: Sha256::digest(&hosts).into(),
            },
            key,
            hosts,
        })
    }
}

/// Parsing the embedded public field alone does not prove private-seed integrity.
pub(in crate::cloud_runtime) fn public_identity(bytes: &[u8], runner: &super::Runner<'_>) -> Result<String> {
    const PROBE: &[u8] = b"horizon bootstrap private identity validation v1\n";
    const NAMESPACE: &str = "horizon-bootstrap-identity";
    let mut key = tempfile::NamedTempFile::new()?;
    key.write_all(bytes)?;
    let public = runner
        .private_exchange(
            Command::new("ssh-keygen")
                .env_remove("SSH_AUTH_SOCK")
                .args(["-y", "-P", "", "-f"])
                .arg(key.path()),
            &[],
            Duration::from_secs(5),
        )
        .map_err(|_| Error::Invalid)?;
    let public = std::str::from_utf8(&public).map_err(|_| Error::Invalid)?.trim();
    let signature = runner
        .private_exchange(
            Command::new("ssh-keygen")
                .env_remove("SSH_AUTH_SOCK")
                .args(["-Y", "sign", "-n", NAMESPACE, "-f"])
                .arg(key.path()),
            PROBE,
            Duration::from_secs(5),
        )
        .map_err(|_| Error::Invalid)?;
    let mut signed = tempfile::NamedTempFile::new()?;
    signed.write_all(&signature)?;
    let mut allowed = tempfile::NamedTempFile::new()?;
    writeln!(allowed, "bootstrap {public}")?;
    runner
        .private_exchange(
            Command::new("ssh-keygen")
                .env_remove("SSH_AUTH_SOCK")
                .args(["-Y", "verify", "-n", NAMESPACE, "-I", "bootstrap", "-f"])
                .arg(allowed.path())
                .arg("-s")
                .arg(signed.path()),
            PROBE,
            Duration::from_secs(5),
        )
        .map_err(|_| Error::Invalid)?;
    Ok(public.to_owned())
}

fn exact_pins(bytes: &[u8], alias: &str) -> Result<Zeroizing<Vec<u8>>> {
    let source = std::str::from_utf8(bytes).map_err(|_| Error::Invalid)?;
    let mut pins = String::new();
    for line in source.lines() {
        let mut fields = line.split_whitespace();
        let Some(host) = fields.next() else { continue };
        if host.starts_with('#') {
            continue;
        }
        // A dedicated pin file cannot expand trust through patterns, host lists,
        // hashed aliases, authorities or revocations discarded by normalization.
        if host != alias {
            return Err(Error::Invalid);
        }
        let kind = fields.next().ok_or(Error::Invalid)?;
        let key = fields.next().ok_or(Error::Invalid)?;
        writeln!(&mut pins, "{alias} {kind} {key}").map_err(|_| Error::Invalid)?;
    }
    if pins.is_empty() {
        return Err(Error::Invalid);
    }
    Ok(Zeroizing::new(pins.into_bytes()))
}

pub(in crate::cloud_runtime) fn read(path: &Path, private: bool) -> Result<(PathBuf, Zeroizing<Vec<u8>>)> {
    read_inner(path, private, false)
}

pub(in crate::cloud_runtime) fn read_empty(path: &Path) -> Result<(PathBuf, Zeroizing<Vec<u8>>)> {
    read_inner(path, false, true)
}

fn read_inner(path: &Path, private: bool, empty: bool) -> Result<(PathBuf, Zeroizing<Vec<u8>>)> {
    if !path.is_absolute() || std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(Error::Invalid);
    }
    #[cfg(unix)]
    let mut file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(std::io::Error::from)?,
    );
    #[cfg(not(unix))]
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || (!empty && metadata.len() == 0) || metadata.len() > 64 * 1024 {
        return Err(Error::Invalid);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & if private { 0o077 } else { 0o022 } != 0
        {
            return Err(Error::Invalid);
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    // Fixed storage prevents reallocations from leaving private-key fragments.
    let mut bytes = Zeroizing::new(vec![0; 64 * 1024 + 1]);
    let mut length = 0;
    while length < bytes.len() {
        let read = file.read(&mut bytes[length..])?;
        if read == 0 {
            break;
        }
        length += read;
    }
    if length > 64 * 1024 || (!empty && length == 0) {
        return Err(Error::Invalid);
    }
    bytes.truncate(length);
    Ok((path.canonicalize()?, bytes))
}

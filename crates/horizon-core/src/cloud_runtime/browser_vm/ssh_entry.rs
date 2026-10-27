//! The managed SSH entry of the browser VM: one host block in the ssh configuration of the
//! person, and the host keys of the machine that this Horizon trusts.
//!
//! The machine is rented with one authorized key, the one beside `ssh_identity_file` in the
//! cloud settings, so a route that presents any other key cannot open it. Horizon writes one
//! block that names the alias, that key and a `known_hosts` file of its own, and scans the host
//! keys of the machine into that file once the machine is up. Every route that runs
//! `ssh horizon-browser-vm` then reaches the machine: the browser bridge, the `device_panel`
//! tool and a terminal.
//!
//! The home of the person is a parameter and never an environment read, so a test works in a
//! temporary home and never touches the ssh configuration of the developer.
use std::{
    borrow::Cow,
    fmt::Write as _,
    io::Write as _,
    ops::Range,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use super::super::{Error, Result};

/// The alias that every route to the machine names.
pub(super) const ALIAS: &str = "horizon-browser-vm";
/// The account of the machine: the image has no other login.
const USER: &str = "root";
/// The `known_hosts` file of this Horizon, in its cloud root. It holds the keys of this one
/// machine and of no other host.
pub(super) const KNOWN_HOSTS: &str = "browser-vm-known-hosts";
/// How the block opens, and the line that marks it as the block of Horizon.
const BEGIN: &str = "# >>> horizon browser vm; written by Horizon, do not edit";
/// How the block closes.
const END: &str = "# <<< horizon browser vm";

/// One host block of the ssh configuration of the person, and the `known_hosts` file of this
/// Horizon that trusts the machine it names.
pub(super) struct Entry<'a> {
    /// The directory that holds `.ssh`, which is the home of the person.
    home: &'a Path,
    /// The cloud root of this Horizon, which holds the managed `known_hosts` file.
    root: &'a Path,
    scan: HostKeyScan,
}

/// What a written entry holds beyond its block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Trust {
    /// Why the host keys of the machine are not in the managed file, when they are not. The
    /// entry itself is written, so the machine is still reachable.
    pub note: Option<String>,
}

/// How the host keys of a machine are scanned.
#[derive(Clone, Debug)]
pub(super) struct HostKeyScan {
    /// The program: `ssh-keyscan` of the OpenSSH tools beside `ssh`.
    pub program: PathBuf,
    /// How many attempts are made, because the sshd of a machine that has just started may
    /// not answer yet.
    pub attempts: u32,
    /// How long one attempt waits for the machine to answer, as `ssh-keyscan -T` takes it.
    pub connect_timeout_secs: u16,
    /// How long the scan waits before another attempt.
    pub pause: Duration,
}

impl Default for HostKeyScan {
    fn default() -> Self {
        Self {
            program: PathBuf::from("ssh-keyscan"),
            // A machine that Hetzner reports as running has not necessarily started its sshd:
            // a live resume failed every attempt of a four attempt scan, so the scan waits long
            // enough to cover boot to sshd and stops as soon as a key arrives.
            attempts: 24,
            connect_timeout_secs: 5,
            pause: Duration::from_secs(5),
        }
    }
}

impl<'a> Entry<'a> {
    pub(super) fn new(home: &'a Path, root: &'a Path) -> Self {
        Self {
            home,
            root,
            scan: HostKeyScan::default(),
        }
    }

    /// Writes the block for `address` where the first host section of the ssh configuration
    /// begins, so that its values win over a `Host *` below it while the options that a person
    /// sets for every host stay above it, and puts the host keys of that address in the
    /// `known_hosts` file of this Horizon.
    /// # Errors
    /// Refuses a configuration that already names the alias outside the block, and reports a
    /// configuration, or a `known_hosts` file, that cannot be made durable.
    pub(super) fn write(&self, address: &str, identity: &Path) -> Result<Trust> {
        let config = self.target()?;
        let existing = read(&config)?;
        let block = self.block(address, identity)?;
        let kept = without_block(&existing)?;
        // The block goes where the first host section begins, never above the options that a
        // person sets for every host: a comment does not end a Host section, so a block that
        // came first would take those options for itself.
        let at = first_host_section(&kept).unwrap_or(0);
        let mut written = Vec::with_capacity(block.len() + kept.len());
        written.extend_from_slice(&kept[..at]);
        written.extend_from_slice(&block);
        written.extend_from_slice(&kept[at..]);
        if written != existing {
            private_folder(&self.folder())?;
            store(&config, &written)?;
        }
        Ok(Trust {
            note: self.trust(address)?,
        })
    }

    /// Removes the block and the `known_hosts` file of this Horizon. Every line outside the
    /// block, and the hosts and comments of the person, stay as they were.
    /// # Errors
    /// Refuses a configuration that names the alias outside the block, and reports a
    /// configuration that cannot be made durable.
    pub(super) fn remove(&self) -> Result<()> {
        let config = self.target()?;
        let existing = read(&config)?;
        let kept = without_block(&existing)?;
        if kept != existing {
            if kept.is_empty() {
                remove_file(&config)?;
            } else {
                store(&config, &kept)?;
            }
        }
        remove_file(&self.known_hosts())
    }

    fn folder(&self) -> PathBuf {
        self.home.join(".ssh")
    }

    fn config(&self) -> PathBuf {
        self.folder().join("config")
    }

    /// The file that holds the configuration: the target of the link when the person manages
    /// `.ssh/config` with a dotfiles tool, so a write reaches their repository and the link is
    /// never replaced by a regular file.
    fn target(&self) -> Result<PathBuf> {
        let config = self.config();
        match std::fs::symlink_metadata(&config) {
            Ok(metadata) if metadata.file_type().is_symlink() => std::fs::canonicalize(&config).map_err(Error::from),
            Ok(_) => Ok(config),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(config),
            Err(error) => Err(error.into()),
        }
    }

    fn known_hosts(&self) -> PathBuf {
        self.root.join(KNOWN_HOSTS)
    }

    /// The block, exactly as Horizon writes it and nothing else.
    fn block(&self, address: &str, identity: &Path) -> Result<Vec<u8>> {
        let identity = absolute(identity)?;
        let known = absolute(&self.known_hosts())?;
        let values = [
            address.to_owned(),
            identity.to_string_lossy().into_owned(),
            known.to_string_lossy().into_owned(),
        ];
        if values.iter().any(|value| !one_line(value)) || !host_name(address) {
            return Err(Error::Invalid(
                "The browser VM has no address or identity that the ssh configuration can take on one line",
            ));
        }
        Ok(format!(
            "{BEGIN}\nHost {ALIAS}\n  HostName {address}\n  User {USER}\n  IdentityFile {}\n  \
             IdentitiesOnly yes\n  UserKnownHostsFile {}\n  HostKeyAlias {ALIAS}\n{END}\n",
            values[1], values[2]
        )
        .into_bytes())
    }

    /// Puts the keys of `address` in the `known_hosts` file of this Horizon, and reports the
    /// reason they are not there instead of failing a deploy that already rented the machine.
    fn trust(&self, address: &str) -> Result<Option<String>> {
        let keys = match self.scan_keys(address) {
            Ok(keys) => keys,
            Err(reason) => return Ok(Some(reason)),
        };
        let known = self.known_hosts();
        if read(&known)? == keys {
            // The machine is already trusted with these keys, so a second deploy adds nothing.
            return Ok(None);
        }
        store(&known, &keys)?;
        Ok(None)
    }

    /// The keys that `address` presents, keyed by the alias, or the reason no key arrived.
    fn scan_keys(&self, address: &str) -> std::result::Result<Vec<u8>, String> {
        let mut reason = String::new();
        for attempt in 0..self.scan.attempts {
            if attempt > 0 {
                std::thread::sleep(self.scan.pause);
            }
            let output = Command::new(&self.scan.program)
                .arg("-T")
                .arg(self.scan.connect_timeout_secs.to_string())
                .arg(address)
                .stdin(Stdio::null())
                .output();
            let output = match output {
                Ok(output) => output,
                // The program cannot run at all, so no further attempt can succeed.
                Err(error) => return Err(scan_reason(address, &error.to_string())),
            };
            let keys = host_keys(&output.stdout);
            if !keys.is_empty() {
                return Ok(keys);
            }
            let silence = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            reason = scan_reason(
                address,
                &if silence.is_empty() {
                    "the machine answered with no host key".to_owned()
                } else {
                    silence
                },
            );
        }
        Err(reason)
    }
}

#[cfg(test)]
impl<'a> Entry<'a> {
    pub(super) fn with_scan(home: &'a Path, root: &'a Path, scan: HostKeyScan) -> Self {
        Self { home, root, scan }
    }
}

/// Why the host keys of a machine are not in the managed file, as the entry reports it.
fn scan_reason(address: &str, reason: &str) -> String {
    format!(
        "Horizon wrote the ssh entry {ALIAS}, but the host keys of {address} are not trusted yet: \
         {reason}. A person compares the key of the machine out of band at the first sign-in"
    )
}

/// The configuration without the block Horizon writes. A line outside the block that names the
/// alias belongs to the person, and Horizon never changes such a file.
fn without_block(config: &[u8]) -> Result<Vec<u8>> {
    let block = block_range(config);
    for (offset, line) in lines(config) {
        let inside = block.as_ref().is_some_and(|block| block.contains(&offset));
        if !inside && names_alias(&line_text(line)) {
            return Err(Error::Invalid(
                "The ssh configuration of this computer names Host horizon-browser-vm outside the block \
                 Horizon writes, so Horizon changed nothing and the machine has no managed entry",
            ));
        }
    }
    Ok(match block {
        Some(block) => {
            let mut kept = Vec::with_capacity(config.len());
            kept.extend_from_slice(&config[..block.start]);
            kept.extend_from_slice(&config[block.end..]);
            kept
        }
        None => config.to_vec(),
    })
}

/// Where the first host section of a configuration begins, so a block can go there without
/// changing what the lines above it mean. A file with no section has no place to keep, so a
/// block that follows it stands at the top.
fn first_host_section(config: &[u8]) -> Option<usize> {
    lines(config).find_map(|(offset, line)| {
        let line = line_text(line);
        let keyword = line.split('#').next()?.trim().split_once(char::is_whitespace)?.0;
        (keyword.eq_ignore_ascii_case("host") || keyword.eq_ignore_ascii_case("match")).then_some(offset)
    })
}

/// The bytes of the block: from the opening marker through the line of the closing marker. A
/// marker without its pair is not a block, so nothing of the person is removed.
fn block_range(config: &[u8]) -> Option<Range<usize>> {
    let mut begin = None;
    for (offset, line) in lines(config) {
        let text = line_text(line);
        match text.trim() {
            BEGIN => begin = Some(offset),
            END => {
                if let Some(begin) = begin {
                    return Some(begin..offset + line.len());
                }
            }
            _ => {}
        }
    }
    None
}

/// Every line of `text` with the offset where it starts. A line keeps its terminator, so the
/// bytes outside a removed block are the bytes that were there.
fn lines(text: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    let mut offset = 0;
    text.split_inclusive(|byte| *byte == b'\n').map(move |line| {
        let start = offset;
        offset += line.len();
        (start, line)
    })
}

fn line_text(line: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(line)
}

/// Whether the line is a `Host` line that names the alias. ssh compares a host name without
/// case, so `Host Horizon-Browser-VM` is the same host to it.
fn names_alias(line: &str) -> bool {
    let line = line.split('#').next().unwrap_or_default().trim();
    let Some((keyword, patterns)) = line.split_once(char::is_whitespace) else {
        return false;
    };
    keyword.eq_ignore_ascii_case("host")
        && patterns
            .split(|character: char| character.is_whitespace() || character == ',')
            .any(|pattern| pattern.eq_ignore_ascii_case(ALIAS))
}

/// The key lines of a scan, keyed by the alias. ssh checks the key of a host against the name it
/// connected to, which is the address behind the entry, so the block names `HostKeyAlias`; the
/// alias is then both what a route types and what ssh looks the key up by, and the keys of a new
/// machine replace the keys of the last one without changing the file that other routes name.
fn host_keys(output: &[u8]) -> Vec<u8> {
    let mut keys = String::new();
    for line in line_text(output).lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let (Some(_host), Some(key_type), Some(key)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        if !host_key(key_type, key) {
            continue;
        }
        let _ = writeln!(keys, "{ALIAS} {key_type} {key}");
    }
    keys.into_bytes()
}

/// Whether `key_type` and `key` are a host key that a server offers: one of the algorithms of
/// OpenSSH, and the base64 blob of the key. A line of another shape is no host key and never
/// enters the file that ssh trusts.
fn host_key(key_type: &str, key: &str) -> bool {
    ["ssh-", "ecdsa-", "sk-"]
        .iter()
        .any(|algorithm| key_type.starts_with(algorithm))
        && key.len() > 32
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "+/=".contains(character))
}

/// Whether `address` is one host name that the ssh configuration takes on one line. An address
/// of another shape would add a line of its own to the configuration.
fn host_name(address: &str) -> bool {
    !address.is_empty()
        && address
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".:-_[]".contains(character))
}

fn one_line(value: &str) -> bool {
    !value.contains(['\n', '\r'])
}

fn absolute(path: &Path) -> Result<PathBuf> {
    std::path::absolute(path).map_err(Error::from)
}

/// The file at `path`, or nothing when there is none yet.
fn read(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// The `.ssh` folder of the person, at the 0700 that ssh requires of it.
fn private_folder(folder: &Path) -> Result<()> {
    std::fs::create_dir_all(folder)?;
    set_mode(folder, 0o700)
}

/// Writes `contents` into `path` at 0600, through a temporary file in the same folder, so a
/// reader sees the file before or after the change and never a part of it.
fn store(path: &Path, contents: &[u8]) -> Result<()> {
    let folder = path
        .parent()
        .ok_or(Error::Invalid("The ssh entry has no folder to write into"))?;
    std::fs::create_dir_all(folder)?;
    let mut file = tempfile::NamedTempFile::new_in(folder)?;
    file.write_all(contents)?;
    set_mode(file.path(), 0o600)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(folder)?.sync_all()?;
    Ok(())
}

fn remove_file(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// The SSH route of the machine that `target` names: the alias of the managed entry when
/// Horizon wrote it, else the target itself, with the reason the entry is missing.
pub(super) fn route(entry: &Entry<'_>, target: &str, identity: &Path) -> (String, Option<String>) {
    let address = target.split_once('@').map_or(target, |(_, address)| address);
    match entry.write(address, identity) {
        Ok(trust) => (ALIAS.to_owned(), trust.note),
        Err(error) => (target.to_owned(), Some(error.to_string())),
    }
}

/// [`route`] in the home of the person, or the reason no entry was written when this Horizon
/// does not know that home.
pub(super) fn machine_route(
    home: Option<&Path>,
    root: &Path,
    target: &str,
    identity: &Path,
) -> (String, Option<String>) {
    let Some(home) = home else {
        return (
            target.to_owned(),
            Some(
                "Horizon did not write the ssh entry horizon-browser-vm: it does not know the home \
                 directory of this computer, so every route names the address of the machine"
                    .to_owned(),
            ),
        );
    };
    route(&Entry::new(home, root), target, identity)
}

/// Removes the ssh entry and the host keys of a machine that is gone. The machine is already
/// gone, so a refusal is recorded in the answer and not raised.
pub(super) fn withdraw(home: Option<&Path>, root: &Path) -> Option<String> {
    let home = home?;
    Entry::new(home, root).remove().err().map(|error| error.to_string())
}

#[cfg(all(test, unix))]
mod tests;

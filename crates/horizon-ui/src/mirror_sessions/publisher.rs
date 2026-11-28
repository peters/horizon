//! The publisher of the session mirror: one process that reads the session stores
//! of this computer and sends what it reads to the server of its home.
//!
//! It opens no window and installs nothing, and it reads and writes no `~/.horizon`:
//! its only files are the name, the address and the token of its own home
//! ([`super::home`]). It sends the list of the sessions when that list changes, and
//! it sends the entries of a session only while one computer follows that session.
//! The live link of `horizon-sync` opens, reopens and ends it on the delays of a
//! computer, with the sign of life of a live link.
//!
//! Nothing of a session reaches a log line of this process: it installs no tracing
//! writer, and the lines that it prints name the program and counts, never a text.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use horizon_core::{SecretError, SecretId, SecretTarget};

use horizon_core::server_link::JoinLink;
use horizon_core::session_mirror::{
    MirrorCatalog, MirrorEntry, MirrorProgram, SessionKey, process_alive, read_entries, read_list, stores_home,
};
use horizon_sync::{LinkBounds, LiveEvent, LiveHandle, LiveState, LiveSupervisor, MirrorList, SupervisorConfig};

use super::home::MirrorHome;

/// How often the publisher reads the stores.
pub const READ_EVERY: Duration = Duration::from_secs(3);
/// How often the publisher reads the events of its link. A follow arrives within
/// this time, and so do the entries of it.
const TICK: Duration = Duration::from_millis(200);
/// The longest device name that the server accepts, which [`device_name`] cuts at.
const DEVICE_CHARS: usize = 40;

/// What the publisher printed for the state of its link last.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Shown {
    Connecting,
    Open,
    Away(String),
}

/// One session that some computer follows: what the publisher sent last, and the
/// size and time of its file then.
#[derive(Default)]
struct Followed {
    entries: Vec<MirrorEntry>,
    stamp: Option<Stamp>,
}

/// The size and the time of one file, which say that its entries changed.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: u128,
}

impl Stamp {
    fn of(path: &PathBuf) -> Option<Self> {
        let metadata = std::fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis();
        Some(Self {
            len: metadata.len(),
            modified,
        })
    }
}

/// The secrets of a publisher of the session mirror.
///
/// The live link carries no secret: it presents the token of its join, which the
/// secrets store of its home holds. This target therefore holds nothing, and a
/// write that no part of this mode makes is refused.
#[derive(Debug)]
struct NoSecrets;

impl SecretTarget for NoSecrets {
    fn read(&self, _id: &SecretId) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretError> {
        Ok(None)
    }

    fn write(&self, _id: &SecretId, _body: &[u8]) -> Result<(), SecretError> {
        Err(refused())
    }

    fn remove(&self, _id: &SecretId) -> Result<(), SecretError> {
        Ok(())
    }

    fn keep_local(&self, _id: &SecretId, _body: &[u8], _at_millis: u64) -> Result<String, SecretError> {
        Err(refused())
    }

    fn local_ids(&self, _kind: horizon_core::SecretKind) -> Vec<String> {
        Vec::new()
    }
}

/// Why a publisher holds no secret of a server.
fn refused() -> SecretError {
    SecretError::Unsupported {
        reason: "the publisher of the session mirror holds no secret of a server".to_owned(),
    }
}

/// Runs the publisher of `home` until a stop, and answers the exit code of this
/// start: zero for a stop, and two for a server that refused this computer.
pub fn run(home: &MirrorHome) -> i32 {
    let Some(address) = home.address() else {
        return refuse("this computer is not joined to a server: run `--mirror-sessions --join <link>` first.");
    };
    let Some(device) = home.device() else {
        return refuse("this computer is not joined to a server: run `--mirror-sessions --join <link>` first.");
    };
    let secrets = match home.secrets() {
        Ok(secrets) => secrets,
        Err(problem) => return refuse(&problem),
    };
    let mut config = SupervisorConfig::for_login(&address, "", home.root(), Arc::new(NoSecrets))
        .ok()
        .flatten()
        .unwrap_or_else(|| SupervisorConfig {
            target: address.clone(),
            route: None,
            command: String::new(),
            program: String::new(),
            address: None,
            token_store: None,
            device: device.clone(),
            queue_file: home.root().join("server-queue.json"),
            mirror_file: home.root().join("server-mirror.json"),
            machines_dir: home.root().to_path_buf(),
            settings_path: None,
            settings_mirror_file: home.root().join("server-settings.json"),
            secrets: Arc::new(NoSecrets),
            bounds: LinkBounds::STANDARD,
            lookup: horizon_sync::LinkLookup::system(),
        });
    config.address = Some(address.clone());
    config.token_store = Some(secrets);
    config.bounds = LinkBounds::STANDARD;
    let stop = match horizon_sync::terminate_flag() {
        Ok(stop) => stop,
        Err(error) => return refuse(&format!("the signals of this process: {error}")),
    };
    let supervisor = match LiveSupervisor::start(config, &device, || {}) {
        Ok(supervisor) => supervisor,
        Err(error) => return refuse(&format!("the thread of the link: {error}")),
    };
    let handle = supervisor.handle();
    handle.open_mirror();
    println!("horizon mirror: the sessions of this computer go to {address} as {device}.");
    let code = publish(&handle, &stop);
    supervisor.stop();
    code
}

/// The life of one publisher with an open link: the list, the follows and the
/// entries, until a stop or until the server refuses this computer.
fn publish(handle: &LiveHandle, stop: &std::sync::atomic::AtomicBool) -> i32 {
    let mut shown = Shown::Connecting;
    let mut sent: Option<MirrorList> = None;
    let mut followed: BTreeMap<SessionKey, Followed> = BTreeMap::new();
    let mut sources: HashMap<SessionKey, PathBuf>;
    let mut read_at = Instant::now();
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            println!("horizon mirror: stopped.");
            return 0;
        }
        for event in handle.take_events() {
            if let LiveEvent::MirrorWatched {
                program,
                session,
                watched,
            } = event
                && let Some(program) = MirrorProgram::from_key(&program)
            {
                let key = SessionKey { program, session };
                if watched {
                    followed.entry(key).or_default();
                } else {
                    followed.remove(&key);
                }
            }
        }
        let state = handle.state();
        if let Some(code) = show(&state, &mut shown, &mut sent) {
            return code;
        }
        if state == LiveState::Open && read_at.elapsed() >= READ_EVERY {
            read_at = Instant::now();
            let catalog = stores_home().map_or_else(MirrorCatalog::default, |home| {
                read_list(&home, now_millis(), &|pid| process_alive(pid))
            });
            // The whole catalog is read again, so a session that left is forgotten
            // even when nobody followed it.
            sources = catalog
                .rows()
                .iter()
                .map(|row| (row.key(), row.source.clone().unwrap_or_default()))
                .collect();
            let list = mirror_list(&catalog);
            if sent.as_ref() != Some(&list) {
                handle.set_mirror_list(list.clone());
                sent = Some(list);
                println!("horizon mirror: the list holds {}.", counts(&catalog));
            }
            let keys: Vec<SessionKey> = followed.keys().cloned().collect();
            for key in keys {
                let Some(source) = sources.get(&key).filter(|source| !source.as_os_str().is_empty()) else {
                    continue;
                };
                let stamp = Stamp::of(source);
                let followed = followed.entry(key.clone()).or_default();
                if stamp.is_some() && followed.stamp == stamp {
                    continue;
                }
                followed.stamp = stamp;
                let entries = read_entries(key.program, source);
                let (frame, tail) = next_entries(&followed.entries, entries.clone());
                if frame.is_empty() {
                    continue;
                }
                if handle.mirror_entries(key.program.key(), &key.session, frame, tail) {
                    followed.entries = entries;
                }
            }
        }
        std::thread::sleep(TICK);
    }
}

/// The entries that the publisher sends for one newly read tail of a session, and
/// whether the frame holds the whole tail of the follow.
///
/// A tail that extends the entries that this computer sent is the new entries
/// alone. A tail that changed before them is a whole tail again, because an entry
/// that the viewer read first would stand in the wrong place.
fn next_entries(sent: &[MirrorEntry], read: Vec<MirrorEntry>) -> (Vec<MirrorEntry>, bool) {
    if sent.is_empty() || !read.starts_with(sent) {
        return (read, true);
    }
    (read[sent.len()..].to_vec(), false)
}

/// Prints one line for a change of the state of the link, sends the list on a new
/// link, and answers why this start ends when the server refused this computer.
fn show(state: &LiveState, shown: &mut Shown, sent: &mut Option<MirrorList>) -> Option<i32> {
    let next = match state {
        LiveState::Open => Shown::Open,
        LiveState::Connecting => Shown::Connecting,
        LiveState::Unsupported => {
            // The server of an older build reads no frame of the mirror.
            println!("horizon mirror: the server speaks an older protocol than this mirror.");
            return Some(2);
        }
        LiveState::Down(reason) => Shown::Away(reason.clone()),
    };
    if &next == shown {
        return None;
    }
    *shown = next;
    match shown {
        Shown::Open => {
            println!("horizon mirror: connected to the server.");
            // A new link holds nothing of the last one, so the list goes again.
            *sent = None;
        }
        Shown::Connecting => println!("horizon mirror: the link to the server starts."),
        Shown::Away(reason) => {
            println!("horizon mirror: the link to the server ended: {reason}");
            *sent = None;
            if is_refusal(reason) {
                return Some(2);
            }
        }
    }
    None
}

/// Whether the reason of a link that ended says that this computer was refused, and
/// that no attempt follows.
fn is_refusal(reason: &str) -> bool {
    ["removed:", "not_logged_in:"]
        .iter()
        .any(|code| reason.starts_with(code))
}

/// The list of the mirror of one catalog of this computer.
fn mirror_list(catalog: &MirrorCatalog) -> MirrorList {
    MirrorList {
        sessions: catalog.rows().iter().map(|row| row.session.clone()).collect(),
        unreadable: catalog.unreadable().to_vec(),
    }
}

/// The count of each program of the list, for one line of the publisher.
fn counts(catalog: &MirrorCatalog) -> String {
    let mut parts = Vec::new();
    for program in horizon_core::session_mirror::PROGRAMS {
        let count = catalog
            .rows()
            .iter()
            .filter(|row| row.session.program == program)
            .count();
        parts.push(format!("{} {count}", program.key()));
    }
    format!("{} sessions: {}", catalog.rows().len(), parts.join(", "))
}

/// The one line that ends a start with code two.
fn refuse(problem: &str) -> i32 {
    println!("horizon mirror: {problem}");
    2
}

/// The clock of this computer, in milliseconds.
fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
}

/// The address of the server of one join link, `host:port`, which this home keeps
/// so a start without `--join` reaches the same server.
#[must_use]
pub fn address_of(link: &str) -> Option<String> {
    JoinLink::parse(link).ok().map(|link| link.authority())
}

/// The name of this publisher on its server: the name of this computer and
/// `-sessions`, as a device name.
#[must_use]
pub fn device_name(host: &str) -> String {
    const SUFFIX: &str = "-sessions";
    let mut name = String::new();
    for character in host.chars() {
        if character.is_ascii_alphanumeric() {
            name.push(character.to_ascii_lowercase());
        } else if !name.ends_with('-') {
            name.push('-');
        }
    }
    let name = name.trim_matches('-');
    let most = DEVICE_CHARS.saturating_sub(SUFFIX.len()).max(1);
    let name: String = name.chars().take(most).collect();
    let name = name.trim_end_matches('-');
    if name.is_empty() {
        format!("horizon{SUFFIX}")
    } else {
        format!("{name}{SUFFIX}")
    }
}

/// The name of this computer, as its system writes it.
#[must_use]
pub fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|name| !name.is_empty()))
        .unwrap_or_else(|| "this-computer".to_owned())
}

#[cfg(test)]
mod tests {
    use horizon_core::session_mirror::MirrorEntry;

    use super::{address_of, device_name, host_name, is_refusal, next_entries};

    fn person(text: &str) -> MirrorEntry {
        MirrorEntry::Person { text: text.to_owned() }
    }

    #[test]
    fn a_tail_that_grows_sends_the_new_entries_alone() {
        let sent = vec![person("one")];
        let read = vec![person("one"), person("two")];
        assert_eq!(next_entries(&sent, read), (vec![person("two")], false));

        let first = vec![person("one")];
        assert_eq!(
            next_entries(&[], first.clone()),
            (first, true),
            "the follow starts with a whole tail"
        );
        assert_eq!(
            next_entries(&sent, sent.clone()),
            (Vec::new(), false),
            "no change is no frame"
        );
    }

    #[test]
    fn a_tail_that_changed_before_its_end_is_a_whole_tail_again() {
        let sent = vec![person("one")];
        let read = vec![person("another"), person("two")];
        assert_eq!(next_entries(&sent, read.clone()), (read, true));
    }

    #[test]
    fn the_name_of_a_publisher_is_one_device_name_with_the_suffix_of_a_session() {
        assert_eq!(device_name("laptop"), "laptop-sessions");
        assert_eq!(
            device_name("Peters-MacBook-Pro.local"),
            "peters-macbook-pro-local-sessions"
        );
        assert_eq!(device_name("a--b"), "a-b-sessions");
        assert_eq!(device_name(""), "horizon-sessions");
        assert_eq!(device_name("---"), "horizon-sessions");
        assert_eq!(device_name("é"), "horizon-sessions", "a letter outside ASCII is a dash");
        let long = device_name(&"x".repeat(200));
        assert!(horizon_sync::check_device_name(&long), "{long}");
        assert!(long.ends_with("-sessions"), "{long}");
    }

    #[test]
    fn the_address_of_a_join_is_the_authority_of_its_link() {
        assert_eq!(
            address_of("horizon://127.0.0.1:7000/join#ABCD-EFGH-JKMN").as_deref(),
            Some("127.0.0.1:7000")
        );
        assert_eq!(address_of("not a link"), None);
    }

    #[test]
    fn a_refusal_of_the_server_is_no_attempt_again() {
        assert!(is_refusal("removed: this computer left the server"));
        assert!(is_refusal("not_logged_in: the token is gone"));
        assert!(!is_refusal("bad_frame: the frame is not one of this protocol"));
        assert!(!host_name().is_empty());
    }
}

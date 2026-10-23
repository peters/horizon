//! The host file of a profile host: `browser.profiles`, one named persistent
//! profile for each entry, with its engine, its extra browser arguments and,
//! for Firefox, its preferences. The host file holds no credential.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io;
use std::path::Path;

use horizon_browser::BackendKind;
use serde::Deserialize;

const MAX_HOST_FILE_BYTES: u64 = 256 * 1024;
/// At most this many profiles in one host file.
pub const MAX_PROFILES: usize = 32;
const MAX_NAME_CHARS: usize = 40;
const MAX_ARGS: usize = 64;
const MAX_ARG_BYTES: usize = 1024;
const MAX_PREFS: usize = 256;
const MAX_PREF_NAME_BYTES: usize = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostFile {
    browser: BrowserSection,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserSection {
    profiles: BTreeMap<String, ProfileEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileEntry {
    backend: Engine,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    firefox_prefs: BTreeMap<String, Pref>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Engine {
    Chromium,
    Firefox,
}

/// One Firefox preference value, as `user.js` writes it. Firefox keeps an
/// integer preference in 32 bits, so a larger number is refused.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Pref {
    Bool(bool),
    Int(i32),
    Text(String),
}

/// One validated profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub backend: BackendKind,
    pub args: Vec<String>,
    pub firefox_prefs: BTreeMap<String, Pref>,
}

/// A validated host file of a profile host.
#[derive(Debug)]
pub struct ProfileConfig {
    pub profiles: BTreeMap<String, Profile>,
}

impl ProfileConfig {
    /// Read and check the host file at `path`.
    ///
    /// # Errors
    /// The file is missing, too large, writable by other users, not valid
    /// YAML or JSON, or names an invalid profile, argument or preference.
    pub fn load(path: &Path) -> io::Result<Self> {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() || metadata.len() > MAX_HOST_FILE_BYTES {
            return Err(io::Error::other(format!(
                "host file {} must be a regular file of at most {MAX_HOST_FILE_BYTES} bytes",
                path.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // The host file decides the arguments and the proxy of each browser.
            if metadata.permissions().mode() & 0o022 != 0 {
                return Err(io::Error::other(format!(
                    "host file {} must not be writable by group or other users",
                    path.display()
                )));
            }
        }
        Self::parse(&std::fs::read(path)?)
            .map_err(|error| io::Error::other(format!("host file {} is invalid: {error}", path.display())))
    }

    /// Parse and check the text of a host file.
    ///
    /// # Errors
    /// The text is not valid YAML or JSON, or names an invalid profile,
    /// argument or preference.
    pub fn parse(text: &[u8]) -> Result<Self, String> {
        let file: HostFile = serde_yaml::from_slice(text).map_err(|error| error.to_string())?;
        let entries = file.browser.profiles;
        if entries.is_empty() {
            return Err("browser.profiles names no profile".into());
        }
        if entries.len() > MAX_PROFILES {
            return Err(format!("browser.profiles names more than {MAX_PROFILES} profiles"));
        }
        let mut profiles = BTreeMap::new();
        for (name, entry) in entries {
            let profile = check(&name, entry).map_err(|error| format!("profile `{name}`: {error}"))?;
            profiles.insert(name, profile);
        }
        Ok(Self { profiles })
    }

    /// The profile that a create names as its target.
    #[must_use]
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }
}

/// A profile name: 1 to 40 of `a-z`, `0-9` and `-`, starting and ending with
/// a letter or a digit. It is part of each page ID and of the profile folder.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    let edge = |byte: Option<&u8>| byte.is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    (1..=MAX_NAME_CHARS).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && edge(name.as_bytes().first())
        && edge(name.as_bytes().last())
}

fn check(name: &str, entry: ProfileEntry) -> Result<Profile, String> {
    if !valid_name(name) {
        return Err(format!(
            "a profile name is 1 to {MAX_NAME_CHARS} of a-z, 0-9 and '-', and starts and ends with a letter or a digit"
        ));
    }
    let backend = match entry.backend {
        Engine::Chromium => BackendKind::ChromiumCdp,
        Engine::Firefox => BackendKind::FirefoxBidi,
    };
    if entry.args.len() > MAX_ARGS {
        return Err(format!("at most {MAX_ARGS} args"));
    }
    for argument in &entry.args {
        if argument.is_empty() || argument.len() > MAX_ARG_BYTES || argument.chars().any(char::is_control) {
            return Err(format!(
                "each arg is 1 to {MAX_ARG_BYTES} bytes with no control character"
            ));
        }
        if managed(backend, argument) {
            return Err(format!("arg {argument:?} overrides an argument that the host manages"));
        }
    }
    if backend != BackendKind::FirefoxBidi && !entry.firefox_prefs.is_empty() {
        return Err("firefox_prefs apply only to a firefox profile".into());
    }
    if entry.firefox_prefs.len() > MAX_PREFS {
        return Err(format!("at most {MAX_PREFS} firefox_prefs"));
    }
    for (pref, value) in &entry.firefox_prefs {
        let named = !pref.is_empty()
            && pref.len() <= MAX_PREF_NAME_BYTES
            && pref
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if !named {
            return Err(format!(
                "a preference name is 1 to {MAX_PREF_NAME_BYTES} of letters, digits, '.', '_' and '-'"
            ));
        }
        if let Pref::Text(text) = value
            && (text.len() > MAX_ARG_BYTES || text.chars().any(char::is_control))
        {
            return Err(format!(
                "preference `{pref}` is at most {MAX_ARG_BYTES} bytes with no control character"
            ));
        }
    }
    Ok(Profile {
        backend,
        args: entry.args,
        firefox_prefs: entry.firefox_prefs,
    })
}

/// The arguments that the engine sets itself: the profile folder, the
/// automation endpoint and the headless mode. The engine refuses the first
/// two again at launch; this check names the problem when the host starts.
fn managed(backend: BackendKind, argument: &str) -> bool {
    let switch = argument.trim_start_matches(['-', '/']);
    let name = switch.split(['=', ':']).next().unwrap_or(switch).to_ascii_lowercase();
    if name.starts_with("remote-debugging-") || name == "headless" {
        return true;
    }
    match backend {
        BackendKind::ChromiumCdp => matches!(name.as_str(), "remote-allow-origins" | "user-data-dir"),
        _ => matches!(name.as_str(), "profile" | "p" | "marionette" | "no-remote"),
    }
}

const USER_JS_HEADER: &str = "// Written by `horizon-browser host` from its host file. A change here is replaced.\n";

/// The `user.js` of a Firefox profile. Firefox reads it at each start and
/// sets each preference in it, so a change of the host file applies at the
/// next start of the profile. [`forget_removed_prefs`] undoes a removal.
#[must_use]
pub fn user_js(prefs: &BTreeMap<String, Pref>) -> String {
    let mut text = String::from(USER_JS_HEADER);
    for (name, value) in prefs {
        let value = match value {
            Pref::Bool(value) => value.to_string(),
            Pref::Int(value) => value.to_string(),
            Pref::Text(value) => quote(value),
        };
        let _ = writeln!(text, "user_pref({}, {value});", quote(name));
    }
    text
}

/// Firefox keeps each preference of `user.js` in `prefs.js` of the profile
/// folder `dir`, also after the host file no longer names it. Before the
/// host writes a new `user.js`, remove from `prefs.js` each preference that
/// the earlier `user.js` of the host set and `prefs` no longer names, so
/// that it gets its default again. Other lines of `prefs.js` stay.
///
/// # Errors
/// `prefs.js` cannot be read or written.
pub fn forget_removed_prefs(dir: &Path, prefs: &BTreeMap<String, Pref>) -> io::Result<()> {
    let Ok(earlier) = std::fs::read_to_string(dir.join("user.js")) else {
        return Ok(());
    };
    if !earlier.starts_with(USER_JS_HEADER) {
        return Ok(());
    }
    let removed: BTreeSet<&str> = earlier
        .lines()
        .filter_map(pref_name)
        .filter(|name| !prefs.contains_key(*name))
        .collect();
    let path = dir.join("prefs.js");
    if removed.is_empty() || !path.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&path)?;
    let kept: String = text
        .split_inclusive('\n')
        .filter(|line| pref_name(line).is_none_or(|name| !removed.contains(name)))
        .collect();
    super::host::write_private(&path, kept.as_bytes())
}

/// The name of the preference that a `user_pref("name", value);` line sets.
fn pref_name(line: &str) -> Option<&str> {
    line.trim_start()
        .strip_prefix("user_pref(\"")?
        .split_once('"')
        .map(|(name, _)| name)
}

/// A string literal of the preference file: JSON escapes are valid there.
fn quote(value: &str) -> String {
    serde_json::Value::String(value.to_owned()).to_string()
}

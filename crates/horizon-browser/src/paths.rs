use std::path::PathBuf;

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

/// The variable that marks a test process, as `horizon-browser-control` names it:
/// a test may not resolve the home of the person.
const FORBID_REAL_HOME_ENV: &str = "HORIZON_FORBID_REAL_HOME";

/// The variable that names the Horizon home of a run, as `horizon-core` names it.
const HORIZON_HOME_ENV: &str = "HORIZON_HOME";

/// The folder that holds the profile of every browser panel of a run.
///
/// A run that names a Horizon home stores its browser here as well, because a
/// second Horizon with its own home must not write into the folder tree of the
/// first one: a person who tries a new build beside the one they use, and a test,
/// then keep their own profiles. A run that names no home keeps the folder that it
/// always had, so the signed-in sites of the person stay where they are.
#[must_use]
pub(crate) fn profile_root() -> PathBuf {
    profile_root_from(std::env::var_os(HORIZON_HOME_ENV), std::env::var_os("HOME"))
}

/// [`profile_root`] with the two homes given as values, so a test reads the answer
/// of both cases on any system and with any environment.
fn profile_root_from(horizon_home: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(private) = horizon_home.filter(|value| !value.is_empty()) {
        return PathBuf::from(private).join("browser-profiles");
    }
    let home = home.filter(|value| !value.is_empty()).map(PathBuf::from);
    if let Some(home) = home.as_deref() {
        refuse_real_home(home);
    }
    home.map_or_else(
        || PathBuf::from(".horizon-browser").join("profiles"),
        |home| home.join(".horizon-browser").join("profiles"),
    )
}

/// Stops a test process that resolved the home of the person. This crate has no
/// dependency on `horizon-browser-control`, so the guard is written out here; the
/// runner of `.cargo/config.toml` sets the variable for every test binary.
///
/// # Panics
///
/// Panics when `home` is the home of the person that runs the suite.
#[track_caller]
fn refuse_real_home(home: &std::path::Path) {
    let Some(forbidden) = std::env::var_os(FORBID_REAL_HOME_ENV) else {
        return;
    };
    let forbidden = std::path::PathBuf::from(forbidden);
    if !forbidden.as_os_str().is_empty() && forbidden != home {
        return;
    }
    let caller = std::panic::Location::caller();
    panic!(
        "a test process resolved the home of the person ({}), from {caller}; give the test a private HOME or \
         HORIZON_HOME",
        home.display()
    );
}

#[must_use]
pub(crate) fn expand_tilde(input: &str) -> PathBuf {
    if input == "~" {
        home_dir()
    } else if let Some(rest) = input.strip_prefix("~/") {
        home_dir().join(rest)
    } else {
        PathBuf::from(input)
    }
}

pub(crate) fn home_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(home) = home.as_deref() {
        refuse_real_home(home);
    }
    home.unwrap_or_else(|| PathBuf::from("."))
}

/// A folder name a person can read: the identifier keeps the characters a
/// person typed and encodes every other byte, so it fits one path component and
/// never becomes `.` or `..`.
#[must_use]
pub(crate) fn readable_id(local_id: &str) -> String {
    let mut encoded = String::with_capacity(local_id.len().max(1));
    for byte in local_id.bytes() {
        let character = char::from(byte);
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            encoded.push(character);
        } else {
            encoded.push('%');
            encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
            encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
        }
    }
    match encoded.as_str() {
        "" | "." | ".." => format!("%{}{}", LOWER_HEX[0], LOWER_HEX[0]),
        _ => encoded,
    }
}

#[must_use]
pub(crate) fn safe_local_id(local_id: &str) -> String {
    let mut encoded = String::with_capacity(1 + local_id.len() * 2);
    encoded.push('%');
    for byte in local_id.bytes() {
        encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_readable_id_keeps_typed_characters_and_encodes_the_rest() {
        assert_eq!(
            readable_id("3f4c1e2a-0000-4000-8000-000000000000"),
            "3f4c1e2a-0000-4000-8000-000000000000"
        );
        assert_eq!(readable_id("panel_1"), "panel_1");
        assert_eq!(readable_id("a/b"), "a%2fb");
        assert_eq!(readable_id("a b"), "a%20b");
        assert_eq!(readable_id("æ"), "%c3%a6");
    }

    /// A run with a Horizon home of its own keeps every browser profile under it,
    /// and a run without one keeps the folder of the person, so no signed-in site
    /// moves.
    #[test]
    fn a_browser_profile_is_under_the_horizon_home_when_one_is_named() {
        use std::ffi::OsString;

        let root = |horizon_home: Option<&str>, home: Option<&str>| {
            profile_root_from(horizon_home.map(OsString::from), home.map(OsString::from))
        };
        let person = PathBuf::from("/home/someone");
        assert_eq!(
            root(None, Some("/home/someone")),
            person.join(".horizon-browser").join("profiles"),
            "a run without a Horizon home keeps the folder that it always had"
        );
        assert_eq!(
            root(Some("/home/someone/horizon-dev"), Some("/home/someone")),
            PathBuf::from("/home/someone/horizon-dev").join("browser-profiles"),
            "a private Horizon home holds the browser of that run"
        );
        assert_eq!(
            root(Some(""), Some("/home/someone")),
            person.join(".horizon-browser").join("profiles"),
            "an empty override is unset, as an interpolated `${{HORIZON_HOME:-}}` is"
        );
        assert_eq!(
            root(Some("/home/someone/horizon-dev"), None),
            PathBuf::from("/home/someone/horizon-dev").join("browser-profiles"),
            "the home of the person is not read at all with an override"
        );
    }

    #[test]
    fn a_readable_id_is_never_a_dot_or_empty_component() {
        for name in ["", ".", ".."] {
            let encoded = readable_id(name);
            assert!(!encoded.is_empty(), "{name:?}");
            assert_ne!(encoded, ".", "{name:?}");
            assert_ne!(encoded, "..", "{name:?}");
        }
    }
}

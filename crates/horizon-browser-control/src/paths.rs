//! Browser coordination paths with Horizon-compatible defaults.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const RUNTIME_ROOT_ENV: &str = "HORIZON_BROWSER_ROOT";
/// Names the application home in place of `.horizon` in the user's home directory.
pub const HOME_ENV: &str = "HORIZON_HOME";
/// The home of the person that runs a test suite: a test process may not resolve
/// it. The runner of `.cargo/config.toml` sets it for every test binary of
/// `cargo test` and `cargo nextest`, and the application of `cargo run` never
/// gets it.
pub const FORBID_REAL_HOME_ENV: &str = "HORIZON_FORBID_REAL_HOME";
static RUNTIME_PATHS: OnceLock<Option<BrowserRuntimePaths>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum RuntimePathError {
    #[error("browser runtime root must not be empty")]
    EmptyRoot,
    #[error("could not resolve browser runtime root: {0}")]
    Resolve(#[from] std::io::Error),
    #[error("browser runtime paths were already used with defaults or configured with a different root")]
    AlreadyInUse,
    #[error("HORIZON_HOME must be an absolute path, not {}", .0.display())]
    RelativeHome(PathBuf),
}

/// The application home is independent of an explicitly configured browser root: the path in
/// `HORIZON_HOME` when it is set and not empty, else `.horizon` in the user's home directory,
/// from `HOME`. Only without either is it `.horizon` in the working directory. While
/// `HORIZON_HOME` is set this never falls back to the user's home directory; entry points reject
/// a relative value with [`home_override`] before they use it.
#[must_use]
pub fn default_horizon_root() -> PathBuf {
    let horizon_home = std::env::var_os(HOME_ENV);
    let home = std::env::var_os("HOME");
    if non_empty_path(horizon_home.clone()).is_none()
        && let Some(home) = home.as_ref().map(PathBuf::from)
    {
        refuse_real_home(&home);
    }
    horizon_root_from(horizon_home, home)
}

/// Stops a test process that resolved the home of the person.
///
/// A test gives its own home, so every path that it resolves differs from the
/// value of [`FORBID_REAL_HOME_ENV`]. The runner of `.cargo/config.toml` sets that
/// value for every test binary, so the panic names the line that reached the home
/// of the person and the test cannot write or read it by mistake.
///
/// # Panics
///
/// Panics when `home` is the home of the person that runs the suite, or when the
/// variable is set to an empty value.
#[track_caller]
pub fn refuse_real_home(home: &Path) {
    let Some(forbidden) = std::env::var_os(FORBID_REAL_HOME_ENV) else {
        return;
    };
    let forbidden = PathBuf::from(forbidden);
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

fn horizon_root_from(horizon_home: Option<std::ffi::OsString>, home: Option<std::ffi::OsString>) -> PathBuf {
    if let Some(root) = non_empty_path(horizon_home) {
        return root;
    }
    home.into_iter().find(|value| !value.is_empty()).map_or_else(
        || PathBuf::from(".horizon"),
        |home| PathBuf::from(home).join(".horizon"),
    )
}

/// Freeze an absolute coordination root before using any default-path helpers.
/// Repeating the same root is allowed; changing it after use is rejected.
/// # Errors
/// Rejects empty paths, unavailable current directories and late reconfiguration.
pub fn configure_runtime_root(root: impl AsRef<Path>) -> Result<(), RuntimePathError> {
    let root = root.as_ref();
    if root.as_os_str().is_empty() {
        return Err(RuntimePathError::EmptyRoot);
    }
    let paths = BrowserRuntimePaths::from_root(std::path::absolute(root)?);
    match RUNTIME_PATHS.set(Some(paths.clone())) {
        Ok(()) => Ok(()),
        Err(_) if RUNTIME_PATHS.get() == Some(&Some(paths)) => Ok(()),
        Err(_) => Err(RuntimePathError::AlreadyInUse),
    }
}

/// The path in `HORIZON_HOME` as given, or `None` when it is unset or empty.
#[must_use]
pub fn configured_home() -> Option<PathBuf> {
    non_empty_path(std::env::var_os(HOME_ENV))
}

/// The application home that `HORIZON_HOME` names, or `None` when it is unset or empty.
/// # Errors
/// Rejects a value that is not an absolute path.
pub fn home_override() -> Result<Option<PathBuf>, RuntimePathError> {
    home_override_from(std::env::var_os(HOME_ENV))
}

fn home_override_from(value: Option<std::ffi::OsString>) -> Result<Option<PathBuf>, RuntimePathError> {
    match non_empty_path(value) {
        Some(root) if !root.is_absolute() => Err(RuntimePathError::RelativeHome(root)),
        root => Ok(root),
    }
}

fn non_empty_path(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// Initialize standalone clients before discovery, pruning or serving requests.
/// # Errors
/// Returns configuration errors without falling back to another root.
pub fn initialize_from_environment() -> Result<(), RuntimePathError> {
    home_override()?;
    if let Some(root) = std::env::var_os(RUNTIME_ROOT_ENV) {
        configure_runtime_root(PathBuf::from(root))
    } else {
        Ok(())
    }
}

const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserRuntimePaths {
    root: PathBuf,
}

impl BrowserRuntimePaths {
    #[must_use]
    pub fn resolve() -> Self {
        RUNTIME_PATHS
            .get_or_init(|| None)
            .clone()
            .unwrap_or_else(|| Self::from_root(default_horizon_root()))
    }

    #[must_use]
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn browsers_manifest_dir(&self) -> PathBuf {
        self.root.join("runtime").join("browsers")
    }

    #[must_use]
    pub fn browser_results_dir(&self) -> PathBuf {
        self.root.join("runtime").join("browser-results")
    }

    /// Private copies of files an agent attaches through `set_files`, one
    /// directory per panel and action. They stay while the panel's page may
    /// still read them lazily and are pruned by age, count and size when the
    /// panel's next attachment is staged. The browser must be able to read
    /// them: a Snap-confined browser of Linux cannot open hidden directories
    /// directly beneath the home directory, and the open dialog of Linux and of
    /// macOS hides a dot folder. A runtime root such as `~/.horizon` therefore
    /// stages under the visible `~/Horizon` directory.
    /// That visible directory is namespaced by the runtime root, so two
    /// hidden roots under one home (say `~/.horizon` and `~/.horizon-dev`)
    /// never share staging or sweep each other's. The path is absolute even
    /// when the runtime root is the relative fallback, because queued
    /// attachment paths must be absolute.
    #[must_use]
    pub fn browser_attachments_dir(&self) -> PathBuf {
        let root = std::path::absolute(&self.root).unwrap_or_else(|_| self.root.clone());
        let home = std::env::var_os("HOME").map(PathBuf::from);
        if let Some(home) = home.as_deref() {
            refuse_real_home(home);
        }
        browser_visible_attachments_dir(&root, home.as_deref())
            .unwrap_or_else(|| root.join("runtime").join("browser-attachments"))
    }

    #[must_use]
    pub fn browser_audit_dir(&self) -> PathBuf {
        self.root.join("audit").join("browsers")
    }
}

/// The visible folder of the attachments of a runtime root that is hidden
/// beneath the home of the person, or `None` when the root is visible itself.
fn browser_visible_attachments_dir(root: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let home = home?;
    let hidden_beneath_home = root
        .strip_prefix(home)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|component| component.as_os_str().to_string_lossy().starts_with('.'));
    hidden_beneath_home.then(|| {
        home.join("Horizon")
            .join("browser-attachments")
            .join(encode_path_bytes(root))
    })
}

/// A bounded directory name derived from a path's exact platform bytes (a
/// SHA-256 digest in hex, 64 characters), so two distinct roots never share
/// it however they are spelled and however long they are.
fn encode_path_bytes(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    use std::os::unix::ffi::OsStrExt;
    let digest = Sha256::digest(path.as_os_str().as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(LOWER_HEX[(byte >> 4) as usize] as char);
        encoded.push(LOWER_HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[must_use]
pub fn safe_local_id(local_id: &str) -> String {
    // Always encode the exact UTF-8 bytes. Keeping an apparently safe ID
    // verbatim would make identifiers that differ only by case collide on
    // default macOS filesystems. The lowercase hexadecimal
    // alphabet itself has no case variants, and the '%' prefix keeps the
    // empty identifier distinct.
    let mut encoded = String::with_capacity(1 + local_id.len() * 2);
    encoded.push('%');
    for byte in local_id.bytes() {
        encoded.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod home_tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn the_home_comes_from_home() {
        let root = |home: Option<&str>| super::horizon_root_from(None, home.map(OsString::from));
        let user = PathBuf::from("user");
        assert_eq!(root(Some("user")), user.join(".horizon"));
        assert_eq!(root(Some("")), PathBuf::from(".horizon"));
        assert_eq!(root(None), PathBuf::from(".horizon"));
    }

    #[test]
    fn a_horizon_home_override_replaces_the_user_home() {
        let root = |horizon_home: Option<&str>, home: Option<&str>| {
            super::horizon_root_from(horizon_home.map(OsString::from), home.map(OsString::from))
        };
        let private = std::env::temp_dir().join("private-horizon");
        let private = private.to_str().expect("UTF-8 temporary path");
        assert_eq!(root(Some(private), Some("user")), PathBuf::from(private));
        assert_eq!(root(Some(private), None), PathBuf::from(private));
        assert_eq!(
            root(Some(""), Some("user")),
            PathBuf::from("user").join(".horizon"),
            "an empty override is unset, as an interpolated `${{HORIZON_HOME:-}}` is"
        );
        assert_eq!(
            root(Some("relative"), Some("user")),
            PathBuf::from("relative"),
            "a set override never falls back to the user home"
        );
    }

    /// The guard of the suite: a private home passes, and the home of the person
    /// that runs the suite ends the test process here.
    #[test]
    fn the_guard_refuses_the_home_of_the_person_and_accepts_a_private_one() {
        let Some(person) = std::env::var_os(super::FORBID_REAL_HOME_ENV) else {
            // A direct start of this test binary, without the runner of the
            // workspace: the guard is not active.
            return;
        };
        let private = tempfile::tempdir().expect("a private home");
        super::refuse_real_home(private.path());
        assert!(
            std::panic::catch_unwind(|| super::refuse_real_home(std::path::Path::new(&person))).is_err(),
            "a test process that reached {} ends here",
            person.display()
        );
    }

    #[test]
    fn only_an_absolute_horizon_home_is_accepted() {
        let absolute = std::env::temp_dir().join("private-horizon");
        assert_eq!(super::home_override_from(None).expect("unset"), None);
        assert_eq!(super::home_override_from(Some(OsString::new())).expect("empty"), None);
        assert_eq!(
            super::home_override_from(Some(absolute.clone().into_os_string())).expect("absolute"),
            Some(absolute)
        );
        for relative in ["relative", "./relative", "../outside"] {
            assert!(matches!(
                super::home_override_from(Some(OsString::from(relative))),
                Err(super::RuntimePathError::RelativeHome(path)) if path == std::path::Path::new(relative)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hidden_runtime_roots_under_one_home_stage_in_separate_visible_directories() {
        let home = std::path::Path::new("/home/someone");
        let first = super::browser_visible_attachments_dir(&home.join(".horizon"), Some(home)).expect("hidden");
        let second = super::browser_visible_attachments_dir(&home.join(".horizon-dev"), Some(home)).expect("hidden");
        assert!(first.starts_with(home.join("Horizon").join("browser-attachments")));
        assert_ne!(first, second, "two hidden roots never share staging");
        assert_eq!(
            first.file_name().map(std::ffi::OsStr::len),
            Some(64),
            "the namespace is a bounded digest, not the path itself"
        );
        let long = home.join(format!(".{}", "h".repeat(200)));
        let bounded = super::browser_visible_attachments_dir(&long, Some(home)).expect("hidden");
        assert_eq!(bounded.file_name().map(std::ffi::OsStr::len), Some(64));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let odd = |bytes: &[u8]| home.join(std::ffi::OsStr::from_bytes(bytes));
            let left = super::browser_visible_attachments_dir(&odd(b".horizon-\x80"), Some(home)).expect("hidden");
            let right = super::browser_visible_attachments_dir(&odd(b".horizon-\x81"), Some(home)).expect("hidden");
            assert_ne!(left, right, "non-UTF-8 roots are told apart by their exact bytes");
        }
        assert!(super::browser_visible_attachments_dir(&home.join("Horizon"), Some(home)).is_none());
        assert!(super::browser_visible_attachments_dir(&home.join(".horizon"), None).is_none());
    }
}

//! The throwaway profile folders of private browser panels.
//!
//! A private panel of this computer runs a browser of its own in a folder of its
//! own, so that it is signed in nowhere and leaves nothing behind. The close of
//! the panel removes that folder. A run that ends while the panel is open and
//! keeps no session of it leaves the folder on the disk, because no close ever
//! came.
//!
//! Such a folder is removed at the next start, by one narrow rule: the folder
//! sits under the folder of the private pages of a profile root, it carries the
//! mark that this program writes when it makes a folder for a private panel, no
//! saved session of this Horizon home names the panel, and no browser holds the
//! folder. A folder of a shared profile is a direct child of a profile root, so
//! no sweep can reach one; a folder that an earlier version made for a panel sits
//! at the root as well and carries no mark. Neither is ever removed here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The name of the file that marks the folder of a private browser panel.
pub const PRIVATE_PROFILE_MARK: &str = ".horizon-private-panel";

/// The words of that file. They say what a person reads when the folder stays.
const MARK_TEXT: &str = "horizon private browser panel: the folder of a panel that keeps no sign-in\n";

/// What one sweep of the folders of private panels did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PrivateProfileSweep {
    /// The folders that the sweep removed.
    pub removed: Vec<PathBuf>,
    /// Folders that a saved session names, so the panel comes back to them.
    pub kept_named: usize,
    /// Folders of a shared profile, which hold the sign-ins of the person. A
    /// sweep counts them and never looks inside one.
    pub kept_shared: usize,
    /// Folders of an earlier version or of another program: no mark of this one.
    pub kept_unmarked: usize,
    /// Folders that a browser of this computer holds now.
    pub kept_held: usize,
}

/// Mark `folder` as the folder of a private panel of this program.
///
/// # Errors
/// The folder cannot be made or the mark cannot be written.
pub fn mark_private_profile(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    std::fs::write(folder.join(PRIVATE_PROFILE_MARK), MARK_TEXT)
}

/// Whether this program marked `folder` as the folder of a private panel.
#[must_use]
pub fn is_private_profile(folder: &Path) -> bool {
    folder.join(PRIVATE_PROFILE_MARK).is_file()
}

/// Remove the marked folders of private panels that no saved session names and
/// that no browser holds, under each of `roots`.
///
/// `named` holds the folder name of every browser panel that a saved session of
/// this Horizon home names, in the form that a profile folder uses.
#[must_use]
pub fn sweep_private_profiles(roots: &[PathBuf], named: &HashSet<String>) -> PrivateProfileSweep {
    let mut sweep = PrivateProfileSweep::default();
    let mut seen = HashSet::new();
    for root in roots {
        // Every folder at the root is the folder of a shared profile or of an
        // earlier version of this program: this sweep counts them and leaves
        // them alone, whatever they hold.
        if let Ok(entries) = std::fs::read_dir(root) {
            sweep.kept_shared += entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .filter(|entry| entry.file_name() != horizon_browser::PRIVATE_PROFILES_FOLDER)
                .count();
        }
        let private_root = root.join(horizon_browser::PRIVATE_PROFILES_FOLDER);
        let Ok(entries) = std::fs::read_dir(&private_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || !seen.insert(path.clone()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_private_profile(&path) {
                sweep.kept_unmarked += 1;
                continue;
            }
            if named.contains(&name) {
                sweep.kept_named += 1;
                continue;
            }
            if held_by_a_browser(&path) {
                sweep.kept_held += 1;
                continue;
            }
            match std::fs::remove_dir_all(&path) {
                Ok(()) => sweep.removed.push(path),
                Err(error) => {
                    tracing::warn!(
                        folder = %path.display(),
                        %error,
                        "the folder of a private browser panel could not be removed"
                    );
                    sweep.kept_held += 1;
                }
            }
        }
    }
    sweep
}

/// Whether a browser of this computer holds `folder` now.
///
/// Chromium and Firefox write a lock into the profile folder that names the
/// process that holds it, so a folder with a live lock belongs to a browser of a
/// run that is open, and no sweep removes it. On another system than Linux the
/// lock is not read here, and the mark and the saved sessions decide alone.
#[cfg(target_os = "linux")]
fn held_by_a_browser(folder: &Path) -> bool {
    for engine in ["chromium", "firefox"] {
        let engine_folder = folder.join(engine);
        for marker in ["SingletonLock", "lock"] {
            let Some(target) = std::fs::read_link(engine_folder.join(marker)).ok() else {
                continue;
            };
            let pid = target
                .to_string_lossy()
                .rsplit(['-', '+'])
                .next()
                .and_then(|pid| pid.parse::<u32>().ok());
            if pid.is_some_and(|pid| Path::new("/proc").join(pid.to_string()).exists()) {
                return true;
            }
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
const fn held_by_a_browser(_folder: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder for a private panel, as this program makes one: under the folder
    /// of the private pages of `root`.
    fn private_folder(root: &Path, name: &str) -> PathBuf {
        let folder = root.join(horizon_browser::PRIVATE_PROFILES_FOLDER).join(name);
        mark_private_profile(&folder).expect("the mark of a private panel");
        std::fs::create_dir_all(folder.join("firefox")).expect("the folder of the engine");
        std::fs::write(folder.join("firefox").join("cookies.sqlite"), "a sign-in").expect("fixture");
        folder
    }

    /// The folder of a private panel that no session names, that a browser does
    /// not hold, and that this program marked goes away with what it holds.
    #[test]
    fn a_private_folder_with_no_owner_is_removed() {
        let root = tempfile::tempdir().expect("a folder of a test");
        let folder = private_folder(root.path(), "%panel-1");
        let sweep = sweep_private_profiles(&[root.path().to_path_buf()], &HashSet::new());
        assert_eq!(sweep.removed, vec![folder.clone()]);
        assert!(!folder.exists(), "the folder of the panel and its cookies are gone");
    }

    /// A saved session names the panel, so the folder of that panel stays for the
    /// panel that comes back.
    #[test]
    fn a_private_folder_that_a_saved_session_names_is_kept() {
        let root = tempfile::tempdir().expect("a folder of a test");
        let folder = private_folder(root.path(), "%panel-1");
        let named = HashSet::from(["%panel-1".to_string()]);
        let sweep = sweep_private_profiles(&[root.path().to_path_buf()], &named);
        assert!(sweep.removed.is_empty(), "no folder of a saved panel is removed");
        assert_eq!(sweep.kept_named, 1);
        assert!(folder.join("firefox").join("cookies.sqlite").exists());
    }

    /// The folder of a shared profile holds the sign-ins of a person and of the
    /// agents of that profile, so no sweep of this program removes one.
    #[test]
    fn a_shared_profile_folder_is_never_removed() {
        let root = tempfile::tempdir().expect("a folder of a test");
        let folder = root.path().join("%64656661756c74");
        std::fs::create_dir_all(folder.join("chromium")).expect("the folder of the engine");
        std::fs::write(folder.join("chromium").join("Cookies"), "a sign-in").expect("fixture");
        // A folder at the root of the profiles is out of reach of this sweep,
        // even when a mark of a private panel sits in it.
        mark_private_profile(&folder).expect("a mark");
        let sweep = sweep_private_profiles(&[root.path().to_path_buf()], &HashSet::new());
        assert!(sweep.removed.is_empty());
        assert_eq!(sweep.kept_shared, 1);
        assert!(folder.exists(), "a shared profile stays");
    }

    /// A folder that an earlier version made for a panel sits at the root and
    /// carries no mark, so the sweep of this version leaves it alone.
    #[test]
    fn a_panel_folder_without_the_mark_of_this_version_is_kept() {
        let root = tempfile::tempdir().expect("a folder of a test");
        let old = root.path().join("%panel-of-an-earlier-version");
        std::fs::create_dir_all(old.join("firefox")).expect("the folder of the engine");
        std::fs::write(old.join("firefox").join("cookies.sqlite"), "a sign-in").expect("fixture");
        let unmarked = root
            .path()
            .join(horizon_browser::PRIVATE_PROFILES_FOLDER)
            .join("%panel-without-a-mark");
        std::fs::create_dir_all(&unmarked).expect("a folder without a mark");
        let sweep = sweep_private_profiles(&[root.path().to_path_buf()], &HashSet::new());
        assert!(sweep.removed.is_empty(), "a folder without the mark is not ours");
        assert_eq!(sweep.kept_unmarked, 1);
        assert!(old.exists() && unmarked.exists());
    }

    /// Two roots, as a home and a configured profile root, are swept once each.
    #[test]
    fn the_sweep_reads_each_root_and_counts_no_folder_twice() {
        let home = tempfile::tempdir().expect("a folder of a test");
        let configured = tempfile::tempdir().expect("a folder of a test");
        let first = private_folder(home.path(), "%panel-1");
        let second = private_folder(configured.path(), "%panel-2");
        let sweep = sweep_private_profiles(
            &[
                home.path().to_path_buf(),
                home.path().to_path_buf(),
                configured.path().to_path_buf(),
            ],
            &HashSet::new(),
        );
        assert_eq!(sweep.removed.len(), 2, "one removal for each folder: {sweep:?}");
        assert!(!first.exists() && !second.exists());
    }
}

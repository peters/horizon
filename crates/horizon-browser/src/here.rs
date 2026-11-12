//! The folder and the programs of a `browser` section that this computer has.
//!
//! The section `browser` follows the server, so it can name what another computer
//! holds: its download folder, its profile folder, its Chromium or Firefox, its
//! `geckodriver` and its `safaridriver`. A value that names a path which this
//! computer does not have is read as if it were unset, so a panel uses what this
//! computer has instead of failing with the path of another home.
//!
//! One line of the log names the setting and never the path. The line is at INFO:
//! the settings of this computer look correct, so a value that does not apply here
//! is the one fact that explains a panel with its own default folder or program.

use std::path::{Path, PathBuf};

use crate::paths;
use crate::process::resolve_binary;

/// The program of `setting` when this computer can run it.
///
/// A command is absolute or a name of `PATH`, as [`resolve_binary`] reads it, so a
/// program of another computer is one that this computer does not find. `None`
/// answers that the caller uses what it uses with no value at all: the candidate
/// list of this computer or the browser that the driver finds by itself.
#[must_use]
pub(crate) fn program(setting: &'static str, command: Option<&str>) -> Option<String> {
    let command = command.map(str::trim).filter(|command| !command.is_empty())?;
    if resolve_binary(command).is_ok() {
        return Some(command.to_owned());
    }
    tracing::info!(
        setting,
        "a program of the settings of the server is not on this computer, so this computer uses its own"
    );
    None
}

/// The folder of `setting` when this computer has it.
///
/// The value keeps the `~` form of a person, and a folder that this computer does
/// not have answers `None`, so the caller uses the folder that it uses with no
/// value. A folder that the person sets on this computer and does not make yet
/// reads as one of another computer: the caller logs this one line and uses its
/// own folder.
#[must_use]
pub(crate) fn folder(setting: &'static str, folder: Option<&Path>) -> Option<PathBuf> {
    let folder = paths::expand_tilde(&folder?.to_string_lossy());
    if folder.is_dir() {
        return Some(folder);
    }
    tracing::info!(
        setting,
        "a folder of the settings of the server is not on this computer, so this computer uses its own"
    );
    None
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{folder, program};

    #[test]
    fn a_program_that_this_computer_runs_is_the_program_of_the_setting() {
        // The running test binary always exists, so it stands for a program that a
        // computer has.
        let here = std::env::current_exe().expect("the test binary").display().to_string();
        assert_eq!(program("browser.command", Some(&here)), Some(here.clone()));
        assert_eq!(
            program("browser.command", Some("sh")),
            Some("sh".to_owned()),
            "a name of PATH is a program of this computer"
        );
    }

    #[test]
    fn a_program_of_another_computer_is_read_as_unset() {
        let missing = "/nonexistent/a-browser-of-another-computer/chromium";
        assert_eq!(program("browser.command", Some(missing)), None);
        assert_eq!(
            program("browser.geckodriver_command", Some("not-a-driver-of-this-computer")),
            None
        );
        assert_eq!(program("browser.command", Some("   ")), None);
        assert_eq!(program("browser.command", None), None);
    }

    #[test]
    fn a_folder_that_this_computer_has_is_the_folder_of_the_setting() {
        let here = std::env::temp_dir();
        assert_eq!(folder("browser.downloads_root", Some(&here)), Some(here.clone()));
    }

    #[test]
    fn a_folder_of_another_computer_is_read_as_unset() {
        let missing = PathBuf::from("/nonexistent/a-state-folder-of-another-computer");
        assert_eq!(folder("browser.downloads_root", Some(&missing)), None);
        assert_eq!(folder("browser.profile_root", Some(&missing)), None);
        // A file is no folder, and a value that is no path at all is no value.
        let file = std::env::current_exe().expect("the test binary");
        assert_eq!(folder("browser.profile_root", Some(&file)), None);
        assert_eq!(folder("browser.profile_root", None), None);
    }
}

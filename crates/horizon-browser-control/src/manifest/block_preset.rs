//! The panel of a preset that an agent asks Horizon to open for a person.
//!
//! `block_panel open_panel` names one preset of the person's settings and one
//! directory. Horizon takes the command, the arguments and the kind from the
//! preset, so an agent contributes no command, no argument, no prompt and no
//! environment: it chooses a preset that the person already saved and a folder
//! that its own task uses. The preset is resolved against the settings of this
//! Horizon and the folder is checked on the coordination worker, because that
//! reads the file system. This module holds the rules and the refusal codes that
//! the MCP server and the UI both use, as [`super::block_keys`] does for keys.

use std::path::{Path, PathBuf};

/// The longest directory accepted, in bytes of the path.
pub const MAX_DIRECTORY_BYTES: usize = 1024;

/// Most characters a preset name or alias holds.
pub const MAX_PRESET_CHARS: usize = 80;

/// Most characters the one prompt line of an `open_panel` holds.
pub const MAX_PROMPT_CHARS: usize = 512;

/// Most panels one agent panel opens from a preset inside [`PANEL_WINDOW`].
pub const MAX_PANELS_PER_OWNER: usize = 8;

/// The window of the allowance of one agent panel.
pub const PANEL_WINDOW: std::time::Duration = std::time::Duration::from_mins(10);

/// Refusal code of a preset name that this Horizon does not keep.
pub const UNKNOWN_PRESET: &str = "unknown_preset";

/// Refusal code of a directory that is not there, or is not a directory.
pub const DIRECTORY_MISSING: &str = "directory_missing";

/// Refusal code of an agent panel that opened [`MAX_PANELS_PER_OWNER`] panels
/// inside [`PANEL_WINDOW`].
pub const RATE_LIMITED: &str = "rate_limited";

/// Refusal code of a panel of a preset that Horizon could not open.
pub const PANEL_UNAVAILABLE: &str = "panel_unavailable";

/// Why a request of `open_panel` is not one panel that Horizon may open. `code`
/// is the stable value that the tool reports, and `message` names the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresetRefusal {
    pub code: &'static str,
    pub message: String,
}

impl PresetRefusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Check the shape of the preset name of one request: 1 to
/// [`MAX_PRESET_CHARS`] characters and no control character. The name is matched
/// against the presets of the settings, so it reaches no path, no command and no
/// program.
///
/// # Errors
/// Describes the rule with [`super::block_machines::INVALID_REQUEST`].
pub fn check_name(preset: &str) -> Result<&str, PresetRefusal> {
    let preset = preset.trim();
    let refused = |message: String| PresetRefusal::new(super::block_machines::INVALID_REQUEST, message);
    if preset.is_empty() {
        return Err(refused("preset must name one preset of the settings".into()));
    }
    if preset.chars().count() > MAX_PRESET_CHARS {
        return Err(refused(format!("preset must be at most {MAX_PRESET_CHARS} characters")));
    }
    if let Some(character) = preset.chars().find(|character| character.is_control()) {
        return Err(refused(format!("preset holds the control character {character:?}")));
    }
    Ok(preset)
}

/// Check the shape of the directory of one request: an absolute path of at most
/// [`MAX_DIRECTORY_BYTES`] bytes with no control character. The caller reads no
/// file, so a frame can run this check.
///
/// # Errors
/// Describes the rule with [`super::block_machines::INVALID_REQUEST`].
pub fn check_directory(directory: &str) -> Result<&str, PresetRefusal> {
    let refused = |message: String| PresetRefusal::new(super::block_machines::INVALID_REQUEST, message);
    if directory.is_empty() {
        return Err(refused("directory must name the folder of the panel".into()));
    }
    if directory.len() > MAX_DIRECTORY_BYTES {
        return Err(refused(format!(
            "directory must be at most {MAX_DIRECTORY_BYTES} bytes"
        )));
    }
    if let Some(character) = directory.chars().find(|character| character.is_control()) {
        return Err(refused(format!("directory holds the control character {character:?}")));
    }
    let path = Path::new(directory);
    if !path.is_absolute() {
        return Err(refused(
            "directory must be an absolute path, because a panel of a worktree runs where that task is".into(),
        ));
    }
    Ok(directory)
}

/// Check that the directory of one request is a directory of this computer. This
/// call reads the file system, so the coordination worker runs it and no frame
/// does.
///
/// # Errors
/// Answers [`DIRECTORY_MISSING`] when the folder is not there or is not a
/// folder, and [`super::block_machines::INVALID_REQUEST`] when the path breaks
/// its own rule.
pub fn check_directory_on_disk(directory: &str) -> Result<PathBuf, PresetRefusal> {
    let directory = check_directory(directory)?;
    let path = Path::new(directory);
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(path.to_path_buf()),
        Ok(_) => Err(PresetRefusal::new(
            DIRECTORY_MISSING,
            format!("{directory} is a file and not a directory"),
        )),
        Err(error) => Err(PresetRefusal::new(
            DIRECTORY_MISSING,
            format!("{directory} is not a directory of this computer: {error}"),
        )),
    }
}

/// Check the one prompt line of a request: one line of at most
/// [`MAX_PROMPT_CHARS`] characters and no control character. Horizon types the
/// trimmed line into the new panel and nothing else: it is text, so it reaches no
/// command line, no environment and no file.
///
/// # Errors
/// Describes the rule with [`super::block_machines::INVALID_REQUEST`].
pub fn check_prompt(prompt: &str) -> Result<&str, PresetRefusal> {
    let prompt = prompt.trim();
    let refused = |message: String| PresetRefusal::new(super::block_machines::INVALID_REQUEST, message);
    if prompt.is_empty() {
        return Err(refused("prompt must hold the one line to type, or be left out".into()));
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(refused(format!("prompt must be at most {MAX_PROMPT_CHARS} characters")));
    }
    if let Some(character) = prompt.chars().find(|character| character.is_control()) {
        return Err(refused(format!("prompt holds the control character {character:?}")));
    }
    Ok(prompt)
}

/// The sentence of an `unknown_preset` refusal: it lists the names and the
/// aliases of the presets of the settings and never their commands.
#[must_use]
pub fn unknown_preset_message(names: &[String]) -> String {
    if names.is_empty() {
        return "This Horizon has no preset in its settings, so ask the person to make one".into();
    }
    let listed = names.iter().take(40).map(String::as_str).collect::<Vec<_>>().join(", ");
    format!("This Horizon has no preset of that name. Its presets are: {listed}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(refusal: &PresetRefusal) -> &str {
        refusal.code
    }

    #[test]
    fn a_directory_is_absolute_bounded_and_plain() {
        assert_eq!(
            check_directory("/home/person/work").expect("a folder"),
            "/home/person/work"
        );
        for refused in ["", "work", "./work", "/work\n", "/work/one\u{7}"] {
            let refusal = check_directory(refused).expect_err("a refused folder");
            assert_eq!(
                code(&refusal),
                super::super::block_machines::INVALID_REQUEST,
                "{refused:?}"
            );
        }
        let long = format!("/{}", "a".repeat(MAX_DIRECTORY_BYTES));
        assert_eq!(
            code(&check_directory(&long).expect_err("a folder above the bound")),
            super::super::block_machines::INVALID_REQUEST
        );
        assert!(
            check_directory(&long[..MAX_DIRECTORY_BYTES]).is_ok(),
            "the bound itself passes"
        );
    }

    #[test]
    fn a_preset_name_is_one_bounded_line() {
        assert_eq!(check_name(" Claude Code ").expect("a preset"), "Claude Code");
        for refused in ["", " ", "Claude\nCode"] {
            assert_eq!(
                code(&check_name(refused).expect_err("a refused preset")),
                super::super::block_machines::INVALID_REQUEST
            );
        }
        assert!(
            check_name(&"a".repeat(MAX_PRESET_CHARS)).is_ok(),
            "the bound itself passes"
        );
        assert!(check_name(&"a".repeat(MAX_PRESET_CHARS + 1)).is_err());
    }

    #[test]
    fn a_folder_of_this_computer_passes_and_a_missing_one_or_a_file_does_not() {
        let home = tempfile::tempdir().expect("a folder");
        let directory = home.path().to_string_lossy().into_owned();
        assert_eq!(
            check_directory_on_disk(&directory).expect("the folder of the test"),
            home.path()
        );

        let missing = home.path().join("gone").to_string_lossy().into_owned();
        assert_eq!(
            code(&check_directory_on_disk(&missing).expect_err("no folder")),
            DIRECTORY_MISSING
        );

        let file = home.path().join("file");
        std::fs::write(&file, b"x").expect("the file of the test");
        let file = file.to_string_lossy().into_owned();
        let refusal = check_directory_on_disk(&file).expect_err("a file is not a folder");
        assert_eq!(code(&refusal), DIRECTORY_MISSING);
        assert!(refusal.message.contains("is a file"), "{}", refusal.message);
    }

    #[test]
    fn a_prompt_line_is_one_bounded_line_without_a_control_character() {
        assert_eq!(
            check_prompt("  Read .horizon-plan/BRIEF.md and do t4  ").expect("a line"),
            "Read .horizon-plan/BRIEF.md and do t4"
        );
        for refused in ["", "   ", "one\ntwo", "one\u{7}", "one\ttwo"] {
            assert_eq!(
                code(&check_prompt(refused).expect_err("a refused line")),
                super::super::block_machines::INVALID_REQUEST,
                "{refused:?}"
            );
        }
        assert!(
            check_prompt(&"a".repeat(MAX_PROMPT_CHARS)).is_ok(),
            "the bound itself passes"
        );
        assert_eq!(
            code(&check_prompt(&"a".repeat(MAX_PROMPT_CHARS + 1)).expect_err("a line above the bound")),
            super::super::block_machines::INVALID_REQUEST
        );
    }

    #[test]
    fn the_names_of_the_presets_are_listed_and_no_command_is() {
        let names = vec!["Claude Code".to_string(), "oc".to_string()];
        let message = unknown_preset_message(&names);
        assert!(message.contains("Claude Code, oc"), "{message}");
        assert!(
            unknown_preset_message(&[]).contains("no preset"),
            "an empty list says so"
        );
    }
}

use std::io::ErrorKind;
use std::path::Path;

use horizon_core::Config;

pub(in crate::app) fn validate_setup_saved_config(
    config_path: &Path,
    expected_saved_contents: &str,
) -> Result<(), String> {
    let saved_contents = std::fs::read_to_string(config_path).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            format!(
                "The saved configuration at {} is missing. Save the current settings to create it, or close and reopen Settings after restoring the file.",
                config_path.display()
            )
        } else {
            format!(
                "The saved configuration at {} could not be read: {error}. Restore read access, then save or close and reopen Settings before starting setup.",
                config_path.display()
            )
        }
    })?;

    Config::from_yaml(&saved_contents).map_err(|error| {
        format!(
            "The saved configuration at {} is invalid: {error}. Save the current settings to replace it, or close and reopen Settings after fixing the file.",
            config_path.display()
        )
    })?;

    if saved_contents != expected_saved_contents {
        return Err(format!(
            "The saved configuration at {} changed outside Horizon. Save the current settings to replace it, or close and reopen Settings to load the external changes.",
            config_path.display()
        ));
    }

    Ok(())
}

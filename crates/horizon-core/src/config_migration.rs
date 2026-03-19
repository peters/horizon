use std::path::Path;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::shortcuts::ShortcutBinding;

pub const CURRENT_CONFIG_VERSION: u32 = 2;

/// Run any pending migrations on `config` and write back to disk.
///
/// Returns `true` if a migration was applied.
///
/// # Errors
///
/// Returns an error if an unrecognised config version is encountered.
pub fn migrate_if_needed(config: &mut Config, config_path: &Path) -> Result<bool> {
    if config.version >= CURRENT_CONFIG_VERSION {
        return Ok(false);
    }

    let mut version = config.version;
    while version < CURRENT_CONFIG_VERSION {
        match version {
            1 => migrate_v1_to_v2(config),
            _ => {
                return Err(Error::Config(format!(
                    "unknown config version {version}, expected 1..{CURRENT_CONFIG_VERSION}"
                )));
            }
        }
        version += 1;
    }

    config.version = CURRENT_CONFIG_VERSION;

    if let Err(error) = write_back(config, config_path) {
        tracing::warn!(%error, "could not write migrated config back to disk");
    }

    Ok(true)
}

/// v1 -> v2: move all Ctrl+Key shortcuts to Ctrl+Shift+Key.
///
/// Only rewrites bindings that still match the old v1 defaults so that
/// user-customised shortcuts are left untouched.
fn migrate_v1_to_v2(config: &mut Config) {
    const REWRITES: &[(&str, &str)] = &[
        ("Ctrl+K", "Ctrl+Shift+K"),
        ("Ctrl+N", "Ctrl+Shift+N"),
        ("Ctrl+B", "Ctrl+Shift+B"),
        ("Ctrl+,", "Ctrl+Shift+Comma"),
        ("Ctrl+0", "Ctrl+Shift+0"),
        ("Ctrl+Plus", "Ctrl+Shift+Plus"),
        ("Ctrl+Minus", "Ctrl+Shift+Minus"),
        ("Ctrl+F11", "Ctrl+Shift+F11"),
        ("Ctrl+S", "Ctrl+Shift+S"),
    ];

    let fields: &mut [&mut String] = &mut [
        &mut config.shortcuts.command_palette,
        &mut config.shortcuts.new_terminal,
        &mut config.shortcuts.toggle_sidebar,
        &mut config.shortcuts.toggle_settings,
        &mut config.shortcuts.reset_view,
        &mut config.shortcuts.zoom_in,
        &mut config.shortcuts.zoom_out,
        &mut config.shortcuts.fullscreen_window,
        &mut config.shortcuts.save_editor,
    ];

    for (field, (old_default, new_default)) in fields.iter_mut().zip(REWRITES.iter()) {
        if bindings_match(field, old_default) {
            **field = (*new_default).to_string();
        }
    }
}

fn bindings_match(a: &str, b: &str) -> bool {
    match (ShortcutBinding::parse(a), ShortcutBinding::parse(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn write_back(config: &Config, path: &Path) -> Result<()> {
    let yaml = config.to_yaml()?;
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, &yaml)?;
    std::fs::rename(&tmp, path)?;
    tracing::info!("migrated config to version {CURRENT_CONFIG_VERSION}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1_YAML: &str = "\
shortcuts:
  command_palette: Ctrl+K
  new_terminal: Ctrl+N
  toggle_sidebar: Ctrl+B
  toggle_settings: \"Ctrl+,\"
  reset_view: Ctrl+0
  zoom_in: Ctrl+Plus
  zoom_out: Ctrl+Minus
  fullscreen_window: Ctrl+F11
  save_editor: Ctrl+S
";

    #[test]
    fn missing_version_defaults_to_one() {
        let config: Config = serde_yaml::from_str("{}\n").expect("should deserialize");
        assert_eq!(config.version, 1);
    }

    #[test]
    fn fresh_config_uses_current_version() {
        assert_eq!(Config::default().version, CURRENT_CONFIG_VERSION);
    }

    #[test]
    fn migration_rewrites_old_defaults() {
        let mut config: Config = serde_yaml::from_str(V1_YAML).expect("should deserialize");
        assert_eq!(config.shortcuts.command_palette, "Ctrl+K");

        migrate_v1_to_v2(&mut config);

        assert_eq!(config.shortcuts.command_palette, "Ctrl+Shift+K");
        assert_eq!(config.shortcuts.new_terminal, "Ctrl+Shift+N");
        assert_eq!(config.shortcuts.toggle_sidebar, "Ctrl+Shift+B");
        assert_eq!(config.shortcuts.toggle_settings, "Ctrl+Shift+Comma");
        assert_eq!(config.shortcuts.reset_view, "Ctrl+Shift+0");
        assert_eq!(config.shortcuts.zoom_in, "Ctrl+Shift+Plus");
        assert_eq!(config.shortcuts.zoom_out, "Ctrl+Shift+Minus");
        assert_eq!(config.shortcuts.fullscreen_window, "Ctrl+Shift+F11");
        assert_eq!(config.shortcuts.save_editor, "Ctrl+Shift+S");
    }

    #[test]
    fn migration_preserves_custom_bindings() {
        let mut config: Config =
            serde_yaml::from_str("shortcuts:\n  command_palette: Alt+K\n  save_editor: Ctrl+Shift+X\n")
                .expect("should deserialize");

        migrate_v1_to_v2(&mut config);

        assert_eq!(config.shortcuts.command_palette, "Alt+K");
        assert_eq!(config.shortcuts.save_editor, "Ctrl+Shift+X");
    }

    #[test]
    fn migration_skips_current_version() {
        let mut config = Config::default();
        let tmp = tempfile::NamedTempFile::new().expect("temp file");

        let migrated = migrate_if_needed(&mut config, tmp.path()).expect("should succeed");

        assert!(!migrated);
    }

    #[test]
    fn migration_writes_back_to_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, V1_YAML).expect("write");

        let mut config: Config = serde_yaml::from_str(V1_YAML).expect("should deserialize");
        let migrated = migrate_if_needed(&mut config, &path).expect("should succeed");

        assert!(migrated);
        assert_eq!(config.version, CURRENT_CONFIG_VERSION);

        let reloaded = std::fs::read_to_string(&path).expect("read back");
        assert!(reloaded.contains("version: 2"));
        assert!(reloaded.contains("Ctrl+Shift+K"));
    }

    #[test]
    fn serialized_config_includes_version() {
        let yaml = Config::default().to_yaml().expect("should serialize");
        assert!(yaml.contains("version: 2"));
    }
}

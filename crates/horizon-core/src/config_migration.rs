use std::path::Path;

use crate::config::{
    Config, insert_missing_gemini_presets, insert_missing_kilo_presets, insert_missing_opencode_presets,
};
use crate::error::{Error, Result};
use crate::shortcuts::ShortcutBinding;

pub const CURRENT_CONFIG_VERSION: u32 = 5;

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
            2 => migrate_v2_to_v3(config),
            3 => migrate_v3_to_v4(config),
            4 => migrate_v4_to_v5(config),
            _ => {
                return Err(Error::Config(format!(
                    "unknown config version {version}, expected 1..={CURRENT_CONFIG_VERSION}"
                )));
            }
        }
        version += 1;
    }

    config.version = CURRENT_CONFIG_VERSION;

    config.validate()?;

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
    rewrite(&mut config.shortcuts.command_palette, "Ctrl+K", "Ctrl+Shift+K");
    rewrite(&mut config.shortcuts.new_terminal, "Ctrl+N", "Ctrl+Shift+N");
    rewrite(&mut config.shortcuts.open_remote_hosts, "Ctrl+Shift+R", "Ctrl+Shift+H");
    rewrite(&mut config.shortcuts.toggle_sidebar, "Ctrl+B", "Ctrl+Shift+B");
    rewrite(&mut config.shortcuts.toggle_hud, "Ctrl+Shift+H", "Ctrl+Shift+U");
    rewrite(&mut config.shortcuts.toggle_settings, "Ctrl+,", "Ctrl+Shift+Comma");
    rewrite(&mut config.shortcuts.zoom_reset, "Ctrl+0", "Ctrl+Shift+0");
    rewrite(&mut config.shortcuts.zoom_in, "Ctrl+Plus", "Ctrl+Shift+Plus");
    rewrite(&mut config.shortcuts.zoom_out, "Ctrl+Minus", "Ctrl+Shift+Minus");
    rewrite(&mut config.shortcuts.fullscreen_window, "Ctrl+F11", "Ctrl+Shift+F11");
    rewrite(&mut config.shortcuts.save_editor, "Ctrl+S", "Ctrl+Shift+S");
}

/// v2 -> v3: add default `OpenCode` presets when they are missing.
///
/// This migration is additive and preserves custom presets.
fn migrate_v2_to_v3(config: &mut Config) {
    insert_missing_opencode_presets(&mut config.presets);
}

/// v3 -> v4: add default Gemini CLI and `KiloCode` presets when they are missing.
fn migrate_v3_to_v4(config: &mut Config) {
    insert_missing_gemini_presets(&mut config.presets);
    insert_missing_kilo_presets(&mut config.presets);
}

/// v4 -> v5: restore the standard zoom/reset bindings.
///
/// Only rewrites bindings that still match the v4 defaults so that
/// user-customised shortcuts are left untouched.
fn migrate_v4_to_v5(config: &mut Config) {
    rewrite(&mut config.shortcuts.zoom_reset, "Ctrl+Shift+0", "Ctrl+0");
    rewrite(&mut config.shortcuts.zoom_in, "Ctrl+Shift+Plus", "Ctrl+Plus");
    rewrite(&mut config.shortcuts.zoom_out, "Ctrl+Shift+Minus", "Ctrl+Minus");
}

fn rewrite(field: &mut String, old_default: &str, new_default: &str) {
    if bindings_match(field, old_default) {
        *field = new_default.to_string();
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
  open_remote_hosts: Ctrl+Shift+R
  toggle_sidebar: Ctrl+B
  toggle_hud: Ctrl+Shift+H
  toggle_settings: \"Ctrl+,\"
  reset_view: Ctrl+0
  zoom_in: Ctrl+Plus
  zoom_out: Ctrl+Minus
  fullscreen_window: Ctrl+F11
  save_editor: Ctrl+S
";

    const V2_YAML: &str = "\
version: 2
presets:
  - name: Shell
    alias: sh
    kind: shell
  - name: Codex
    alias: cx
    kind: codex
    args:
      - --no-alt-screen
    resume: last
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
        assert_eq!(config.shortcuts.open_remote_hosts, "Ctrl+Shift+H");
        assert_eq!(config.shortcuts.toggle_sidebar, "Ctrl+Shift+B");
        assert_eq!(config.shortcuts.toggle_hud, "Ctrl+Shift+U");
        assert_eq!(config.shortcuts.toggle_settings, "Ctrl+Shift+Comma");
        assert_eq!(config.shortcuts.zoom_reset, "Ctrl+Shift+0");
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
        assert!(reloaded.contains("version: 5"));
        assert!(reloaded.contains("Ctrl+Shift+K"));
        assert!(reloaded.contains("zoom_reset: Ctrl+0"));
    }

    #[test]
    fn serialized_config_includes_version() {
        let yaml = Config::default().to_yaml().expect("should serialize");
        assert!(yaml.contains("version: 5"));
    }

    #[test]
    fn migration_restores_standard_zoom_shortcuts() {
        let mut config: Config = serde_yaml::from_str(
            "version: 4\nshortcuts:\n  reset_view: Ctrl+Shift+0\n  zoom_in: Ctrl+Shift+Plus\n  zoom_out: Ctrl+Shift+Minus\n",
        )
        .expect("should deserialize");

        migrate_v4_to_v5(&mut config);

        assert_eq!(config.shortcuts.zoom_reset, "Ctrl+0");
        assert_eq!(config.shortcuts.zoom_in, "Ctrl+Plus");
        assert_eq!(config.shortcuts.zoom_out, "Ctrl+Minus");
    }

    #[test]
    fn migration_adds_missing_opencode_presets() {
        let mut config: Config = serde_yaml::from_str(V2_YAML).expect("should deserialize");

        migrate_v2_to_v3(&mut config);

        assert!(config.presets.iter().any(|preset| preset.name == "OpenCode"));
        assert!(config.presets.iter().any(|preset| preset.name == "OpenCode (Fresh)"));
    }

    #[test]
    fn migration_does_not_duplicate_existing_opencode_presets() {
        let mut config: Config = serde_yaml::from_str(
            "\
version: 2
presets:
  - name: My OpenCode
    alias: custom-oc
    kind: open_code
    resume: last
  - name: OpenCode (Fresh)
    alias: ocf
    kind: open_code
    resume: fresh
",
        )
        .expect("should deserialize");

        migrate_v2_to_v3(&mut config);

        assert_eq!(
            config
                .presets
                .iter()
                .filter(|preset| preset.kind == crate::panel::PanelKind::OpenCode)
                .count(),
            2
        );
    }

    #[test]
    fn migration_adds_missing_gemini_and_kilo_presets() {
        let mut config: Config = serde_yaml::from_str(
            "\
version: 3
presets:
  - name: Shell
    alias: sh
    kind: shell
",
        )
        .expect("should deserialize");

        migrate_v3_to_v4(&mut config);

        assert!(
            config
                .presets
                .iter()
                .any(|preset| preset.kind == crate::panel::PanelKind::Gemini)
        );
        assert_eq!(
            config
                .presets
                .iter()
                .filter(|preset| preset.kind == crate::panel::PanelKind::KiloCode)
                .count(),
            2
        );
    }
}

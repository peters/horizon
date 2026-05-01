use std::path::Path;

use crate::config::{
    Config, PresetConfig, default_claude_preset, default_codex_preset, default_kilo_preset, default_opencode_preset,
    insert_missing_gemini_presets, insert_missing_kilo_presets, insert_missing_opencode_presets,
};
use crate::error::{Error, Result};
use crate::panel::{PanelKind, PanelResume};
use crate::shortcuts::ShortcutBinding;

pub const CURRENT_CONFIG_VERSION: u32 = 7;

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
            5 => migrate_v5_to_v6(config),
            6 => migrate_v6_to_v7(config),
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

/// v5 -> v6: add the appearance block with the default auto theme.
fn migrate_v5_to_v6(_config: &mut Config) {}

/// v6 -> v7: collapse the agent preset pairs that no longer carry meaningful
/// distinctions in their default shape.
///
/// codex-cli 0.128 boots into auto mode by default and Claude Code v2.1.83+
/// has `--permission-mode auto`, so the old interactive/hands-off split has
/// stopped buying anything for those two — auto mode is now the default
/// behavior, and the single `Codex` / `Claude Code` preset uses it without
/// further qualification. The `OpenCode` and `KiloCode` pairs only differed
/// by resume mode, and we prefer always-fresh as the single default. Drop
/// default-shaped variants of `Codex`, `Codex (YOLO)`, `Claude Code`,
/// `Claude Code (Auto)`, `OpenCode`, `OpenCode (Fresh)`, `KiloCode`, and
/// `KiloCode (Fresh)`, then insert each replacement at the position of the
/// first removed entry from its agent. User-customised presets are preserved.
fn migrate_v6_to_v7(config: &mut Config) {
    collapse_agent_presets(config, "Codex", default_codex_preset, |preset| {
        matches_default_codex(preset) || matches_default_yolo(preset)
    });
    collapse_agent_presets(config, "Claude Code", default_claude_preset, |preset| {
        matches_default_claude(preset) || matches_default_claude_auto(preset)
    });
    collapse_agent_presets(
        config,
        "OpenCode",
        default_opencode_preset,
        matches_default_opencode_pair,
    );
    collapse_agent_presets(config, "KiloCode", default_kilo_preset, matches_default_kilo_pair);
}

/// Remove every preset matching `should_remove`, then insert `replacement()`
/// at the slot of the first removed preset (unless a preset named
/// `replacement_name` already exists).
fn collapse_agent_presets(
    config: &mut Config,
    replacement_name: &str,
    replacement: fn() -> PresetConfig,
    should_remove: impl Fn(&PresetConfig) -> bool,
) {
    let mut first_removed = None;
    let mut index = 0;
    while index < config.presets.len() {
        if should_remove(&config.presets[index]) {
            if first_removed.is_none() {
                first_removed = Some(index);
            }
            config.presets.remove(index);
        } else {
            index += 1;
        }
    }
    if !config.presets.iter().any(|preset| preset.name == replacement_name) {
        let position = first_removed.unwrap_or(config.presets.len()).min(config.presets.len());
        config.presets.insert(position, replacement());
    }
}

fn matches_default_codex(preset: &PresetConfig) -> bool {
    preset.name == "Codex"
        && preset.alias.as_deref() == Some("cx")
        && preset.kind == PanelKind::Codex
        && preset.command.is_none()
        && preset.args.as_slice() == ["--no-alt-screen"]
        && preset.resume == PanelResume::Last
}

/// Matches every `Codex (YOLO)` default that has shipped on `release/v0.2.6`:
/// the original `--yolo`, the broken `--full-auto` from commit `eff9335`, and
/// the explicit auto-mode args used in pre-release dev builds.
fn matches_default_yolo(preset: &PresetConfig) -> bool {
    if preset.name != "Codex (YOLO)"
        || preset.alias.as_deref() != Some("cxy")
        || preset.kind != PanelKind::Codex
        || preset.command.is_some()
        || preset.resume != PanelResume::Fresh
    {
        return false;
    }
    let args: Vec<&str> = preset.args.iter().map(String::as_str).collect();
    matches!(
        args.as_slice(),
        ["--yolo" | "--full-auto", "--no-alt-screen"]
            | [
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "--no-alt-screen"
            ]
    )
}

fn matches_default_claude(preset: &PresetConfig) -> bool {
    preset.name == "Claude Code"
        && preset.alias.as_deref() == Some("cc")
        && preset.kind == PanelKind::Claude
        && preset.command.is_none()
        && preset.args.is_empty()
        && preset.resume == PanelResume::Last
}

/// Matches the `Claude Code (Auto)` default with either of the two arg shapes
/// it has shipped with: the original `--dangerously-skip-permissions` and the
/// `--permission-mode auto` form added later in v6.
fn matches_default_claude_auto(preset: &PresetConfig) -> bool {
    if preset.name != "Claude Code (Auto)"
        || preset.alias.as_deref() != Some("cca")
        || preset.kind != PanelKind::Claude
        || preset.command.is_some()
        || preset.resume != PanelResume::Fresh
    {
        return false;
    }
    let args: Vec<&str> = preset.args.iter().map(String::as_str).collect();
    matches!(
        args.as_slice(),
        ["--dangerously-skip-permissions"] | ["--permission-mode", "auto"]
    )
}

/// Matches either of the two default `OpenCode` presets that shipped together
/// up through v6: the resume-last `OpenCode` and the resume-fresh
/// `OpenCode (Fresh)`. v7 collapses both into the single always-fresh form.
fn matches_default_opencode_pair(preset: &PresetConfig) -> bool {
    if preset.kind != PanelKind::OpenCode || preset.command.is_some() || !preset.args.is_empty() {
        return false;
    }
    matches!(
        (preset.name.as_str(), preset.alias.as_deref(), &preset.resume),
        ("OpenCode", Some("oc"), PanelResume::Last) | ("OpenCode (Fresh)", Some("ocf"), PanelResume::Fresh)
    )
}

/// Matches either of the two default `KiloCode` presets that shipped together
/// up through v6: the resume-last `KiloCode` and the resume-fresh
/// `KiloCode (Fresh)`. v7 collapses both into the single always-fresh form.
fn matches_default_kilo_pair(preset: &PresetConfig) -> bool {
    if preset.kind != PanelKind::KiloCode || preset.command.is_some() || !preset.args.is_empty() {
        return false;
    }
    matches!(
        (preset.name.as_str(), preset.alias.as_deref(), &preset.resume),
        ("KiloCode", Some("kc"), PanelResume::Last) | ("KiloCode (Fresh)", Some("kcf"), PanelResume::Fresh)
    )
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
        assert!(reloaded.contains("version: 7"));
        assert!(reloaded.contains("Ctrl+Shift+K"));
        assert!(reloaded.contains("zoom_reset: Ctrl+0"));
        assert!(reloaded.contains("appearance:"));
    }

    #[test]
    fn serialized_config_includes_version() {
        let yaml = Config::default().to_yaml().expect("should serialize");
        assert!(yaml.contains("version: 7"));
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
    fn migration_to_v6_adds_default_appearance_block() {
        let mut config: Config = serde_yaml::from_str("version: 5\nworkspaces: []\n").expect("should deserialize");

        migrate_v5_to_v6(&mut config);

        let yaml = config.to_yaml().expect("should serialize");
        assert!(yaml.contains("appearance:"));
        assert!(yaml.contains("theme: auto"));
    }

    #[test]
    fn migration_adds_missing_opencode_presets() {
        let mut config: Config = serde_yaml::from_str(V2_YAML).expect("should deserialize");

        migrate_v2_to_v3(&mut config);

        let opencode = config
            .presets
            .iter()
            .find(|preset| preset.name == "OpenCode")
            .expect("OpenCode preset");
        assert_eq!(opencode.alias.as_deref(), Some("oc"));
        assert_eq!(opencode.resume, PanelResume::Fresh);
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
        let kilo = config
            .presets
            .iter()
            .find(|preset| preset.kind == crate::panel::PanelKind::KiloCode)
            .expect("KiloCode preset");
        assert_eq!(kilo.name, "KiloCode");
        assert_eq!(kilo.alias.as_deref(), Some("kc"));
        assert_eq!(kilo.resume, PanelResume::Fresh);
    }

    const V6_DEFAULT_AGENT_PRESETS_YAML: &str = "\
version: 6
presets:
  - name: Shell
    alias: sh
    kind: shell
    resume: fresh
  - name: Codex
    alias: cx
    kind: codex
    args:
      - --no-alt-screen
    resume: last
  - name: Codex (YOLO)
    alias: cxy
    kind: codex
    args:
      - --yolo
      - --no-alt-screen
    resume: fresh
  - name: Claude Code
    alias: cc
    kind: claude
    resume: last
  - name: Claude Code (Auto)
    alias: cca
    kind: claude
    args:
      - --dangerously-skip-permissions
    resume: fresh
  - name: OpenCode
    alias: oc
    kind: open_code
    resume: last
  - name: OpenCode (Fresh)
    alias: ocf
    kind: open_code
    resume: fresh
  - name: KiloCode
    alias: kc
    kind: kilo_code
    resume: last
  - name: KiloCode (Fresh)
    alias: kcf
    kind: kilo_code
    resume: fresh
";

    #[test]
    fn migration_v6_to_v7_collapses_default_agent_pairs() {
        let mut config: Config = serde_yaml::from_str(V6_DEFAULT_AGENT_PRESETS_YAML).expect("should deserialize");

        migrate_v6_to_v7(&mut config);

        let codex_presets: Vec<_> = config
            .presets
            .iter()
            .filter(|preset| preset.kind == PanelKind::Codex)
            .collect();
        assert_eq!(codex_presets.len(), 1, "should collapse to a single codex preset");
        assert_eq!(codex_presets[0].name, "Codex");
        assert_eq!(codex_presets[0].alias.as_deref(), Some("cx"));
        assert_eq!(codex_presets[0].args, vec!["--no-alt-screen".to_string()]);
        assert_eq!(codex_presets[0].resume, PanelResume::Last);

        let claude_presets: Vec<_> = config
            .presets
            .iter()
            .filter(|preset| preset.kind == PanelKind::Claude)
            .collect();
        assert_eq!(claude_presets.len(), 1, "should collapse to a single claude preset");
        assert_eq!(claude_presets[0].name, "Claude Code");
        assert_eq!(claude_presets[0].alias.as_deref(), Some("cc"));
        assert_eq!(
            claude_presets[0].args,
            vec!["--permission-mode".to_string(), "auto".to_string()]
        );
        assert_eq!(claude_presets[0].resume, PanelResume::Last);

        let opencode_presets: Vec<_> = config
            .presets
            .iter()
            .filter(|preset| preset.kind == PanelKind::OpenCode)
            .collect();
        assert_eq!(opencode_presets.len(), 1, "should collapse to a single opencode preset");
        assert_eq!(opencode_presets[0].name, "OpenCode");
        assert_eq!(opencode_presets[0].alias.as_deref(), Some("oc"));
        assert!(opencode_presets[0].args.is_empty());
        assert_eq!(opencode_presets[0].resume, PanelResume::Fresh);

        let kilo_presets: Vec<_> = config
            .presets
            .iter()
            .filter(|preset| preset.kind == PanelKind::KiloCode)
            .collect();
        assert_eq!(kilo_presets.len(), 1, "should collapse to a single kilo preset");
        assert_eq!(kilo_presets[0].name, "KiloCode");
        assert_eq!(kilo_presets[0].alias.as_deref(), Some("kc"));
        assert!(kilo_presets[0].args.is_empty());
        assert_eq!(kilo_presets[0].resume, PanelResume::Fresh);
    }

    #[test]
    fn migration_v6_to_v7_inserts_replacements_at_first_removed_slot() {
        let mut config: Config = serde_yaml::from_str(V6_DEFAULT_AGENT_PRESETS_YAML).expect("should deserialize");

        migrate_v6_to_v7(&mut config);

        let codex_position = config
            .presets
            .iter()
            .position(|preset| preset.kind == PanelKind::Codex)
            .expect("codex preset position");
        assert_eq!(
            codex_position, 1,
            "Codex replacement should land at the first removed Codex slot"
        );
        let claude_position = config
            .presets
            .iter()
            .position(|preset| preset.kind == PanelKind::Claude)
            .expect("claude preset position");
        assert_eq!(
            claude_position, 2,
            "Claude replacement should land at the first removed Claude slot"
        );
        let opencode_position = config
            .presets
            .iter()
            .position(|preset| preset.kind == PanelKind::OpenCode)
            .expect("opencode preset position");
        assert_eq!(
            opencode_position, 3,
            "OpenCode replacement should land at the first removed OpenCode slot"
        );
        let kilo_position = config
            .presets
            .iter()
            .position(|preset| preset.kind == PanelKind::KiloCode)
            .expect("kilo preset position");
        assert_eq!(
            kilo_position, 4,
            "KiloCode replacement should land at the first removed KiloCode slot"
        );
    }

    #[test]
    fn migration_v6_to_v7_preserves_customised_opencode_presets() {
        let mut config: Config = serde_yaml::from_str(
            "\
version: 6
presets:
  - name: OpenCode
    alias: oc
    kind: open_code
    args:
      - --model
      - gpt-5
    resume: last
  - name: OpenCode (Fresh)
    alias: ocf
    kind: open_code
    resume: fresh
",
        )
        .expect("should deserialize");

        migrate_v6_to_v7(&mut config);

        // Customised OpenCode (has args) is preserved with its original name.
        let customised = config
            .presets
            .iter()
            .find(|preset| preset.alias.as_deref() == Some("oc") && !preset.args.is_empty())
            .expect("customised OpenCode preset should be preserved");
        assert_eq!(customised.name, "OpenCode");
        assert_eq!(customised.resume, PanelResume::Last);

        // Default-shaped OpenCode (Fresh) is removed; replacement is inserted.
        assert!(
            !config.presets.iter().any(|preset| preset.name == "OpenCode (Fresh)"),
            "default-shaped OpenCode (Fresh) should be removed"
        );
        // The replacement reuses the "OpenCode" name. Because the customised
        // preset already has that name, the replacement is not inserted —
        // collapse_agent_presets short-circuits when the target name exists.
        let opencode_count = config
            .presets
            .iter()
            .filter(|preset| preset.kind == PanelKind::OpenCode)
            .count();
        assert_eq!(opencode_count, 1, "only the customised preset should remain");
    }

    #[test]
    fn migration_v6_to_v7_handles_broken_full_auto_yolo_preset() {
        // Dev users who ran the broken commit (eff9335) ended up with
        // --full-auto in their YOLO preset; v6→v7 must still recognise and
        // replace it.
        let mut config: Config = serde_yaml::from_str(
            "\
version: 6
presets:
  - name: Codex (YOLO)
    alias: cxy
    kind: codex
    args:
      - --full-auto
      - --no-alt-screen
    resume: fresh
",
        )
        .expect("should deserialize");

        migrate_v6_to_v7(&mut config);

        assert!(!config.presets.iter().any(|preset| preset.name == "Codex (YOLO)"));
        let codex = config
            .presets
            .iter()
            .find(|preset| preset.name == "Codex")
            .expect("codex preset");
        assert_eq!(codex.args, vec!["--no-alt-screen".to_string()]);
    }

    #[test]
    fn migration_v6_to_v7_preserves_customised_yolo_and_claude_auto_presets() {
        // User customised the Codex (YOLO) and Claude Code (Auto) presets with
        // extra args. Neither matches the default predicate, so both are kept,
        // and the new "Codex" / "Claude Code" replacements are inserted alongside.
        let mut config: Config = serde_yaml::from_str(
            "\
version: 6
presets:
  - name: Codex (YOLO)
    alias: cxy
    kind: codex
    args:
      - --yolo
      - --no-alt-screen
      - --model
      - gpt-5
    resume: fresh
  - name: Claude Code (Auto)
    alias: cca
    kind: claude
    args:
      - --dangerously-skip-permissions
      - --model
      - opus
    resume: fresh
",
        )
        .expect("should deserialize");

        migrate_v6_to_v7(&mut config);

        assert!(
            config.presets.iter().any(|preset| preset.name == "Codex (YOLO)"),
            "customised YOLO preset should be preserved"
        );
        assert!(
            config.presets.iter().any(|preset| preset.name == "Codex"),
            "Codex replacement should be inserted"
        );
        assert!(
            config.presets.iter().any(|preset| preset.name == "Claude Code (Auto)"),
            "customised Claude Code (Auto) preset should be preserved"
        );
        assert!(
            config.presets.iter().any(|preset| preset.name == "Claude Code"),
            "Claude Code replacement should be inserted"
        );
    }

    #[test]
    fn migration_v6_to_v7_skips_replacement_when_customised_codex_keeps_the_name() {
        // User customised the plain "Codex" preset (extra args). It doesn't
        // match the default predicate, so it stays. Because a preset named
        // "Codex" already exists, the replacement is not inserted — we don't
        // want two presets sharing the same name in the menu.
        let mut config: Config = serde_yaml::from_str(
            "\
version: 6
presets:
  - name: Codex
    alias: cx
    kind: codex
    args:
      - --no-alt-screen
      - --model
      - gpt-5
    resume: last
",
        )
        .expect("should deserialize");

        migrate_v6_to_v7(&mut config);

        assert_eq!(
            config.presets.iter().filter(|preset| preset.name == "Codex").count(),
            1,
            "exactly one Codex preset should remain (the customised one)"
        );
        let codex = config
            .presets
            .iter()
            .find(|preset| preset.name == "Codex")
            .expect("codex preset");
        assert_eq!(codex.args.last().map(String::as_str), Some("gpt-5"));
    }

    #[test]
    fn migration_from_v6_via_migrate_if_needed_lands_on_codex_preset() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.yaml");
        let v6_yaml = "\
version: 6
presets:
  - name: Codex (YOLO)
    alias: cxy
    kind: codex
    args:
      - --yolo
      - --no-alt-screen
    resume: fresh
";
        std::fs::write(&path, v6_yaml).expect("write");

        let mut config: Config = serde_yaml::from_str(v6_yaml).expect("should deserialize");
        let migrated = migrate_if_needed(&mut config, &path).expect("should succeed");

        assert!(migrated);
        assert_eq!(config.version, CURRENT_CONFIG_VERSION);
        assert!(!config.presets.iter().any(|preset| preset.name == "Codex (YOLO)"));
        let codex = config
            .presets
            .iter()
            .find(|preset| preset.name == "Codex")
            .expect("codex preset");
        assert_eq!(codex.args, vec!["--no-alt-screen".to_string()]);
        assert_eq!(codex.alias.as_deref(), Some("cx"));
    }
}

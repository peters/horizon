//! Saving a discovered host as a preset, so any workspace can add it later.
use horizon_core::{Config, PanelKind, PanelResume, PresetConfig, RemoteHostsConfig, SshConnection};

use crate::app::HorizonApp;
use crate::remote_hosts_overlay::RemoteConnectMode;

/// The preset a host row's "Save … shortcut" stores. Names follow the
/// discovered `SSH: <alias>` presets so the two never duplicate each other.
pub(in crate::app) fn remote_host_shortcut(
    label: &str,
    connection: SshConnection,
    mode: RemoteConnectMode,
    remote_hosts: &RemoteHostsConfig,
) -> PresetConfig {
    let (kind, command) = match mode {
        RemoteConnectMode::Ssh => (PanelKind::Ssh, None),
        RemoteConnectMode::Vnc => (PanelKind::Device, Some(remote_hosts.vnc_target())),
    };
    PresetConfig {
        name: format!("{}: {}", mode.label(), label.trim()),
        alias: None,
        kind,
        command,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: Some(connection),
    }
}

impl HorizonApp {
    /// Persist the host as a preset and return the preset's name. A preset
    /// with that name is replaced, so re-saving updates its connection.
    pub(in crate::app) fn save_remote_host_shortcut(
        &mut self,
        label: &str,
        connection: SshConnection,
        mode: RemoteConnectMode,
    ) -> Option<String> {
        if !connection.is_valid() {
            return None;
        }
        let preset = remote_host_shortcut(label, connection, mode, &self.template_config.remote_hosts);
        let name = preset.name.clone();
        // Stage the change against the file as it is now, so an edit made
        // since the last reload survives; an unreadable or invalid file is
        // left alone, and only an absent file is written from memory.
        let staged = match std::fs::read_to_string(&self.config_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!(%error, path = %self.config_path.display(), "config file unreadable; shortcut not saved");
                return None;
            }
            Ok(source) => {
                let staged = Config::from_yaml(&source).ok().and_then(|mut on_disk| {
                    upsert_preset(&mut on_disk.presets, preset.clone());
                    on_disk.to_yaml().ok()
                });
                let Some(staged) = staged else {
                    tracing::warn!(path = %self.config_path.display(), "config file cannot be updated; shortcut not saved");
                    return None;
                };
                Some(staged)
            }
        };
        let saved = self.persist_config_change("presets", staged, |config| {
            upsert_preset(&mut config.presets, preset);
        });
        saved.then_some(name)
    }
}

/// Replace the preset with the same name (case-insensitively) or append.
fn upsert_preset(presets: &mut Vec<PresetConfig>, preset: PresetConfig) {
    match presets
        .iter_mut()
        .find(|existing| existing.name.eq_ignore_ascii_case(&preset.name))
    {
        Some(existing) => *existing = preset,
        None => presets.push(preset),
    }
}

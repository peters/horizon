//! Saving a discovered host as a preset, so any workspace can add it later.
use horizon_core::{PanelKind, PanelResume, PresetConfig, RemoteHostsConfig, SshConnection};

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
        let saved = self.persist_config_change("presets", None, |config| {
            match config
                .presets
                .iter_mut()
                .find(|existing| existing.name.eq_ignore_ascii_case(&preset.name))
            {
                Some(existing) => *existing = preset,
                None => config.presets.push(preset),
            }
        });
        saved.then_some(name)
    }
}

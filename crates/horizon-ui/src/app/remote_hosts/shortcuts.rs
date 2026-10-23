//! Saving a discovered host as a preset, so any workspace can add it later.
use horizon_core::{PanelKind, PanelResume, PresetConfig, RemoteHostsConfig};

use super::launch::RemoteLaunch;
use crate::app::HorizonApp;
use crate::remote_hosts_overlay::RemoteConnectMode;

/// The preset a host row's "Save … shortcut" stores. Names follow the
/// discovered `SSH: <alias>` presets so the two never duplicate each other.
/// A VNC shortcut freezes the port the launch resolves to, so a later change
/// of `remote_hosts` does not move an already saved preset.
pub(in crate::app) fn remote_host_shortcut(launch: RemoteLaunch, remote_hosts: &RemoteHostsConfig) -> PresetConfig {
    let (kind, command) = match launch.mode {
        RemoteConnectMode::Ssh => (PanelKind::Ssh, None),
        RemoteConnectMode::Vnc => (PanelKind::Device, Some(launch.vnc_target(remote_hosts))),
    };
    PresetConfig {
        name: format!("{}: {}", launch.mode.label(), launch.label.trim()),
        alias: None,
        kind,
        command,
        args: Vec::new(),
        resume: PanelResume::Fresh,
        ssh_connection: Some(launch.connection),
    }
}

impl HorizonApp {
    /// Persist the host as a preset and return the preset's name. A preset
    /// with that name is replaced, so re-saving updates its connection.
    pub(in crate::app) fn save_remote_host_shortcut(&mut self, launch: RemoteLaunch) -> Option<String> {
        if !launch.connection.is_valid() {
            return None;
        }
        // The settings that this run holds are the ones in the store, so the
        // preset's tunnel port comes from the reloaded settings, not from a
        // file read in this frame.
        let preset = remote_host_shortcut(launch, &self.template_config.remote_hosts);
        let name = preset.name.clone();
        let saved = self.persist_config_change("presets", |config| {
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

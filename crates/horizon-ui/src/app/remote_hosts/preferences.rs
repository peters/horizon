//! Remote Hosts preferences that live in the settings store.
use horizon_core::Config;

use crate::app::HorizonApp;

impl HorizonApp {
    /// Make `name` the workspace that receives remote sessions by default.
    pub(in crate::app) fn set_remote_hosts_default_workspace(&mut self, name: &str) -> bool {
        // Workspace names are not normalized anywhere, so the default is
        // stored exactly as the workspace is named; only a blank name is refused.
        if name.trim().is_empty() {
            return false;
        }
        // With unsaved Settings edits the live config may already say `name`
        // without the file doing so; that is a refusal, not a success.
        if self.settings_has_unsaved_edits() {
            tracing::warn!("default workspace change refused while Settings has unsaved edits");
            return false;
        }
        if self.template_config.remote_hosts.default_workspace_name() == name {
            return true;
        }
        self.persist_config_change("remote_hosts.default_workspace", |config| {
            config.remote_hosts.default_workspace = name.to_string();
        })
    }

    /// Whether the settings editor is open with edits that were not saved;
    /// `template_config` then mirrors that draft rather than the store.
    pub(in crate::app) fn settings_has_unsaved_edits(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|editor| editor.buffer != editor.original)
    }

    /// Persist one change the way the settings editor saves: the store is
    /// written, and the new config is applied live. Refused while the editor
    /// holds unsaved edits, since it re-applies its draft every frame and a
    /// later Save would take them over the change; an open editor without edits
    /// is moved onto the saved settings instead.
    pub(in crate::app) fn persist_config_change(&mut self, what: &str, mutate: impl FnOnce(&mut Config)) -> bool {
        if self.settings_has_unsaved_edits() {
            tracing::warn!(setting = what, "config change refused while Settings has unsaved edits");
            return false;
        }
        let mut config = self.template_config.clone();
        mutate(&mut config);
        self.apply_runtime_config(&config);
        // The write waits on the settings thread; its reply moves the editor
        // onto the saved settings.
        self.settings_io.save_config(self.config_path.clone(), config);
        tracing::info!(setting = what, path = %self.config_path.display(), "config change queued");
        true
    }
}

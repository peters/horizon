//! Remote Hosts preferences that live in the config file.
use horizon_core::Config;

use crate::app::HorizonApp;
use crate::app::util::atomic_write;

impl HorizonApp {
    /// Make `name` the workspace that receives remote sessions by default.
    pub(in crate::app) fn set_remote_hosts_default_workspace(&mut self, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
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
    /// `template_config` then mirrors that draft rather than the file.
    pub(in crate::app) fn settings_has_unsaved_edits(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|editor| editor.buffer != editor.original)
    }

    /// Persist one change the way the settings editor saves: the whole file
    /// is rewritten atomically and the new config is applied live. Refused
    /// while the editor holds unsaved edits, since it re-applies its draft
    /// every frame and a later Save would overwrite the file with it; an open
    /// editor without edits is moved onto the new text instead.
    fn persist_config_change(&mut self, what: &str, mutate: impl FnOnce(&mut Config)) -> bool {
        if self.settings_has_unsaved_edits() {
            tracing::warn!(setting = what, "config change refused while Settings has unsaved edits");
            return false;
        }
        let mut config = self.template_config.clone();
        mutate(&mut config);
        let written = config.to_yaml().and_then(|yaml| {
            atomic_write(&self.config_path, &yaml)
                .map(|()| yaml)
                .map_err(|error| horizon_core::Error::Config(error.to_string()))
        });
        match written {
            Ok(yaml) => {
                self.apply_runtime_config(&config);
                if let Some(editor) = self.settings.as_mut() {
                    editor.adopt_saved_text(yaml);
                }
                tracing::info!(setting = what, path = %self.config_path.display(), "config updated");
                true
            }
            Err(error) => {
                tracing::error!(setting = what, %error, "failed to persist config change");
                false
            }
        }
    }
}

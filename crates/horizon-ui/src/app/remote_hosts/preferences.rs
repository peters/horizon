//! Remote Hosts preferences that live in the config file.
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
        // Patch the one key in the file's own text so comments, ordering and
        // unknown keys survive. An existing file that cannot be patched that
        // way (unreadable, currently invalid, or a flow-style section) is left
        // alone rather than overwritten from memory; only a genuinely absent
        // file is written from the config.
        let patched = match std::fs::read_to_string(&self.config_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                tracing::warn!(%error, path = %self.config_path.display(), "config file unreadable; default workspace not changed");
                return false;
            }
            Ok(source) => {
                let patched = Config::from_yaml(&source)
                    .ok()
                    .and_then(|_| horizon_core::patch_default_workspace_source(&source, name))
                    .filter(|patched| Config::from_yaml(patched).is_ok());
                let Some(patched) = patched else {
                    tracing::warn!(path = %self.config_path.display(), "config file cannot be patched in place; default workspace not changed");
                    return false;
                };
                Some(patched)
            }
        };
        self.persist_config_change("remote_hosts.default_workspace", patched, |config| {
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

    /// Persist one change the way the settings editor saves: the settings
    /// store and its exported copy are written, and the new config is applied
    /// live. Refused while the editor holds unsaved edits, since it re-applies
    /// its draft every frame and a later Save would overwrite them; an open
    /// editor without edits is moved onto the saved text instead.
    ///
    /// `source_text` is the copy's own text with only the change applied; when
    /// given it is what gets saved, otherwise the mutated in-memory config is.
    /// Both arrive in the store as the same canonical form, so an applied
    /// value is exactly a saved value.
    pub(in crate::app) fn persist_config_change(
        &mut self,
        what: &str,
        source_text: Option<String>,
        mutate: impl FnOnce(&mut Config),
    ) -> bool {
        if self.settings_has_unsaved_edits() {
            tracing::warn!(setting = what, "config change refused while Settings has unsaved edits");
            return false;
        }
        // What gets applied is exactly what gets saved: the copy's patched text
        // parsed back, or the mutated in-memory config. The two never mix, so a
        // copy edited since the last reload cannot be applied with stale values
        // from memory.
        let mut config = self.template_config.clone();
        mutate(&mut config);
        let staged = match source_text {
            Some(text) => Config::from_yaml(&text).map(|parsed| (Some(text), parsed)),
            None => Ok((None, config)),
        };
        match staged {
            Ok((text, config)) => {
                self.apply_runtime_config(&config);
                // The write waits on the settings thread; its reply moves the
                // editor onto the saved text.
                match text {
                    Some(text) => self.settings_io.save_text(self.config_path.clone(), text),
                    None => self.settings_io.save_config(self.config_path.clone(), config),
                }
                tracing::info!(setting = what, path = %self.config_path.display(), "config change queued");
                true
            }
            Err(error) => {
                tracing::error!(setting = what, %error, "failed to stage config change");
                false
            }
        }
    }
}

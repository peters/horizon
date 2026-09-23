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

    /// Persist one change the way the settings editor saves: the whole file
    /// is rewritten atomically and the new config is applied live.
    fn persist_config_change(&mut self, what: &str, mutate: impl FnOnce(&mut Config)) -> bool {
        let mut config = self.template_config.clone();
        mutate(&mut config);
        let written = config.to_yaml().and_then(|yaml| {
            atomic_write(&self.config_path, &yaml).map_err(|error| horizon_core::Error::Config(error.to_string()))
        });
        match written {
            Ok(()) => {
                self.apply_runtime_config(&config);
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

//! The settings of the machine list: whether this computer watches the agents that
//! run on its machines.
//!
//! The watch costs one `ssh` process per machine of the list and a report every two
//! seconds, so a person on a slow link turns it off here. The setting stays with
//! this computer: another computer of the same server has its own link and its own
//! machines.

use serde::{Deserialize, Serialize};

/// Whether this computer watches the agents of the machines of its list.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct MachinesConfig {
    /// Whether the watch of the agents of the machine list is on. It is on by
    /// default: with no machine in the list and with the setting off, the watch
    /// starts no thread and no process.
    pub watch_agents: bool,
}

impl Default for MachinesConfig {
    fn default() -> Self {
        Self { watch_agents: true }
    }
}

#[cfg(test)]
mod tests {
    use super::MachinesConfig;
    use crate::Config;

    #[test]
    fn the_watch_of_the_agents_of_a_machine_is_on_by_default() {
        assert!(MachinesConfig::default().watch_agents);
        assert!(Config::default().machines.watch_agents);
        assert_eq!(Config::default().machines, MachinesConfig::default());
    }

    #[test]
    fn a_document_without_the_section_or_with_the_setting_off_reads_as_its_default() {
        let config = Config::from_json(r#"{"version": 11}"#).unwrap();
        assert!(
            config.machines.watch_agents,
            "a settings document of an older build watches the agents"
        );
        let config = Config::from_json(r#"{"version": 11, "machines": {"watch_agents": false}}"#).unwrap();
        assert!(!config.machines.watch_agents);
        let document = Config::default().to_json().unwrap();
        assert!(
            document.contains("\"watch_agents\": true"),
            "the section is written: {document}"
        );
        assert_eq!(
            Config::from_json(&document).unwrap().machines,
            MachinesConfig::default()
        );
    }
}

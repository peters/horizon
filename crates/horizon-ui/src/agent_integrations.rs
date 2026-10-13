//! `HORIZON_AGENT_INTEGRATIONS=off` keeps a Horizon out of the user's home directory. It then
//! installs no agent skills or plugins, takes no skill leases, registers no MCP servers in the
//! agents' settings and writes no desktop entry. Its own state stays in the Horizon home.

use std::ffi::OsStr;

use horizon_core::HorizonHome;

use crate::plugin_install::{self, AgentPluginHostGuard};

const ENV: &str = "HORIZON_AGENT_INTEGRATIONS";

pub(crate) struct AgentIntegrations {
    enabled: bool,
}

impl AgentIntegrations {
    /// Reads the switch. An invalid value stops Horizon with exit code 2, as an unusable
    /// `HORIZON_HOME` does.
    pub(crate) fn from_environment() -> Self {
        match enabled() {
            Ok(enabled) => Self { enabled },
            Err(problem) => {
                eprintln!("error: {problem}");
                std::process::exit(2);
            }
        }
    }

    /// Installs the agent skills and plugins and takes their leases, unless the switch is off.
    pub(crate) fn install_plugins(&self, horizon_home: &HorizonHome) -> Option<AgentPluginHostGuard> {
        self.enabled
            .then(|| plugin_install::install_agent_plugins(horizon_home))
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn install_desktop_entry(&self) {
        if self.enabled {
            crate::linux_desktop::install();
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{ENV} must be on or off, not {0:?}")]
struct InvalidValue(String);

/// Whether this Horizon installs its integrations into the user's home directory. Unset or
/// empty means on. Any value other than `on` or `off` is an error, so that a typo such as `0`
/// cannot write into the home directory that the switch was meant to protect.
fn enabled() -> Result<bool, InvalidValue> {
    enabled_from(std::env::var_os(ENV).as_deref())
}

fn enabled_from(value: Option<&OsStr>) -> Result<bool, InvalidValue> {
    match value {
        None => Ok(true),
        Some(value) if value.is_empty() || value.eq_ignore_ascii_case("on") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("off") => Ok(false),
        Some(value) => Err(InvalidValue(value.to_string_lossy().into_owned())),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::{InvalidValue, enabled_from};

    #[test]
    fn only_off_turns_the_integrations_off() {
        assert_eq!(enabled_from(None), Ok(true));
        for on in ["", "on", "ON"] {
            assert_eq!(enabled_from(Some(OsStr::new(on))), Ok(true), "{on:?}");
        }
        for off in ["off", "Off", "OFF"] {
            assert_eq!(enabled_from(Some(OsStr::new(off))), Ok(false), "{off:?}");
        }
    }

    #[test]
    fn any_other_value_is_rejected() {
        for value in ["0", "false", "no", " off", "offline"] {
            assert_eq!(
                enabled_from(Some(OsStr::new(value))),
                Err(InvalidValue(value.to_string())),
                "{value:?}"
            );
        }
    }
}

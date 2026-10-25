//! The one remote browser host that the workspace agents of this Horizon use.
//!
//! A person attaches a machine that already runs `horizon-browser host`. Horizon
//! writes this entry into the settings store; the MCP entry that Horizon gives
//! an agent then reaches that host over SSH instead of starting a local browser.
//! The settings window edits it in a later change, so the store is the only
//! writer for now.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The SSH target of a browser host and the profile its pages use.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserHostConfig {
    /// SSH target of the machine that runs `horizon-browser host`. It is the
    /// name that `ssh` resolves: an SSH config alias, a user and host, or an
    /// address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// Profile of that host whose pages the workspace agents use. A
    /// `browser_create` that names no target opens in this profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

/// The browser host of this Horizon process, read once from the settings store
/// at start.
///
/// A panel resolves its launch arguments on the UI thread, and a restore of a
/// saved panel takes the same path, so the value is held here instead of reading
/// the store again on each spawn. A change of the entry applies at the next
/// start, as the agent integration that writes the same entry does.
static BROWSER_HOST: OnceLock<Option<BrowserHostConfig>> = OnceLock::new();

/// Set the browser host that the settings name. The first call wins, and it is
/// the start of Horizon that makes it.
pub fn configure_browser_host(browser_host: Option<BrowserHostConfig>) {
    let _ = BROWSER_HOST.set(browser_host);
}

/// The browser host that the settings named at start, or `None` when the agents
/// of this Horizon use the browser of this computer.
#[must_use]
pub fn browser_host() -> Option<&'static BrowserHostConfig> {
    BROWSER_HOST.get().and_then(Option::as_ref)
}

impl BrowserHostConfig {
    /// Longest profile name, as the host file of `horizon-browser host` allows.
    pub const MAX_PROFILE_LEN: usize = 40;
    /// Longest SSH target. It is one argument of `ssh`, so a short name is
    /// enough for an alias, a `user@host` and an address.
    pub const MAX_SSH_TARGET_LEN: usize = 255;

    /// Whether the entry names a host. No host means the local browser, as
    /// before this entry existed.
    #[must_use]
    pub fn names_host(&self) -> bool {
        self.ssh_target
            .as_deref()
            .is_some_and(|target| !target.trim().is_empty())
    }

    /// The SSH target, without the surrounding spaces that a person can type.
    #[must_use]
    pub fn ssh_target(&self) -> Option<&str> {
        self.ssh_target
            .as_deref()
            .map(str::trim)
            .filter(|target| !target.is_empty())
    }

    /// The profile, without the surrounding spaces that a person can type.
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile
            .as_deref()
            .map(str::trim)
            .filter(|profile| !profile.is_empty())
    }

    /// # Errors
    /// Rejects an SSH target that `ssh` would read as an option or that holds a
    /// control character, a profile that the host file rule does not accept,
    /// and a profile without an SSH target.
    pub fn validate(&self) -> Result<()> {
        if let Some(target) = self.ssh_target() {
            if target.len() > Self::MAX_SSH_TARGET_LEN {
                return Err(Error::Config(format!(
                    "browser_host.ssh_target must be at most {} characters",
                    Self::MAX_SSH_TARGET_LEN
                )));
            }
            // `ssh` takes the target as one argument; a target that starts with
            // `-` would become an option, and whitespace would split it.
            if target.starts_with('-') || target.chars().any(char::is_whitespace) {
                return Err(Error::Config(
                    "browser_host.ssh_target must be one SSH name without a leading dash or a space".into(),
                ));
            }
            if target.chars().any(char::is_control) {
                return Err(Error::Config(
                    "browser_host.ssh_target must not hold a control character".into(),
                ));
            }
        } else if self.profile().is_some() {
            return Err(Error::Config(
                "browser_host.profile names a profile of a host, so browser_host.ssh_target is required".into(),
            ));
        }
        if let Some(profile) = self.profile()
            && !valid_profile(profile)
        {
            return Err(Error::Config(
                "browser_host.profile must be 1 to 40 of a-z, 0-9 and '-', with a letter or a digit at each end".into(),
            ));
        }
        Ok(())
    }
}

/// The arguments that reach `horizon --browser-mcp` in the MCP entry of an
/// agent: the local server of this computer, or the bridge to the named host.
///
/// No host gives `--browser-mcp` alone, so the entry of an agent is the entry
/// that Horizon wrote before this setting existed.
#[must_use]
pub fn browser_mcp_args(browser_host: Option<&BrowserHostConfig>) -> Vec<String> {
    let mut args = vec!["--browser-mcp".to_string()];
    let Some(browser_host) = browser_host.filter(|browser_host| browser_host.names_host()) else {
        return args;
    };
    if let Some(target) = browser_host.ssh_target() {
        args.extend(["--remote".to_string(), target.to_string()]);
    }
    if let Some(profile) = browser_host.profile() {
        args.extend(["--profile".to_string(), profile.to_string()]);
    }
    args
}

/// The profile-name rule of the host file of `horizon-browser host`, which is
/// `valid_name` in `crates/horizon-browser-cli/src/standalone/profile/config.rs`.
/// It is repeated here because the base crate does not depend on the CLI crate.
fn valid_profile(profile: &str) -> bool {
    let length = profile.chars().count();
    let first_and_last_are_letters_or_digits = profile
        .chars()
        .next()
        .zip(profile.chars().next_back())
        .is_some_and(|(first, last)| first.is_ascii_alphanumeric() && last.is_ascii_alphanumeric());
    (1..=BrowserHostConfig::MAX_PROFILE_LEN).contains(&length)
        && first_and_last_are_letters_or_digits
        && profile
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
}

#[cfg(test)]
mod tests {
    use super::{BrowserHostConfig, browser_mcp_args};
    use crate::Config;

    fn entry(ssh_target: Option<&str>, profile: Option<&str>) -> BrowserHostConfig {
        BrowserHostConfig {
            ssh_target: ssh_target.map(str::to_string),
            profile: profile.map(str::to_string),
        }
    }

    #[test]
    fn an_empty_entry_names_no_host() {
        let config = BrowserHostConfig::default();
        assert!(!config.names_host());
        assert!(config.validate().is_ok());
        let missing: BrowserHostConfig = serde_json::from_str("{}").expect("an empty section");
        assert!(!missing.names_host());
        assert_eq!(missing, BrowserHostConfig::default());
    }

    #[test]
    fn a_target_names_the_host_and_an_absent_profile_is_allowed() {
        let config = entry(Some("browser-vm"), None);
        assert!(config.names_host());
        assert_eq!(config.ssh_target(), Some("browser-vm"));
        assert_eq!(config.profile(), None);
        assert!(config.validate().is_ok());

        let config = entry(Some("peters@10.0.0.5"), Some("mail"));
        assert_eq!(config.ssh_target(), Some("peters@10.0.0.5"));
        assert_eq!(config.profile(), Some("mail"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn spaces_around_a_person_value_do_not_change_the_target() {
        let config = entry(Some("  browser-vm  "), Some(" mail "));
        assert_eq!(config.ssh_target(), Some("browser-vm"));
        assert_eq!(config.profile(), Some("mail"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn an_entry_without_a_host_keeps_the_arguments_of_the_local_server() {
        assert_eq!(browser_mcp_args(None), vec!["--browser-mcp"]);
        assert_eq!(
            browser_mcp_args(Some(&BrowserHostConfig::default())),
            vec!["--browser-mcp"],
            "an empty entry is no host"
        );
        assert_eq!(
            browser_mcp_args(Some(&entry(Some("  "), None))),
            vec!["--browser-mcp"],
            "an empty target is no host"
        );
        assert_eq!(
            browser_mcp_args(Some(&entry(Some("browser-vm"), None))),
            vec!["--browser-mcp", "--remote", "browser-vm"]
        );
        assert_eq!(
            browser_mcp_args(Some(&entry(Some("browser-vm"), Some("mail")))),
            vec!["--browser-mcp", "--remote", "browser-vm", "--profile", "mail"],
            "the profile of the entry travels with the target"
        );
    }

    #[test]
    fn a_target_that_ssh_would_read_as_an_option_or_split_is_rejected() {
        for target in ["-oProxyCommand=evil", "two hosts", "host\nname"] {
            let error = entry(Some(target), None).validate().expect_err(target);
            assert!(
                error.to_string().contains("browser_host.ssh_target"),
                "the error names the entry: {error}"
            );
        }
        let long = "a".repeat(BrowserHostConfig::MAX_SSH_TARGET_LEN + 1);
        assert!(entry(Some(&long), None).validate().is_err());
    }

    #[test]
    fn a_profile_without_a_host_and_a_profile_off_the_host_rule_are_rejected() {
        let error = entry(None, Some("mail")).validate().expect_err("a profile alone");
        assert!(error.to_string().contains("ssh_target is required"), "{error}");
        for profile in ["Mail", "-mail", "mail-", "mail_box", "", "a".repeat(41).as_str()] {
            if profile.is_empty() {
                assert!(
                    entry(Some("browser-vm"), Some(profile)).validate().is_ok(),
                    "blank means unset"
                );
                continue;
            }
            let error = entry(Some("browser-vm"), Some(profile)).validate().expect_err(profile);
            assert!(error.to_string().contains("browser_host.profile"), "{error}");
        }
    }

    #[test]
    fn the_section_round_trips_through_the_configuration() {
        let config = Config {
            browser_host: entry(Some("browser-vm"), Some("mail")),
            ..Config::default()
        };
        let yaml = config.to_yaml().expect("yaml");
        assert!(
            yaml.contains("browser_host:\n  ssh_target: browser-vm\n  profile: mail\n"),
            "{yaml}"
        );
        assert_eq!(
            Config::from_yaml(&yaml).expect("parse").browser_host,
            config.browser_host
        );

        let yaml = Config::default().to_yaml().expect("yaml");
        assert!(
            !yaml.contains("browser_host") || !yaml.contains("ssh_target"),
            "an entry that names no host writes no field: {yaml}"
        );
    }
}

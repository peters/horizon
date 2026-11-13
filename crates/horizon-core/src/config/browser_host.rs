//! The browser host that the browser panels of this Horizon follow.
//!
//! A person names one machine and one profile of its Chromium. A browser panel
//! is then the ordinary browser panel, whose Chromium runs on that machine over a
//! loopback SSH forward. This module owns the entry and the rule of a target and
//! a profile; the tunnel lives in `horizon-browser-control::ssh_forward` and the
//! panel in `crate::browser::attached_host`. The settings window edits the entry,
//! and a change of it applies at the next start.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The SSH target of a browser host and the profile its pages use.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserHostConfig {
    /// SSH target of the machine that runs the Chromium of the browser panels.
    /// It is the name that `ssh` resolves: an SSH config alias, a user and host,
    /// or an address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_target: Option<String>,
    /// Profile of that machine whose pages the browser panels use. A
    /// `browser_create` that names no target opens in this profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Engine of that machine: its Chromium through the `DevTools` port, or its
    /// Firefox through the driver of Horizon that runs beside it.
    #[serde(default, skip_serializing_if = "engine_is_default")]
    pub engine: BrowserHostEngine,
    /// Program that starts the driver of that machine, when the default does not
    /// fit. A non-interactive `ssh` reads a short `PATH`, so a person can write
    /// the whole path of the binary here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// The engine of the browser of a browser host.
///
/// A Chromium host is reached through its `DevTools` port and one SSH forward. A
/// Firefox host runs a driver of Horizon beside its browser
/// (`horizon --browser-host-driver`), because its `WebDriver` port refuses a
/// foreign host port and every picture is one screenshot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub enum BrowserHostEngine {
    /// The Chromium of that machine, through its `DevTools` port.
    #[default]
    #[serde(rename = "chromium")]
    Chromium,
    /// The Firefox of that machine, through the driver of Horizon that runs
    /// beside it.
    #[serde(rename = "firefox")]
    Firefox,
}

impl BrowserHostEngine {
    /// The name of the engine in the settings store and on a command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chromium => "chromium",
            Self::Firefox => "firefox",
        }
    }

    /// The backend of a browser panel that this engine serves.
    #[must_use]
    pub const fn backend(self) -> crate::browser::BackendKind {
        match self {
            Self::Chromium => crate::browser::BackendKind::ChromiumCdp,
            Self::Firefox => crate::browser::BackendKind::FirefoxBidi,
        }
    }

    /// Whether this is the engine of an entry that names none.
    #[must_use]
    pub const fn is_default(self) -> bool {
        matches!(self, Self::Chromium)
    }

    /// The engine of the driver of this host, for a host that runs a driver.
    #[must_use]
    pub const fn driver(self) -> Option<crate::browser::host_driver::HostDriverEngine> {
        match self {
            Self::Chromium => None,
            Self::Firefox => Some(crate::browser::host_driver::HostDriverEngine::Firefox),
        }
    }
}

/// Whether an entry names no engine, so it writes none: an entry of Chromium is
/// the one that every older settings file already names.
///
/// It takes a reference because that is what a serialized field asks.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn engine_is_default(engine: &BrowserHostEngine) -> bool {
    engine.is_default()
}

/// Longest command that starts the driver of a browser host, as one line that
/// the remote shell of `ssh` passes as one program.
pub const MAX_DRIVER_COMMAND_LEN: usize = 200;

/// The characters that a command of a browser host driver may hold. Every one of
/// them passes the remote shell of `ssh` without a quote, so the entry needs no
/// shell rule of its own and no value can start a second command.
const DRIVER_COMMAND_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 ._:/=@+-";

/// Whether `command` is one line that `ssh` passes to the remote shell as this
/// program alone: the allowed characters, no leading or trailing space and no
/// two spaces in a row.
#[must_use]
pub fn valid_driver_command(command: &str) -> bool {
    !command.is_empty()
        && command.len() <= MAX_DRIVER_COMMAND_LEN
        && command
            .chars()
            .all(|character| DRIVER_COMMAND_CHARS.contains(character))
        && !command.starts_with(' ')
        && !command.ends_with(' ')
        && !command.contains("  ")
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

/// The browser host that the settings named at start, or `None` when a browser
/// panel of this Horizon runs on this computer.
#[must_use]
pub fn browser_host() -> Option<&'static BrowserHostConfig> {
    BROWSER_HOST.get().and_then(Option::as_ref)
}

impl BrowserHostConfig {
    /// Longest profile name, as one folder of a browser profile allows.
    pub const MAX_PROFILE_LEN: usize = horizon_browser::MAX_PROFILE_NAME_CHARS;
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

    /// The engine of the browser of the host, when the entry names one. An
    /// entry that names no host has no engine either.
    #[must_use]
    pub const fn engine(&self) -> BrowserHostEngine {
        self.engine
    }

    /// The command that starts the driver of the host, without the surrounding
    /// spaces that a person can type.
    #[must_use]
    pub fn driver_command(&self) -> Option<&str> {
        self.command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty())
    }

    /// The reason that a create of `backend` is refused while this entry names a
    /// host: that machine runs the engine of its profile, so it serves no page
    /// of another engine, and it never serves Safari. An entry that names no
    /// host refuses nothing.
    #[must_use]
    pub fn backend_refusal(&self, backend: crate::browser::BackendKind) -> Option<&'static str> {
        self.names_host()
            .then(|| crate::browser::attached_host_backend_refusal(self.engine, backend))
            .flatten()
    }

    /// # Errors
    /// Rejects an SSH target that `ssh` would read as an option or that holds a
    /// control character, a profile that the profile-name rule does not accept,
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
        if let Some(command) = self.driver_command()
            && !valid_driver_command(command)
        {
            return Err(Error::Config(format!(
                "browser_host.command must be 1 to {MAX_DRIVER_COMMAND_LEN} characters of letters, digits and `._:/=@+-`, with single spaces between words"
            )));
        }
        if self.command.is_some() && !self.names_host() {
            return Err(Error::Config(
                "browser_host.command names the program of a host, so browser_host.ssh_target is required".into(),
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
/// agent: the local server of this computer.
///
/// A browser panel that follows `browser_host` is the ordinary browser panel,
/// whose Chromium runs on that machine over a tunnel, so the tools of an agent
/// reach it exactly as they reach a panel of this computer. The entry therefore
/// names no host.
#[must_use]
pub fn browser_mcp_args() -> Vec<String> {
    vec!["--browser-mcp".to_string()]
}

/// The profile-name rule of a browser profile, one implementation for the
/// settings entry and the remote tunnel so that both accept the same names.
fn valid_profile(profile: &str) -> bool {
    horizon_browser::valid_profile_name(profile)
}

#[cfg(test)]
mod tests {
    use super::{BrowserHostConfig, browser_mcp_args};
    use crate::Config;

    fn entry(ssh_target: Option<&str>, profile: Option<&str>) -> BrowserHostConfig {
        BrowserHostConfig {
            ssh_target: ssh_target.map(str::to_string),
            profile: profile.map(str::to_string),
            ..BrowserHostConfig::default()
        }
    }

    fn firefox_entry(ssh_target: &str) -> BrowserHostConfig {
        BrowserHostConfig {
            ssh_target: Some(ssh_target.to_string()),
            engine: super::BrowserHostEngine::Firefox,
            ..BrowserHostConfig::default()
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
    fn every_entry_of_an_agent_names_the_local_server() {
        assert_eq!(
            browser_mcp_args(),
            vec!["--browser-mcp"],
            "the tools of an agent drive the panels of this Horizon"
        );
    }

    #[test]
    fn the_engine_of_a_host_names_its_backend_and_its_driver() {
        use super::BrowserHostEngine;
        assert_eq!(BrowserHostEngine::default(), BrowserHostEngine::Chromium);
        assert_eq!(BrowserHostEngine::Chromium.as_str(), "chromium");
        assert_eq!(BrowserHostEngine::Firefox.as_str(), "firefox");
        assert_eq!(
            BrowserHostEngine::Firefox.backend(),
            crate::browser::BackendKind::FirefoxBidi
        );
        assert!(BrowserHostEngine::Chromium.is_default());
        assert!(!BrowserHostEngine::Firefox.is_default());
        assert!(
            BrowserHostEngine::Chromium.driver().is_none(),
            "a Chromium host needs no driver"
        );
        assert_eq!(
            BrowserHostEngine::Firefox.driver(),
            Some(crate::browser::host_driver::HostDriverEngine::Firefox)
        );
        assert_eq!(
            serde_json::to_string(&BrowserHostEngine::Firefox).ok().as_deref(),
            Some("\"firefox\"")
        );
    }

    #[test]
    fn the_command_of_a_driver_is_one_line_that_ssh_passes_as_one_program() {
        for command in [
            "horizon --browser-host-driver",
            "/opt/horizon/bin/horizon --browser-host-driver",
            "/home/x/my_tools/horizon --browser-host-driver",
            "/Applications/Horizon.app/Contents/MacOS/horizon --browser-host-driver",
        ] {
            assert!(super::valid_driver_command(command), "{command}");
        }
        for command in [
            "",
            " horizon --browser-host-driver",
            "horizon  --browser-host-driver",
            "horizon --browser-host-driver; rm -rf /",
            "horizon --browser-host-driver && id",
            "horizon $(id)",
            "horizon `id`",
        ] {
            assert!(!super::valid_driver_command(command), "{command}");
        }
        assert!(!super::valid_driver_command(
            &"h".repeat(super::MAX_DRIVER_COMMAND_LEN + 1)
        ));
    }

    #[test]
    fn a_named_host_serves_its_engine_and_refuses_the_others() {
        use crate::browser::BackendKind;
        let named = entry(Some("browser-vm"), None);
        assert_eq!(named.engine(), super::BrowserHostEngine::Chromium);
        assert_eq!(named.backend_refusal(BackendKind::ChromiumCdp), None);
        for backend in [BackendKind::FirefoxBidi, BackendKind::SafariWebDriver] {
            let reason = named.backend_refusal(backend).expect("a refusal");
            assert!(reason.contains("Chromium"), "{reason}");
        }

        let firefox = firefox_entry("browser-vm");
        assert_eq!(firefox.backend_refusal(BackendKind::FirefoxBidi), None);
        for backend in [BackendKind::ChromiumCdp, BackendKind::SafariWebDriver] {
            let reason = firefox.backend_refusal(backend).expect("a refusal");
            assert!(reason.contains("runs Firefox"), "the words of a Firefox host: {reason}");
        }
        let absent = BrowserHostConfig::default();
        assert!(
            absent.driver_command().is_none(),
            "an entry that names no host names no driver command"
        );
        for backend in [
            BackendKind::ChromiumCdp,
            BackendKind::FirefoxBidi,
            BackendKind::SafariWebDriver,
        ] {
            assert_eq!(
                absent.backend_refusal(backend),
                None,
                "an entry that names no host refuses no engine"
            );
        }
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
    fn the_engine_and_the_command_of_a_host_round_trip_and_are_checked() {
        let firefox = BrowserHostConfig {
            ssh_target: Some("browser-vm".to_string()),
            profile: Some("mail".to_string()),
            engine: super::BrowserHostEngine::Firefox,
            command: Some("/opt/horizon/bin/horizon --browser-host-driver".to_string()),
        };
        assert!(firefox.validate().is_ok());
        assert_eq!(
            firefox.driver_command(),
            Some("/opt/horizon/bin/horizon --browser-host-driver")
        );
        let document = crate::Config {
            browser_host: firefox.clone(),
            ..crate::Config::default()
        }
        .to_json()
        .expect("serialize");
        assert!(document.contains("\"engine\": \"firefox\""), "{document}");
        assert!(
            document.contains("\"command\": \"/opt/horizon/bin/horizon"),
            "{document}"
        );
        assert_eq!(
            crate::Config::from_json(&document).expect("parse").browser_host,
            firefox
        );

        let default_engine = entry(Some("browser-vm"), None);
        let document = crate::Config {
            browser_host: default_engine,
            ..crate::Config::default()
        }
        .to_json()
        .expect("serialize");
        assert!(
            !document.contains("\"engine\""),
            "a Chromium entry writes no engine: {document}"
        );

        for command in [
            "horizon --browser-host-driver; id",
            "horizon $(id)",
            "horizon  --browser-host-driver",
        ] {
            let broken = BrowserHostConfig {
                command: Some(command.to_string()),
                ..firefox_entry("browser-vm")
            };
            let error = broken.validate().expect_err(command);
            assert!(error.to_string().contains("browser_host.command"), "{error}");
        }
        let orphan = BrowserHostConfig {
            command: Some("horizon --browser-host-driver".to_string()),
            ..BrowserHostConfig::default()
        };
        let error = orphan.validate().expect_err("a command without a host");
        assert!(error.to_string().contains("ssh_target is required"), "{error}");
    }

    #[test]
    fn the_section_round_trips_through_the_configuration() {
        let config = Config {
            browser_host: entry(Some("browser-vm"), Some("mail")),
            ..Config::default()
        };
        let document = config.to_json().expect("serialize");
        assert!(
            document.contains("\"ssh_target\": \"browser-vm\"") && document.contains("\"profile\": \"mail\""),
            "{document}"
        );
        assert_eq!(
            Config::from_json(&document).expect("parse").browser_host,
            config.browser_host
        );

        let document = Config::default().to_json().expect("serialize");
        assert!(
            !document.contains("ssh_target"),
            "an entry that names no host writes no field: {document}"
        );
    }
}

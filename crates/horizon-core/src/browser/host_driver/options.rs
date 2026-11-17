//! The settings of one driver of a browser host.
//!
//! A link of a browser host and the session of its profile take the same command
//! line, so a person reads one command of a host in a log. The mode names the
//! profile, the engine, an optional idle bound and the bound of a private page
//! with no link.

use std::fmt::Write as _;
use std::time::Duration;

use super::HostDriverEngine;
use crate::error::{Error, Result};
use crate::util::quote;

/// The settings of one driver of a browser host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostDriverOptions {
    profile: String,
    engine: HostDriverEngine,
    idle_stop: Option<Duration>,
    unlinked_private_page: Duration,
}

/// The name of the profile that a browser host driver uses when no link names
/// one, as the browser host entry does.
pub const DEFAULT_HOST_DRIVER_PROFILE: &str = crate::browser::DEFAULT_HOST_PROFILE;

impl HostDriverOptions {
    /// # Errors
    /// The profile is not one profile name of a browser host.
    pub fn new(profile: impl Into<String>, engine: HostDriverEngine) -> Result<Self> {
        let profile = profile.into();
        if !horizon_browser::valid_profile_name(&profile) {
            return Err(Error::Config(format!(
                "`{profile}` is not one profile name of a browser host: 1 to 40 of a-z, 0-9 and '-', with a letter or a digit                  at each end, and `private` is reserved for a panel that keeps nothing"
            )));
        }
        Ok(Self {
            profile,
            engine,
            idle_stop: None,
            unlinked_private_page: super::pages::PRIVATE_PAGE_UNLINKED,
        })
    }

    /// Close a private page of this host that no link drove for `seconds`.
    ///
    /// The bound of the session is [`super::pages::PRIVATE_PAGE_UNLINKED`], and this
    /// option shortens it for a check that cannot wait ten minutes: a probe reads
    /// the rule of a private page without a clock that no person waits for.
    ///
    /// # Errors
    /// The bound is not 1 second to one day.
    pub fn with_unlinked_private_page_seconds(mut self, seconds: u64) -> Result<Self> {
        if seconds == 0 || seconds > MAX_UNLINKED_PRIVATE_PAGE_SECONDS {
            return Err(Error::Config(format!(
                "`{seconds}` is not a bound for a private page of a browser host: `--private-unlinked` takes 1 to {MAX_UNLINKED_PRIVATE_PAGE_SECONDS} seconds"
            )));
        }
        self.unlinked_private_page = Duration::from_secs(seconds);
        Ok(self)
    }

    /// The bound of a private page with no link in this session.
    #[must_use]
    pub const fn unlinked_private_page(&self) -> Duration {
        self.unlinked_private_page
    }

    /// Stop the browser of this host after `minutes` with no link attached.
    ///
    /// The session keeps its browser and its page until the person closes the
    /// panel, until the person removes the browser or until the machine of the
    /// host restarts, so this bound is off unless the entry of the host asks for
    /// it.
    ///
    /// # Errors
    /// The bound is not 1 to [`MAX_IDLE_STOP_MINUTES`] minutes.
    pub fn with_idle_stop_minutes(mut self, minutes: u32) -> Result<Self> {
        if minutes == 0 || minutes > MAX_IDLE_STOP_MINUTES {
            return Err(Error::Config(format!(
                "`{minutes}` is not a bound for an idle browser host: `--idle-stop` takes 1 to {MAX_IDLE_STOP_MINUTES} minutes, and a session without the option keeps its browser"
            )));
        }
        self.idle_stop = Some(Duration::from_secs(u64::from(minutes) * 60));
        Ok(self)
    }

    /// Read the arguments of `--browser-host-driver`: `--profile <name>`,
    /// `--engine <engine>`, `--idle-stop <minutes>` and
    /// `--private-unlinked <seconds>`, each at most once.
    ///
    /// # Errors
    /// An argument that this mode does not serve, a repeated flag, or a value
    /// that the rule of the profile, the engine and the idle bound refuses.
    pub fn parse(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Self> {
        let mut profile: Option<String> = None;
        let mut engine: Option<HostDriverEngine> = None;
        let mut idle_stop: Option<u32> = None;
        let mut unlinked_private_page: Option<u64> = None;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            let flag = argument.to_str().ok_or_else(|| {
                Error::Config(format!(
                    "`{}` is not part of the browser host driver mode",
                    argument.to_string_lossy()
                ))
            })?;
            match flag {
                "--profile" => {
                    let value = value_of(&mut arguments, flag)?;
                    if profile.replace(value).is_some() {
                        return Err(Error::Config("`--profile` is given once".into()));
                    }
                }
                "--engine" => {
                    let value = value_of(&mut arguments, flag)?;
                    if engine.replace(HostDriverEngine::parse(&value)?).is_some() {
                        return Err(Error::Config("`--engine` is given once".into()));
                    }
                }
                "--idle-stop" => {
                    let value = value_of(&mut arguments, flag)?;
                    let minutes = value.trim().parse::<u32>().map_err(|_| {
                        Error::Config(format!(
                            "`{value}` is not a number of minutes for the idle bound of a browser host"
                        ))
                    })?;
                    if idle_stop.replace(minutes).is_some() {
                        return Err(Error::Config("`--idle-stop` is given once".into()));
                    }
                }
                "--private-unlinked" => {
                    let value = value_of(&mut arguments, flag)?;
                    let seconds = value.trim().parse::<u64>().map_err(|_| {
                        Error::Config(format!(
                            "`{value}` is not a number of seconds for the bound of a private page of a browser host"
                        ))
                    })?;
                    if unlinked_private_page.replace(seconds).is_some() {
                        return Err(Error::Config("`--private-unlinked` is given once".into()));
                    }
                }
                other => {
                    return Err(Error::Config(format!(
                        "`{other}` is not part of the browser host driver mode"
                    )));
                }
            }
        }
        let options = Self::new(
            profile.unwrap_or_else(|| DEFAULT_HOST_DRIVER_PROFILE.to_string()),
            engine.unwrap_or(HostDriverEngine::Firefox),
        )?;
        let options = match idle_stop {
            Some(minutes) => options.with_idle_stop_minutes(minutes)?,
            None => options,
        };
        match unlinked_private_page {
            Some(seconds) => options.with_unlinked_private_page_seconds(seconds),
            None => Ok(options),
        }
    }

    /// The profile of the browser on the host.
    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The engine that the driver runs on the host.
    #[must_use]
    pub const fn engine(&self) -> HostDriverEngine {
        self.engine
    }

    /// The bound that stops the browser of this host with no link attached, and
    /// `None` when the entry of the host asks for none.
    #[must_use]
    pub fn idle_stop(&self) -> Option<Duration> {
        self.idle_stop
    }

    /// The arguments that start this session again, so a person reads one
    /// command of a host in a log.
    ///
    /// A bound that this link names travels with it, because the session of the
    /// profile is the process that holds pages: the link of a second panel finds
    /// the session of the first one, and the bound of that session decides for
    /// every page of the profile.
    pub(super) fn session_arguments(&self) -> String {
        let profile = quote(self.profile());
        let engine = quote(self.engine().as_str());
        let mut arguments = format!("--browser-host-driver --profile {profile} --engine {engine}");

        if let Some(idle_stop) = self.idle_stop {
            let minutes = idle_stop.as_secs() / 60;
            let _ = write!(arguments, " --idle-stop {minutes}");
        }
        if self.unlinked_private_page != super::pages::PRIVATE_PAGE_UNLINKED {
            let seconds = self.unlinked_private_page.as_secs();
            let _ = write!(arguments, " --private-unlinked {seconds}");
        }
        arguments
    }
}

/// The longest idle bound that a host entry can name, in minutes.
pub const MAX_IDLE_STOP_MINUTES: u32 = 10_080;

/// The longest bound that a person can give a private page of a browser host with
/// no link, in seconds.
pub const MAX_UNLINKED_PRIVATE_PAGE_SECONDS: u64 = 86_400;

fn value_of(arguments: &mut impl Iterator<Item = std::ffi::OsString>, flag: &str) -> Result<String> {
    let value = arguments
        .next()
        .ok_or_else(|| Error::Config(format!("`{flag}` needs one value")))?;
    value
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::Config(format!("the value of `{flag}` is not text")))
}

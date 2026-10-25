//! The browser panel whose browser runs on another machine.
//!
//! A person attaches a machine that runs `horizon-browser host` with the profile
//! of their signed-in sites. This panel shows a page of that host and sends the
//! input of the person there, over one SSH channel that carries the MCP contract
//! of the host: the same tools that an agent of another computer uses, on the
//! same server. Horizon starts that channel itself and is its client, the way the
//! cloud view of this module's neighbours is a client of a worker browser.
//!
//! The frames of the page are the PNG that `browser_screenshot` inlines, so the
//! panel never reads a private path on the remote machine. The input crosses as
//! the semantic calls of the contract: a key press, a page scroll, a navigation,
//! a viewport. A pointer position does not cross it, because the contract has no
//! coordinate pointer operation at all: a click takes an element, not a point.
//! That limit belongs to the contract that the agents use, so an agent has the
//! same limit on a local page, and a person who wants to click the page of the
//! host uses the display of that machine in a device panel.
//!
//! The actor of the pages of a panel is [`PANEL_ACTOR`], which no agent has, so a
//! page of a panel and a page of an agent never appear in each other's list.
//!
//! Nothing here waits on `ssh` on the thread that calls it. Creating a panel, and
//! the Retry of a person, start a worker thread that reads the record of the host
//! and starts its server; the outcome, including a host that never answers, is a
//! typed state that the panel draws, so the window keeps painting.

mod client;
mod input;
mod view;

#[cfg(test)]
mod tests;

use std::io;

use horizon_browser::{BrowserCommand, BrowserEventWaker};

use super::{BrowserDrainOutput, BrowserPanelState, BrowserStatus};
use view::{HostChannel, HostOpener, HostPage, HostView};

/// The identity of every page that a panel of this computer opens on a browser
/// host. A profile host takes only `horizon:` identities, and no agent uses this
/// name, so the pages of the person and the pages of an agent stay apart.
pub const PANEL_ACTOR: &str = "horizon:person:browser-panel";

/// The note that a panel of a browser host shows while it is ready: what the
/// channel carries, and the one way to click the page.
pub const POINTER_NOTE: &str = "To click this page, open the machine's display in a device panel.";

/// Why a panel backed by a browser host stopped, as a typed code and a fixed
/// message. The detail of the host stays in the panel note.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteHostFailure {
    pub code: &'static str,
    pub message: &'static str,
}

impl RemoteHostFailure {
    /// No host runs at the SSH target, or its channel could not start.
    pub const UNREACHABLE: Self = Self {
        code: "remote_host_unreachable",
        message: "the browser host of this panel could not be reached over SSH, so the panel shows no page; check the SSH target and that the host runs on that machine, then use Retry",
    };
    /// The host refused a call, with its own reason in the panel note.
    pub const REFUSED: Self = Self {
        code: "remote_host_refused",
        message: "the browser host refused a call of this panel and the page is gone; the panel shows the reason of the host",
    };
    /// The channel ended while the panel was ready.
    pub const STOPPED: Self = Self {
        code: "remote_host_stopped",
        message: "the channel to the browser host ended, so the panel shows no page; check that the host still runs, then use Retry",
    };
    /// The host did not answer within the bound of one call.
    pub const NO_ANSWER: Self = Self {
        code: "remote_host_no_answer",
        message: "the browser host did not answer within the bound of one call, so the panel shows no page; check that machine, then use Retry",
    };

    /// The typed failure of a channel that could not open. Every reason except a
    /// deadline means that this computer could not reach or not use the host;
    /// a deadline has its own code, because the host may still be starting.
    #[must_use]
    pub fn for_channel_error(error: &horizon_browser_control::remote_host::RemoteChannelError) -> Self {
        use horizon_browser_control::remote_host::RemoteChannelError;
        match error {
            RemoteChannelError::NoAnswer { .. } => Self::NO_ANSWER,
            RemoteChannelError::ActorMissing
            | RemoteChannelError::InvalidTarget { .. }
            | RemoteChannelError::HostUnavailable { .. }
            | RemoteChannelError::Transport { .. } => Self::UNREACHABLE,
        }
    }
}

/// A panel command that the channel of a browser host cannot carry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteHostRefusal {
    /// The contract has no coordinate pointer operation.
    Pointer,
    /// The contract takes no text that the US layout cannot press.
    Text,
    /// A page of a host does not take this command from a panel.
    Unsupported,
}

impl RemoteHostRefusal {
    /// The typed code of the refusal.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Pointer => "remote_host_pointer_unsupported",
            Self::Text => "remote_host_text_unsupported",
            Self::Unsupported => "remote_host_command_unsupported",
        }
    }

    /// The message that the panel shows, and that names the way forward.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Pointer => {
                "A page of a browser host takes no pointer position. Use a device panel with SSH to click the display of that machine."
            }
            Self::Text => {
                "A page of a browser host takes only the text that the US keyboard layout can press. Type the text on the display of that machine in a device panel."
            }
            Self::Unsupported => {
                "A page of a browser host does not take this command from a panel. Use the tools of the host, or the display of that machine in a device panel."
            }
        }
    }
}

/// The browser host of another machine that a panel uses as its backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteHostBackend {
    ssh_target: String,
    profile: Option<String>,
}

impl RemoteHostBackend {
    /// The backend of a panel whose page belongs to the browser host at
    /// `ssh_target`, in the profile `profile` of that host.
    ///
    /// # Errors
    /// `ssh_target` must be one name that `ssh` takes as one argument and not as
    /// an option of its own.
    pub fn new(ssh_target: impl Into<String>, profile: Option<String>) -> crate::error::Result<Self> {
        let ssh_target = ssh_target.into().trim().to_string();
        if !horizon_browser_control::remote_host::valid_target(&ssh_target) {
            return Err(crate::error::Error::Config(format!(
                "`{ssh_target}` is not one SSH name without a leading dash, a space or a control character"
            )));
        }
        let profile = profile
            .map(|profile| profile.trim().to_string())
            .filter(|profile| !profile.is_empty());
        Ok(Self { ssh_target, profile })
    }

    /// The SSH target of the host, as `ssh` resolves it.
    #[must_use]
    pub fn ssh_target(&self) -> &str {
        &self.ssh_target
    }

    /// The profile of the host that the page belongs to, when the panel names
    /// one. Without it the host chooses the profile at each create.
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }
}

/// The channel of a browser host that Horizon starts with `ssh`.
struct SshChannel(horizon_browser_control::remote_host::RemoteChannel);

/// The opener of a view: it reads the record of the host and starts the MCP
/// server of that host. It runs on the worker thread of the view, never on the
/// thread that asked for a panel or for a retry, because `ssh` can wait for a
/// host that drops packets.
fn ssh_opener(target: String) -> HostOpener {
    Box::new(move || {
        horizon_browser_control::remote_host::RemoteChannel::open(&target, PANEL_ACTOR)
            .map(|channel| Box::new(SshChannel(channel)) as Box<dyn HostChannel>)
            .map_err(|error| {
                tracing::warn!(target, %error, "the browser host of a panel is unavailable");
                RemoteHostFailure::for_channel_error(&error)
            })
    })
}

impl HostChannel for SshChannel {
    fn take_streams(&mut self) -> Option<(Box<dyn io::Write + Send>, Box<dyn io::BufRead + Send>)> {
        let input = self.0.take_input()?;
        let output = self.0.take_output()?;
        Some((Box::new(input), Box::new(io::BufReader::new(output))))
    }

    fn stop(&mut self) {
        self.0.stop();
    }
}

impl BrowserPanelState {
    /// Create a panel whose page belongs to the browser host of another machine.
    ///
    /// A remote panel keeps no local browser and no local profile: the frames,
    /// the page and its cookies stay on the machine of the host.
    ///
    /// This call returns at once. The record of the host is read, and its server
    /// is started, on the worker thread of the panel, so a host that drops
    /// packets never holds the caller. Whether the host answers is a state of the
    /// panel: `status`, `remote_status` and [`Self::remote_host_failure`] give the
    /// typed reason, and a retry opens a new channel.
    #[must_use]
    pub fn start_remote_host(
        panel_local_id: impl Into<String>,
        config: &super::BrowserConfig,
        initial_url: Option<String>,
        backend: &RemoteHostBackend,
    ) -> Self {
        let mut state = Self::inert();
        state.panel_local_id = panel_local_id.into();
        state.config = config.clone();
        state.status = BrowserStatus::Starting;
        state.loading = true;
        let requested_url = initial_url.map(|url| horizon_browser::normalize_navigation_target(&url));
        state.requested_url.clone_from(&requested_url);
        state.remote_status = Some(format!("page of the browser host {} over SSH", backend.ssh_target()));
        let page = HostPage {
            url: requested_url,
            profile: backend.profile().map(str::to_string),
        };
        let view = HostView::start(
            ssh_opener(backend.ssh_target().to_string()),
            page,
            config.backend,
            std::sync::Arc::clone(&state.frame_slot),
        );
        state.remote_host = Some(RemoteHostSlot {
            view: Some(view),
            target: backend.ssh_target().to_string(),
            profile: backend.profile().map(str::to_string),
            failure: None,
        });
        state
    }

    /// Whether the page of this panel belongs to a browser host on another
    /// machine.
    #[must_use]
    pub fn is_remote_host(&self) -> bool {
        self.remote_host.is_some()
    }

    /// The SSH target of the browser host of this panel.
    #[must_use]
    pub fn remote_host_target(&self) -> Option<&str> {
        self.remote_host.as_ref().map(|host| host.target.as_str())
    }

    /// The profile of the host that the page of this panel belongs to.
    #[must_use]
    pub fn remote_host_profile(&self) -> Option<&str> {
        self.remote_host.as_ref().and_then(|host| host.profile.as_deref())
    }

    /// Why the last channel of this panel ended before its page was ready, as
    /// the typed reason that the panel draws. `None` while a channel is live.
    #[must_use]
    pub fn remote_host_failure(&self) -> Option<RemoteHostFailure> {
        self.remote_host.as_ref().and_then(|host| host.failure)
    }

    /// The typed reason a command does not reach the page of a browser host.
    /// `None` for a local panel, and for a command that the channel carries.
    #[must_use]
    pub fn remote_host_refusal(&self, command: &BrowserCommand) -> Option<RemoteHostRefusal> {
        if !self.is_remote_host() {
            return None;
        }
        match command {
            BrowserCommand::Input(
                horizon_browser::BrowserInput::MousePress { .. } | horizon_browser::BrowserInput::MouseRelease { .. },
            ) => Some(RemoteHostRefusal::Pointer),
            BrowserCommand::DropFiles { .. }
            | BrowserCommand::Video { .. }
            | BrowserCommand::Orientation { .. }
            | BrowserCommand::HandoffDone
            | BrowserCommand::NativeSelectChoose { .. }
            | BrowserCommand::NativeSelectDismiss => Some(RemoteHostRefusal::Unsupported),
            _ => None,
        }
    }

    /// The short line that a panel of a browser host shows while it is ready.
    #[must_use]
    pub fn remote_host_note(&self) -> Option<&'static str> {
        self.is_remote_host().then_some(POINTER_NOTE)
    }

    /// What a page of a browser host can do that a provider remote session can
    /// not: its content viewport can be pinned, and the profile of the host
    /// outlives the panel.
    #[must_use]
    pub fn remote_host_capabilities(&self) -> horizon_browser::BackendCapabilities {
        let mut capabilities = horizon_browser::BackendCapabilities::remote_session();
        capabilities.viewport = true;
        capabilities.persistent_profile = true;
        capabilities
    }

    /// Replace the channel of a panel whose page belongs to a browser host. The
    /// new page belongs to the same host and the same profile, and it opens at
    /// the last page that this panel showed. A retry that cannot open the
    /// channel keeps the backend of the panel, so the next retry still names
    /// the host and never starts a browser of this computer.
    ///
    /// This call returns at once and never waits on `ssh`: the record of the
    /// host is read on the worker thread of the view.
    pub(super) fn relaunch_remote_host(&mut self) {
        let Some(host) = self.remote_host.take() else {
            return;
        };
        if let Some(view) = host.view.as_ref() {
            view.shutdown();
        }
        self.frame_slot.clear();
        self.navigation_error = None;
        let page = HostPage {
            url: self.relaunch_target(),
            profile: host.profile.clone(),
        };
        // The new channel opens on the worker thread of the view, so a retry
        // returns at once and the panel keeps painting while a dead host is
        // being contacted. The outcome arrives as the state of the panel.
        let view = HostView::start(
            ssh_opener(host.target.clone()),
            page,
            self.config.backend,
            std::sync::Arc::clone(&self.frame_slot),
        );
        self.loading = true;
        self.status = BrowserStatus::Starting;
        self.remote_host = Some(RemoteHostSlot {
            view: Some(view),
            target: host.target,
            profile: host.profile,
            failure: None,
        });
    }

    pub(super) fn send_remote_host(&self, command: BrowserCommand) -> bool {
        self.remote_host
            .as_ref()
            .and_then(|host| host.view.as_ref())
            .is_some_and(|view| view.send(command))
    }

    /// Stop the channel of this panel and keep the backend that it reports, so
    /// the panel still names its host and a retry still reaches that host.
    pub(super) fn shutdown_remote_host(&mut self) {
        if let Some(view) = self.remote_host.as_mut().and_then(|host| host.view.take()) {
            view.shutdown();
        }
    }

    pub(super) fn drain_remote_host(&mut self) -> BrowserDrainOutput {
        let Some(state) = self
            .remote_host
            .as_ref()
            .and_then(|host| host.view.as_ref())
            .and_then(HostView::take_state)
        else {
            return BrowserDrainOutput::default();
        };
        let url_changed = !state.url.is_empty() && self.url.as_deref() != Some(&state.url);
        if url_changed {
            self.url = Some(state.url.clone());
            self.pending_user_navigation = None;
        }
        self.title = state.title;
        self.config.backend = state.backend;
        self.loading = !state.ready && state.failure.is_none();
        if let Some(failure) = state.failure {
            if let Some(host) = self.remote_host.as_mut() {
                host.failure = Some(failure);
            }
            self.navigation_error = state.note;
            self.remote_status = Some(format!(
                "browser host {}: {}",
                self.remote_host_target().unwrap_or_default(),
                failure.message
            ));
            self.status = BrowserStatus::Error {
                message: failure.message.to_string(),
            };
            self.shutdown_remote_host();
        } else {
            if state.note.is_some() {
                self.navigation_error = state.note;
            }
            self.status = if state.ready {
                BrowserStatus::Ready
            } else {
                BrowserStatus::Starting
            };
        }
        BrowserDrainOutput {
            had_output: true,
            url_changed,
            config_changed: false,
        }
    }

    pub(super) fn set_remote_host_waker(&self, waker: BrowserEventWaker) {
        if let Some(view) = self.remote_host.as_ref().and_then(|host| host.view.as_ref())
            && view.needs_waker()
        {
            view.set_waker(waker);
        }
    }

    pub(super) fn needs_remote_host_waker(&self) -> bool {
        self.remote_host
            .as_ref()
            .and_then(|host| host.view.as_ref())
            .is_some_and(HostView::needs_waker)
    }
}

/// The backend that a panel reports, and the live channel to it when the panel
/// holds one. The channel ends with a failure and with a stop; the backend stays,
/// so the panel never falls back to a browser of this computer.
pub(super) struct RemoteHostSlot {
    view: Option<HostView>,
    target: String,
    profile: Option<String>,
    /// Why the last channel of this panel ended before its page was ready, as the
    /// typed reason that the panel draws.
    failure: Option<RemoteHostFailure>,
}

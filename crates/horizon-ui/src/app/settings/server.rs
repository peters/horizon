//! The Server tab: the server of this computer, the join with a link, the
//! invitation of another computer, and the list of the computers of that server.
//!
//! Three things of this tab wait: the join, the invitation and the list. None of
//! them runs on the UI thread. The frame asks for one of them, the worker of the
//! link or of the coordination thread answers, and the frame reads the answer when
//! it is there: [`ServerTabState::poll`] takes it, and a frame that has nothing to
//! read draws the state that the last answer left.
//!
//! The QR code of an invitation is built once, when its answer arrives, and the
//! plate of the window paints the modules of that code as runs of dark rectangles.
//! A frame that only paints therefore encodes nothing.

use std::time::{Duration, Instant};

use egui::{Color32, Rect, Stroke, Ui, Vec2};
use horizon_core::{Config, QUIET_ZONE, Qr, QrError};
use horizon_sync::{DeviceLogin, DeviceRow, Invited, ServerReply};

use crate::theme;

/// How often the window reads the list of the computers while an invitation is
/// open, so it learns that another computer used that invitation.
const REFRESH: Duration = Duration::from_secs(2);

/// The fewest points of one module of the QR plate at which a camera reads the
/// code from a screen. The plate of the default panel gives more than this, and a
/// test keeps the two in step.
#[cfg(test)]
const MIN_MODULE_POINTS: f32 = 4.0;

/// The widest plate of one invitation, so the invitation fits the panel above the
/// list of the computers.
const MAX_PLATE_POINTS: f32 = 240.0;

/// The quiet zone of the plate, in modules, as the QR standard asks for it.
const QUIET_MODULES: usize = QUIET_ZONE;

/// The size of a line of this tab.
const TEXT_SIZE: f32 = 12.0;
const SMALL_SIZE: f32 = 11.0;

/// What the sidebar and the link say about this computer now, so the tab draws
/// its state line without taking the lock of the link.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app) struct ServerView {
    /// The state line of the sidebar.
    pub(in crate::app) line: String,
    /// The name of this computer on the server, when it has one.
    pub(in crate::app) device: String,
    /// Whether this computer holds a link to its server.
    pub(in crate::app) linked: bool,
    /// Whether the server removed this computer, or does not know its token.
    pub(in crate::app) logged_out: bool,
    /// Whether the name of this computer belongs to the token of a login, so the
    /// field of the name is read-only.
    pub(in crate::app) name_locked: bool,
}

/// The state of the tab, which outlives the answers that it polls.
#[derive(Default)]
pub(in crate::app) struct ServerTabState {
    /// The text of the link that a person pasted.
    pub(in crate::app) link: String,
    /// The name that this computer takes on the server.
    pub(in crate::app) device: String,
    /// Whether the tab filled the device field, from the link of this computer or
    /// from the name that a first link would take. A field that a person cleared
    /// stays empty.
    device_filled: bool,
    /// The join that waits for its answer.
    pub(in crate::app) joining: bool,
    /// Why the last join did not work, in one sentence.
    pub(in crate::app) join_error: Option<String>,
    /// The invitation that is open in the window.
    pub(in crate::app) invitation: Option<OpenInvitation>,
    /// Whether an invitation waits for its answer.
    pub(in crate::app) inviting: bool,
    /// The computers of the server.
    pub(in crate::app) devices: Vec<DeviceRow>,
    /// The time of the server that answered the list.
    pub(in crate::app) devices_now_millis: u64,
    /// Whether the list waits for its answer.
    pub(in crate::app) reading_devices: bool,
    /// Why the last read of the list did not work, in one sentence.
    pub(in crate::app) devices_error: Option<String>,
    /// When the window reads the list again while an invitation is open.
    next_refresh: Option<Instant>,
    /// The computer whose removal the person confirmed once.
    pub(in crate::app) confirming: Option<String>,
    /// The computer whose removal waits for its answer.
    pub(in crate::app) removing: Option<String>,
    /// One line that the last answer of the tab wrote, such as a removal.
    pub(in crate::app) notice: Option<String>,
    /// The channel of the answer to one invitation.
    invite_rx: Option<std::sync::mpsc::Receiver<ServerReply>>,
    /// The channel of the answer to one read of the list.
    devices_rx: Option<std::sync::mpsc::Receiver<ServerReply>>,
    /// The channel of the answer to one removal.
    remove_rx: Option<std::sync::mpsc::Receiver<ServerReply>>,
    /// What the tab asked the window to do after it drew.
    pub(in crate::app) action: Option<ServerAction>,
}

/// One invitation that the window shows.
pub(in crate::app) struct OpenInvitation {
    invited: Invited,
    qr: Option<Qr>,
    refused: Option<String>,
    received: Instant,
}

impl OpenInvitation {
    /// One open invitation, for a window that shows an invitation that came from
    /// the server.
    fn new(invited: Invited) -> Self {
        let (qr, refused) = match Qr::encode(&invited.link) {
            Ok(qr) => (Some(qr), None),
            Err(QrError::TooLong(reason)) => (None, Some(reason)),
        };
        Self {
            invited,
            qr,
            refused,
            received: Instant::now(),
        }
    }

    /// The invitation of `invited` that just arrived, for a test that draws the
    /// plate of the window without a server.
    #[cfg(test)]
    pub(super) fn for_test(invited: Invited) -> Self {
        Self::new(invited)
    }

    /// The milliseconds that this invitation still lives.
    fn remaining_millis(&self) -> u64 {
        let lived = u64::try_from(self.received.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.invited.expires_millis.saturating_sub(lived)
    }

    fn expired(&self) -> bool {
        self.remaining_millis() == 0
    }

    /// The whole minutes that it still lives, for the line of the window.
    fn minutes(&self) -> u64 {
        self.remaining_millis() / 60_000
    }
}

/// What the tab asks the window to do, once the panel drew it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum ServerAction {
    /// Join the server of one link as one name.
    Join {
        /// The text of the link that a person pasted.
        link: String,
        /// The name that this computer takes on the server.
        device: String,
    },
    /// Make one invitation for another computer.
    Invite,
    /// Read the list of the computers again.
    RefreshDevices,
    /// Remove one computer.
    Remove(String),
}

impl ServerTabState {
    /// Fills the field of the name with the name that the first link of this
    /// computer would take, so a person pastes a link and presses Join without
    /// typing one. A field that a person filled or cleared is left alone.
    pub(in crate::app) fn propose_device(&mut self) {
        if self.device.trim().is_empty() && !self.device_filled {
            self.device = horizon_sync::proposed_device_name();
            self.device_filled = true;
        }
    }

    /// Takes the answers that arrived, and clears an invitation that expired.
    ///
    /// The frame calls this before it draws, so a frame that has nothing to read
    /// keeps what the last answer left and allocates nothing.
    pub(in crate::app) fn poll(&mut self) {
        if let Some(rx) = self.invite_rx.as_ref() {
            match rx.try_recv() {
                Ok(answer) => {
                    self.invite_rx = None;
                    self.inviting = false;
                    self.accept_invitation(answer);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.invite_rx = None;
                    self.inviting = false;
                    self.notice = Some("This computer has no link to the server now.".to_owned());
                }
            }
        }
        if let Some(rx) = self.devices_rx.as_ref() {
            match rx.try_recv() {
                Ok(answer) => {
                    self.devices_rx = None;
                    self.reading_devices = false;
                    self.accept_devices(answer);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.devices_rx = None;
                    self.reading_devices = false;
                    self.devices_error = Some("This computer has no link to the server now.".to_owned());
                }
            }
        }
        if let Some(rx) = self.remove_rx.as_ref() {
            match rx.try_recv() {
                Ok(answer) => {
                    self.remove_rx = None;
                    self.removing = None;
                    self.accept_removal(answer);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.remove_rx = None;
                    self.removing = None;
                    self.devices_error = Some("This computer has no link to the server now.".to_owned());
                }
            }
        }
        if self.invitation.as_ref().is_some_and(OpenInvitation::expired) {
            self.invitation = None;
            self.next_refresh = None;
        }
        // While an invitation is open, the list says whether another computer used
        // it, so the window reads the list at its own pace.
        if self.invitation.is_some()
            && self.devices_rx.is_none()
            && self.next_refresh.is_some_and(|next| Instant::now() >= next)
        {
            self.action.get_or_insert(ServerAction::RefreshDevices);
        }
    }

    /// Takes the answer of one invitation.
    pub(in crate::app) fn accept_invitation(&mut self, answer: ServerReply) {
        match answer {
            ServerReply::Invited(invited) => {
                self.invitation = Some(OpenInvitation::new(invited));
                self.next_refresh = Some(Instant::now() + REFRESH);
                self.notice = None;
            }
            ServerReply::Failed(reason) => self.notice = Some(reason),
            _ => self.notice = Some("The server answered another question than the one asked.".to_owned()),
        }
    }

    /// Takes the answer of one read of the list.
    pub(in crate::app) fn accept_devices(&mut self, answer: ServerReply) {
        match answer {
            ServerReply::Devices { rows, now_millis } => {
                self.devices = rows;
                self.devices_now_millis = now_millis;
                self.devices_error = None;
                self.next_refresh = Some(Instant::now() + REFRESH);
                // A computer that joined after the invitation was made used it. The
                // server stamps the join with its own clock, and the invitation
                // names that clock, so no window compares two clocks.
                let used = self.invitation.as_ref().is_some_and(|open| {
                    self.devices
                        .iter()
                        .any(|row| row.login == DeviceLogin::Token && row.created_millis >= open.invited.now_millis)
                });
                if used {
                    self.invitation = None;
                    self.next_refresh = None;
                    self.notice = Some("Another computer joined this server.".to_owned());
                }
            }
            ServerReply::Failed(reason) => self.devices_error = Some(reason),
            _ => self.devices_error = Some("The server answered another question than the one asked.".to_owned()),
        }
    }

    /// Takes the answer of one removal.
    pub(in crate::app) fn accept_removal(&mut self, answer: ServerReply) {
        match answer {
            ServerReply::Removed { device, removed } => {
                if removed {
                    self.notice = Some(format!("{device} was removed from the server."));
                    self.devices.retain(|row| row.name != device);
                } else {
                    self.notice = Some(format!("The server holds no computer named {device}."));
                }
                self.action.get_or_insert(ServerAction::RefreshDevices);
            }
            ServerReply::Failed(reason) => self.devices_error = Some(reason),
            _ => self.devices_error = Some("The server answered another question than the one asked.".to_owned()),
        }
    }

    /// The channel of one invitation that waits for its answer.
    pub(in crate::app) fn hold_invitation(&mut self, rx: std::sync::mpsc::Receiver<ServerReply>) {
        self.invite_rx = Some(rx);
        self.inviting = true;
    }

    /// The channel of one read of the list that waits for its answer.
    pub(in crate::app) fn hold_devices(&mut self, rx: std::sync::mpsc::Receiver<ServerReply>) {
        self.devices_rx = Some(rx);
        self.reading_devices = true;
        self.next_refresh = None;
    }

    /// The channel of one removal that waits for its answer.
    pub(in crate::app) fn hold_removal(&mut self, device: &str, rx: std::sync::mpsc::Receiver<ServerReply>) {
        self.remove_rx = Some(rx);
        self.removing = Some(device.to_owned());
        self.confirming = None;
    }

    /// Whether the tab waits for an answer that keeps a frame coming.
    pub(in crate::app) fn is_busy(&self) -> bool {
        self.joining || self.inviting || self.reading_devices || self.removing.is_some() || self.invitation.is_some()
    }
}

/// Draws the tab. Answers whether it changed the settings.
pub(super) fn render(ui: &mut Ui, config: &mut Config, state: &mut ServerTabState, view: &ServerView) -> bool {
    let mut changed = false;
    if !state.device_filled && !view.device.is_empty() {
        state.device.clone_from(&view.device);
        state.device_filled = true;
    }
    state.propose_device();
    render_state_line(ui, view);
    // The work of a logged-in computer comes first: the invitation of another
    // computer and the list of the computers. The join and the fields of the entry
    // are below them, because a person reads them once.
    if view.linked {
        render_invitation_section(ui, state);
        render_devices_section(ui, state);
    }
    // A computer that is not logged in, and one that the server removed, join
    // again from this tab: the join is the only way back to a link.
    render_join_section(ui, state, view);
    changed |= render_link_section(ui, config, state, view);
    super::validation_notice(ui, config.server.validate());
    changed
}

/// The two entries that name the server, in the card of this tab.
fn render_link_section(ui: &mut Ui, config: &mut Config, state: &mut ServerTabState, view: &ServerView) -> bool {
    super::section_heading(ui, "This computer");
    let mut changed = false;
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "The machines, the keys and the values of this computer follow one server. A change applies \
             at the next start.",
        );
        ui.add_space(8.0);
        egui::Grid::new("settings_server_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("SSH target")
                        .color(theme::FG_SOFT())
                        .size(TEXT_SIZE),
                );
                changed |= super::optional_text_field(
                    ui,
                    &mut config.server.ssh_target,
                    "user@host or an SSH alias",
                    "settings_server_target",
                );
                ui.end_row();

                ui.label(egui::RichText::new("Command").color(theme::FG_SOFT()).size(TEXT_SIZE));
                changed |= super::optional_text_field(
                    ui,
                    &mut config.server.command,
                    horizon_core::DEFAULT_LINK_COMMAND,
                    "settings_server_command",
                );
                ui.end_row();

                ui.label(egui::RichText::new("Address").color(theme::FG_SOFT()).size(TEXT_SIZE));
                changed |= super::optional_text_field(
                    ui,
                    &mut config.server.address,
                    "host:port of the listener",
                    "settings_server_address",
                );
                ui.end_row();
            });
        if !view.device.is_empty() {
            ui.add_space(6.0);
            super::dim_label(
                ui,
                &format!("The name of this computer on the server is {}.", view.device),
            );
        }
        let _ = state;
    });
    changed
}

/// The state of the link, as the sidebar shows it.
fn render_state_line(ui: &mut Ui, view: &ServerView) {
    let color = if view.logged_out {
        theme::PALETTE_RED()
    } else {
        theme::FG_SOFT()
    };
    ui.label(egui::RichText::new(&view.line).color(color).size(TEXT_SIZE));
    if view.logged_out {
        super::dim_label(
            ui,
            "The copy of the machine keys and the values that this computer already has stays on this \
             computer. Make new keys and new values if this computer is lost.",
        );
    }
    ui.add_space(8.0);
}

/// The one field of a link, the name of this computer and the button of the join.
fn render_join_section(ui: &mut Ui, state: &mut ServerTabState, view: &ServerView) {
    super::section_heading(ui, "Join with a link");
    super::section_card(ui, |ui| {
        super::dim_label(
            ui,
            "A computer that is logged in to a server shows a link for this one. Paste it here with a \
             name for this computer.",
        );
        ui.add_space(8.0);
        let mut link = state.link.clone();
        let response = ui.add(
            egui::TextEdit::singleline(&mut link)
                .hint_text("horizon://server:7000/join#CODE")
                .desired_width(ui.available_width())
                .id_salt("settings_server_join_link"),
        );
        if response.changed() {
            state.link = link;
            state.join_error = None;
        }
        ui.add_space(6.0);
        egui::Grid::new("settings_server_join_grid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new("This computer")
                        .color(theme::FG_SOFT())
                        .size(TEXT_SIZE),
                );
                let mut device = state.device.clone();
                let response = ui.add_enabled(
                    !view.name_locked,
                    egui::TextEdit::singleline(&mut device)
                        .hint_text("a name of a-z, 0-9 and -")
                        .desired_width((ui.available_width() - ui.spacing().item_spacing.x).max(120.0))
                        .id_salt("settings_server_join_device"),
                );
                if response.changed() {
                    state.device = device;
                    state.join_error = None;
                }
                ui.end_row();
            });
        if view.name_locked {
            super::dim_label(
                ui,
                "This name belongs to the login of this computer, so a new name needs a new link.",
            );
        }
        ui.add_space(8.0);
        let busy = state.joining;
        let error = state.join_error.clone();
        let logged_out = view.logged_out;
        let ready = !busy && !state.link.trim().is_empty() && !state.device.trim().is_empty();
        let mut asked = None;
        super::action_row(
            ui,
            |ui| {
                if let Some(error) = error.as_ref() {
                    ui.label(egui::RichText::new(error).color(theme::PALETTE_RED()).size(SMALL_SIZE));
                } else if busy {
                    ui.label(
                        egui::RichText::new("This computer joins the server.")
                            .color(theme::FG_DIM())
                            .size(SMALL_SIZE),
                    );
                } else if logged_out {
                    ui.label(
                        egui::RichText::new("This computer joins the server again with a new link.")
                            .color(theme::FG_DIM())
                            .size(SMALL_SIZE),
                    );
                }
            },
            |ui| {
                if ui
                    .add_enabled(ready, crate::app::util::primary_button("Join"))
                    .clicked()
                {
                    asked = Some(ServerAction::Join {
                        link: state.link.trim().to_owned(),
                        device: state.device.trim().to_owned(),
                    });
                }
            },
        );
        if let Some(action) = asked {
            state.joining = true;
            state.join_error = None;
            state.action = Some(action);
        }
    });
}

/// The invitation of another computer: its QR code, its link and its code.
fn render_invitation_section(ui: &mut Ui, state: &mut ServerTabState) {
    super::section_heading(ui, "Add a computer");
    super::section_card(ui, |ui| {
        if let Some(invitation) = state.invitation.as_ref() {
            let refusal = invitation.refused.as_ref();
            if let Some(refused) = refusal {
                ui.label(
                    egui::RichText::new(refused)
                        .color(theme::PALETTE_RED())
                        .size(SMALL_SIZE),
                );
                return;
            }
            {
                let size = plate_size(ui.available_width());
                if let Some(qr) = invitation.qr.as_ref() {
                    plate(ui, qr, size);
                }
                ui.add_space(8.0);
                let code = invitation.invited.code.clone();
                ui.label(
                    egui::RichText::new(&code)
                        .color(theme::FG())
                        .size(18.0)
                        .monospace()
                        .strong(),
                );
                ui.add_space(4.0);
                let mut link = invitation.invited.link.clone();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut link)
                        .desired_width(ui.available_width())
                        .interactive(false)
                        .id_salt("settings_server_invitation_link"),
                );
                let _ = response;
                ui.add_space(6.0);
                super::action_row(
                    ui,
                    |ui| {
                        ui.label(
                            egui::RichText::new(format!("{} minutes left", invitation.minutes()))
                                .color(theme::FG_DIM())
                                .size(SMALL_SIZE),
                        );
                    },
                    |ui| {
                        if ui.add(crate::app::util::chrome_button("Copy link")).clicked() {
                            ui.ctx().copy_text(link.clone());
                        }
                    },
                );
            }
        } else {
            {
                super::dim_label(
                    ui,
                    "One invitation works for one computer, for ten minutes, and for one use.",
                );

                ui.add_space(8.0);
                super::action_row(
                    ui,
                    |ui| {
                        if state.inviting {
                            ui.label(
                                egui::RichText::new("The server makes an invitation.")
                                    .color(theme::FG_DIM())
                                    .size(SMALL_SIZE),
                            );
                        }
                    },
                    |ui| {
                        if ui
                            .add_enabled(!state.inviting, crate::app::util::primary_button("Add a computer"))
                            .clicked()
                        {
                            state.action = Some(ServerAction::Invite);
                        }
                    },
                );
            }
        }
        if let Some(notice) = state.notice.as_ref() {
            ui.add_space(6.0);
            super::dim_label(ui, notice);
        }
    });
}

/// The list of the computers of the server, with the removal of one of them.
///
/// One computer takes two lines: its name, and how it logs in with the times that
/// the server gave. Both lines wrap inside the width that the buttons of the row
/// leave, so a long name of a computer and a narrow panel never put a button over
/// a word.
fn render_devices_section(ui: &mut Ui, state: &mut ServerTabState) {
    super::section_heading(ui, "Computers");
    super::section_card(ui, |ui| {
        if state.devices.is_empty() {
            super::dim_label(
                ui,
                if state.reading_devices {
                    "The window reads the computers of the server."
                } else {
                    "The server names no computer yet."
                },
            );
        }
        for row in &state.devices {
            let name = row.name.clone();
            // The console of the server computer itself is not a computer that a
            // person removes: the login of that console belongs to the server, and a
            // removal reads as if it removed the server.
            let server_computer = row.is_server_console();
            let confirming = !server_computer && state.confirming.as_deref() == Some(name.as_str());
            let removing = !server_computer && state.removing.as_deref() == Some(name.as_str());
            let detail = format!(
                "{} · seen {} · joined {}",
                if row.login == DeviceLogin::Token { "link" } else { "ssh" },
                ago(state.devices_now_millis, row.last_seen_millis),
                ago(state.devices_now_millis, row.created_millis)
            );
            super::action_row(
                ui,
                |ui| {
                    let mut label = egui::RichText::new(&name).color(theme::FG()).size(TEXT_SIZE);
                    if row.caller {
                        label = label.strong();
                    }
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(label).wrap());
                        ui.add(
                            egui::Label::new(egui::RichText::new(detail).color(theme::FG_DIM()).size(SMALL_SIZE))
                                .wrap(),
                        );
                    });
                },
                |ui| {
                    if server_computer {
                        ui.label(
                            egui::RichText::new("this server")
                                .color(theme::FG_DIM())
                                .size(SMALL_SIZE),
                        );
                    } else if confirming {
                        if ui.add(crate::app::util::chrome_button("Remove")).clicked() {
                            state.removing = Some(name.clone());
                            state.confirming = None;
                            state.action = Some(ServerAction::Remove(name.clone()));
                        }
                        if ui.add(crate::app::util::chrome_button("Keep")).clicked() {
                            state.confirming = None;
                        }
                    } else if removing {
                        ui.label(egui::RichText::new("Removing").color(theme::FG_DIM()).size(SMALL_SIZE));
                    } else if ui
                        .add_enabled(state.removing.is_none(), crate::app::util::chrome_button("Remove"))
                        .clicked()
                    {
                        state.confirming = Some(name.clone());
                    }
                },
            );
            if confirming {
                super::dim_label(
                    ui,
                    "This computer loses its login and the links that are open. It keeps the copies of the \
                     keys and the values that it already has, so make new keys if it is lost.",
                );
            }
            ui.add_space(4.0);
        }
        if let Some(error) = state.devices_error.as_ref() {
            ui.label(egui::RichText::new(error).color(theme::PALETTE_RED()).size(SMALL_SIZE));
        }
        ui.add_space(4.0);
        super::action_row(
            ui,
            |_ui| {},
            |ui| {
                if ui
                    .add_enabled(!state.reading_devices, crate::app::util::chrome_button("Read the list"))
                    .clicked()
                {
                    state.action = Some(ServerAction::RefreshDevices);
                }
            },
        );
    });
}

/// How long ago one time of the server was, in two words.
fn ago(now_millis: u64, then_millis: u64) -> String {
    let millis = now_millis.saturating_sub(then_millis);
    let minutes = millis / 60_000;
    if minutes < 1 {
        "just now".to_owned()
    } else if minutes < 60 {
        format!("{minutes} min ago")
    } else if minutes < 60 * 24 {
        format!("{} h ago", minutes / 60)
    } else {
        format!("{} d ago", minutes / (60 * 24))
    }
}

/// The side of the plate of one invitation: the width that the panel gives, up to
/// [`MAX_PLATE_POINTS`]. A plate is never wider than the panel, because a code that
/// the panel cuts is a code that no camera reads. A panel that is too narrow for
/// [`MIN_MODULE_POINTS`] per module shows a small plate, and a wider window shows a
/// readable one.
fn plate_size(available: f32) -> f32 {
    available.clamp(1.0, MAX_PLATE_POINTS)
}

/// `count` modules as points, for the plate of one code. The side of a QR code is
/// 177 modules at most, so this conversion never loses a point in practice.
fn points(count: usize) -> f32 {
    f32::from(u16::try_from(count).unwrap_or(u16::MAX))
}

/// Paints the modules of `qr` on a white plate with a quiet zone.
///
/// The plate is light in both themes, because a camera reads a dark code on a light
/// plate and a code that follows the theme reads as nothing in the dark one. One
/// run of dark modules of a row is one rectangle, so a frame paints a few hundred
/// rectangles and never touches the encoder.
fn plate(ui: &mut Ui, qr: &Qr, width: f32) {
    let modules = qr.size() + 2 * QUIET_MODULES;
    let module = width / points(modules);
    let side = module * points(modules);
    let (rect, _response) = ui.allocate_exact_size(Vec2::splat(side), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, Color32::WHITE);
    let origin = rect.min + Vec2::splat(module * points(QUIET_MODULES));
    for y in 0..qr.size() {
        let mut x = 0;
        while x < qr.size() {
            if !qr.is_dark(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < qr.size() && qr.is_dark(x, y) {
                x += 1;
            }
            let from = origin + Vec2::new(points(start) * module, points(y) * module);
            let run = Rect::from_min_size(from, Vec2::new(points(x - start) * module, module));
            painter.rect_filled(run, 0.0, Color32::BLACK);
        }
    }
    // The plate is a picture of a code, so its edge is the edge that a camera
    // reads: a thin border keeps it apart from a light panel.
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0_f32, theme::BORDER_SUBTLE()),
        egui::StrokeKind::Inside,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const MADE: u64 = 1_760_000_000_000;

    fn invitation(code: &str) -> Invited {
        Invited {
            code: code.to_owned(),
            link: format!("horizon://server-computer:7000/join#{code}"),
            expires_millis: 600_000,
            now_millis: MADE,
        }
    }

    #[test]
    fn the_tab_draws_an_open_invitation_and_the_computers_it_lists() {
        let mut state = ServerTabState::default();
        state.accept_invitation(ServerReply::Invited(invitation("ABCD-EFGH-JKMN")));
        state.accept_devices(ServerReply::Devices {
            rows: vec![DeviceRow {
                name: "laptop-b".to_owned(),
                created_millis: 1,
                last_seen_millis: 2,
                login: DeviceLogin::Token,
                caller: false,
            }],
            now_millis: 3,
        });
        let view = ServerView {
            line: "Server: in step".to_owned(),
            device: "laptop-a".to_owned(),
            linked: true,
            logged_out: false,
            name_locked: true,
        };
        let mut config = Config::default();
        let texts = tall_frame(|ui| render(ui, &mut config, &mut state, &view));
        assert!(texts.iter().any(|text| text == "ABCD-EFGH-JKMN"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "laptop-b"), "{texts:?}");
        assert!(
            texts.iter().any(|text| text == "laptop-c" || text == "laptop-b"),
            "{texts:?}"
        );
        assert!(texts.iter().any(|text| text == "Computers"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "Add a computer"), "{texts:?}");
    }

    #[test]
    fn a_computer_that_is_not_logged_in_draws_the_join_and_says_why() {
        // The two states that no attempt of the link can leave read as their
        // reason, and both offer the join: it is the only way back to a link.
        for line in [
            "This computer is not logged in to the server.",
            "This computer was removed from the server.",
        ] {
            let mut state = ServerTabState::default();
            let view = ServerView {
                line: line.to_owned(),
                device: "window-b".to_owned(),
                linked: false,
                logged_out: true,
                name_locked: false,
            };
            let mut config = Config::default();
            let texts = tall_frame(|ui| render(ui, &mut config, &mut state, &view));
            assert!(texts.iter().any(|text| text == line), "{texts:?}");
            assert!(texts_hold_join(&texts), "the join is shown for {line}: {texts:?}");
            assert!(
                !texts.iter().any(|text| text == "Computers"),
                "the list of a link that ended is not drawn: {texts:?}"
            );
            assert!(
                state.device == "window-b",
                "the name of the removed computer stays in the field"
            );
        }
    }

    #[test]
    fn a_fresh_computer_shows_the_name_that_its_join_would_take() {
        let mut state = ServerTabState::default();
        let view = ServerView {
            line: "This computer has no server.".to_owned(),
            ..ServerView::default()
        };
        let mut config = Config::default();
        let texts = tall_frame(|ui| render(ui, &mut config, &mut state, &view));
        assert!(
            texts.iter().any(|text| text == "This computer has no server."),
            "{texts:?}"
        );
        assert!(texts_hold_join(&texts), "{texts:?}");
        assert!(
            state.device.starts_with("horizon-"),
            "the field holds the name that a link would take: {:?}",
            state.device
        );
        let proposed = state.device.clone();
        let _ = tall_frame(|ui| render(ui, &mut config, &mut state, &view));
        assert_eq!(state.device, proposed, "a second frame proposes the same name");
    }

    #[test]
    fn the_name_of_a_login_is_read_only_and_says_that_it_belongs_to_the_login() {
        let mut state = ServerTabState {
            device: "window-b".to_owned(),
            ..ServerTabState::default()
        };
        let view = ServerView {
            line: "Server: in step".to_owned(),
            device: "window-b".to_owned(),
            linked: true,
            logged_out: false,
            name_locked: true,
        };
        let mut config = Config::default();
        let texts = tall_frame(|ui| render(ui, &mut config, &mut state, &view));
        assert!(
            texts
                .iter()
                .any(|text| text == "This name belongs to the login of this computer, so a new name needs a new link."),
            "the field of a login says why it is read-only: {texts:?}"
        );
        assert_eq!(state.device, "window-b", "a frame writes no other name");
    }

    /// Whether the tab draws the heading of the join.
    fn texts_hold_join(texts: &[String]) -> bool {
        texts.iter().any(|text| text == "Join with a link")
    }

    /// The texts of one frame of the tab at a height that holds the whole tab, so
    /// no section is cut and no section is reported as missing.
    fn tall_frame(mut render: impl FnMut(&mut egui::Ui) -> bool) -> Vec<String> {
        use crate::test_egui::DiscardTextures;

        let ctx = egui::Context::default();
        let output = ctx
            .run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 2200.0))),
                    ..egui::RawInput::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            render(ui);
                        });
                    });
                },
            )
            .discard_textures();
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect_shapes(&clipped.shape, &mut texts);
        }
        texts
    }

    fn collect_shapes(shape: &egui::Shape, texts: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => texts.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_shapes(shape, texts);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn an_invitation_names_the_minutes_that_it_lives() {
        let open = OpenInvitation::new(invitation("ABCD-EFGH-JKMN"));
        assert_eq!(open.minutes(), 10);
        assert!(!open.expired());
        assert!(open.qr.is_some(), "the plate has one code per invitation");
        let short = OpenInvitation {
            received: Instant::now()
                .checked_sub(Duration::from_secs(600))
                .expect("a time of a test that is not a machine start"),
            ..OpenInvitation::new(invitation("ABCD-EFGH-JKMN"))
        };
        assert!(short.expired(), "an invitation of ten minutes leaves the window");
        assert_eq!(short.minutes(), 0);
    }

    #[test]
    fn a_long_age_reads_in_two_words() {
        let now = 1_000_000_000_u64;
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now, now - 60_000), "1 min ago");
        assert_eq!(ago(now, now - 59 * 60_000), "59 min ago");
        assert_eq!(ago(now, now - 3 * 60 * 60_000), "3 h ago");
        assert_eq!(ago(now, now - 5 * 24 * 60 * 60_000), "5 d ago");
        assert_eq!(ago(now, now + 60_000), "just now", "a clock that is ahead reads as now");
    }

    #[test]
    fn the_plate_of_a_code_keeps_its_modules_readable_and_inside_the_panel() {
        let qr = Qr::encode("horizon://server-computer:7000/join#ABCD-EFGH-JKMN").expect("a code");
        let modules = qr.size() + 2 * QUIET_ZONE;
        for available in [120.0_f32, 238.0, 396.0] {
            let side = plate_size(available);
            assert!(side <= available, "a plate of {side} fits the {available} of the panel");
            assert!(side <= MAX_PLATE_POINTS);
        }
        // The default panel of this build gives more than four points per module.
        let default = plate_size(crate::app::settings::settings_panel_default_width(1600.0));
        assert!(
            default / points(modules) >= MIN_MODULE_POINTS,
            "one module of {default} points over {modules} modules is readable"
        );
    }

    #[test]
    fn the_answer_of_an_invitation_or_of_a_removal_leaves_one_line() {
        let mut state = ServerTabState::default();
        state.accept_invitation(ServerReply::Failed("The server has no listener.".to_owned()));
        assert_eq!(state.notice.as_deref(), Some("The server has no listener."));
        state.notice = None;
        state.accept_removal(ServerReply::Removed {
            device: "laptop-b".to_owned(),
            removed: true,
        });
        assert_eq!(state.notice.as_deref(), Some("laptop-b was removed from the server."));
    }

    #[test]
    fn a_new_computer_ends_the_invitation_that_it_used() {
        let mut state = ServerTabState::default();
        state.accept_devices(ServerReply::Devices {
            rows: vec![DeviceRow {
                name: "laptop-a".to_owned(),
                created_millis: MADE - 60_000,
                last_seen_millis: MADE - 1_000,
                login: DeviceLogin::Ssh,
                caller: true,
            }],
            now_millis: MADE,
        });
        state.accept_invitation(ServerReply::Invited(invitation("ABCD-EFGH-JKMN")));
        assert!(state.invitation.is_some());
        // A computer that was already there and one that joined after the
        // invitation: only the second one used it.
        state.accept_devices(ServerReply::Devices {
            rows: vec![
                DeviceRow {
                    name: "laptop-a".to_owned(),
                    created_millis: MADE - 60_000,
                    last_seen_millis: MADE,
                    login: DeviceLogin::Ssh,
                    caller: true,
                },
                DeviceRow {
                    name: "laptop-b".to_owned(),
                    created_millis: MADE + 30_000,
                    last_seen_millis: MADE + 30_000,
                    login: DeviceLogin::Token,
                    caller: false,
                },
            ],
            now_millis: MADE + 31_000,
        });
        assert!(state.invitation.is_none(), "the invitation was used");
        assert_eq!(state.notice.as_deref(), Some("Another computer joined this server."));
    }
}

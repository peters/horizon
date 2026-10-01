//! A mock of distributed messaging, to see how it would feel (prototype: fixture data, nothing is sent).
//!
//! Everyone, people and agents, has a handle such as `@maria` or `@maria/claude`. Messages go to a channel
//! (`#release`), a person (`@maria`) or a thread. What arrives from someone else lands in the inbox as
//! quoted data with the sender's trust level on it; it is never an instruction to the agent. A stranger
//! has to knock first.

use std::collections::BTreeMap;

use egui::{
    Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, TextEdit, Ui, pos2, vec2,
};

use super::super::super::blocks;
use super::super::HorizonApp;
use super::super::desk_bar::paint::elide;
use crate::theme;

/// The person's own handle.
pub(in crate::app::assistant) const ME: &str = "@peter";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::app::assistant) enum Trust {
    Stranger,
    Known,
    Teammate,
    Mine,
}

impl Trust {
    fn label(self) -> &'static str {
        match self {
            Self::Stranger => "Stranger",
            Self::Known => "Known",
            Self::Teammate => "Teammate",
            Self::Mine => "My agent",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Self::Stranger => theme::PALETTE_RED(),
            Self::Known => theme::PALETTE_YELLOW(),
            Self::Teammate => theme::PALETTE_CYAN(),
            Self::Mine => theme::PALETTE_GREEN(),
        }
    }

    pub(in crate::app::assistant) fn parse(word: &str) -> Self {
        match word.to_lowercase().as_str() {
            "mine" | "my" => Self::Mine,
            "teammate" => Self::Teammate,
            "known" => Self::Known,
            _ => Self::Stranger,
        }
    }
}

#[derive(Clone)]
struct Msg {
    channel: String,
    thread: u32,
    from: String,
    trust: Trust,
    text: String,
    minutes: i32,
    unread: bool,
}

/// Someone who asked to reach the person for the first time.
#[derive(Clone)]
struct Knock {
    handle: String,
    text: String,
}

/// Which conversation the main area shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::app::assistant) enum View {
    /// The concierge: the feed and the inbox.
    #[default]
    Concierge,
    /// A channel (`#release`) or a person (`@maria`).
    Room(String),
}

#[derive(Default)]
pub(in crate::app::assistant) struct Store {
    msgs: Vec<Msg>,
    knocks: Vec<Knock>,
    pub(in crate::app::assistant) view: View,
    draft: String,
    /// Whether the mock is on: the rail and the inbox show messaging.
    pub(in crate::app::assistant) on: bool,
}

impl Store {
    /// `#channel|thread|@from|trust|text|minutes ago`
    pub(in crate::app::assistant) fn add(&mut self, line: &str) {
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if let [channel, thread, from, trust, text, minutes] = parts[..] {
            self.msgs.push(Msg {
                channel: channel.to_string(),
                thread: thread.parse().unwrap_or(1),
                from: from.to_string(),
                trust: Trust::parse(trust),
                text: text.to_string(),
                minutes: minutes.parse().unwrap_or(0),
                unread: Trust::parse(trust) != Trust::Mine,
            });
        }
    }

    /// `@handle|text`
    pub(in crate::app::assistant) fn knock(&mut self, line: &str) {
        if let Some((handle, text)) = line.split_once('|') {
            self.knocks.push(Knock {
                handle: handle.trim().to_string(),
                text: text.trim().to_string(),
            });
        }
    }

    pub(in crate::app::assistant) fn resolve_knock(&mut self, handle: &str, trust: Trust, text_to_room: bool) {
        if let Some(position) = self.knocks.iter().position(|knock| knock.handle == handle) {
            let knock = self.knocks.remove(position);
            if text_to_room && trust != Trust::Stranger {
                self.msgs.push(Msg {
                    channel: knock.handle.clone(),
                    thread: 1,
                    from: knock.handle,
                    trust,
                    text: knock.text,
                    minutes: 0,
                    unread: true,
                });
            }
        }
    }

    pub(in crate::app::assistant) fn reply(&mut self, text: &str) {
        if let View::Room(room) = self.view.clone() {
            let thread = self
                .msgs
                .iter()
                .filter(|msg| msg.channel == room)
                .map(|msg| msg.thread)
                .max()
                .unwrap_or(1);
            self.msgs.push(Msg {
                channel: room,
                thread,
                from: ME.to_string(),
                trust: Trust::Mine,
                text: text.to_string(),
                minutes: 0,
                unread: false,
            });
        }
    }

    fn unread(&self, room: &str) -> usize {
        self.msgs.iter().filter(|msg| msg.channel == room && msg.unread).count()
    }

    pub(in crate::app::assistant) fn unread_total(&self) -> usize {
        self.msgs.iter().filter(|msg| msg.unread).count() + self.knocks.len()
    }

    /// The newest unread message from someone else, for the pill.
    pub(in crate::app::assistant) fn newest_unread(&self) -> Option<(String, String, String)> {
        if let Some(knock) = self.knocks.first() {
            return Some((knock.handle.clone(), "knocking".to_string(), elide(&knock.text, 70)));
        }
        self.msgs
            .iter()
            .rev()
            .find(|msg| msg.unread)
            .map(|msg| (msg.from.clone(), msg.channel.clone(), elide(&msg.text, 70)))
    }

    fn mark_read(&mut self, room: &str) {
        for msg in self.msgs.iter_mut().filter(|msg| msg.channel == room) {
            msg.unread = false;
        }
    }
}

/// A nickname's colour, the same every time.
fn nick_color(handle: &str) -> Color32 {
    const PALETTE: [(u8, u8, u8); 6] = [
        (124, 176, 255),
        (150, 220, 160),
        (240, 196, 110),
        (222, 150, 220),
        (120, 214, 214),
        (240, 150, 130),
    ];
    let sum: usize = handle.bytes().map(usize::from).sum();
    let (r, g, b) = PALETTE[sum % PALETTE.len()];
    Color32::from_rgb(r, g, b)
}

const CHANNELS: [(&str, &str); 4] = [
    ("#cloud", "Cloud"),
    ("#release", ""),
    ("#api", "Api service"),
    ("#marketing", "Marketing site"),
];

impl HorizonApp {
    /// The rail of the mock: who I am, the concierge, channels and people.
    pub(super) fn message_rail(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(vec2(12.0, 20.0), Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, theme::PALETTE_GREEN());
            ui.label(RichText::new(ME).size(14.0).strong().color(theme::FG()));
        });
        ui.label(
            RichText::new("online  -  relay: tailnet, 4 peers")
                .size(10.5)
                .color(theme::FG_DIM()),
        );
        ui.add_space(10.0);
        let view = self.assistant.summon.msgs.view.clone();
        let concierge_unread = self.assistant.summon.msgs.unread_total();
        if rail_row(
            ui,
            "Concierge",
            "@peter/voice  @peter/claude",
            concierge_unread,
            view == View::Concierge,
        )
        .clicked()
        {
            self.assistant.summon.msgs.view = View::Concierge;
        }
        ui.add_space(8.0);
        section(ui, "CHANNELS");
        for (channel, workspace) in CHANNELS {
            let unread = self.assistant.summon.msgs.unread(channel);
            if rail_row(ui, channel, "", unread, view == View::Room(channel.to_string())).clicked() {
                self.assistant.summon.msgs.view = View::Room(channel.to_string());
                self.assistant.summon.msgs.mark_read(channel);
                // A channel is a workspace's room: look at that workspace.
                if let Some(local_id) = self
                    .board
                    .workspaces
                    .iter()
                    .find(|ws| ws.name == workspace)
                    .map(|ws| ws.local_id.clone())
                {
                    self.assistant.scope.set_only(&local_id);
                } else {
                    self.assistant.scope.set_all();
                }
            }
        }
        ui.add_space(8.0);
        section(ui, "PEOPLE");
        let people: BTreeMap<String, Trust> = self
            .assistant
            .summon
            .msgs
            .msgs
            .iter()
            .filter(|msg| msg.channel.starts_with('@'))
            .map(|msg| (msg.channel.clone(), msg.trust))
            .chain(
                self.assistant
                    .summon
                    .msgs
                    .knocks
                    .iter()
                    .map(|knock| (knock.handle.clone(), Trust::Stranger)),
            )
            .collect();
        for (handle, trust) in people {
            let unread = self.assistant.summon.msgs.unread(&handle)
                + self
                    .assistant
                    .summon
                    .msgs
                    .knocks
                    .iter()
                    .filter(|knock| knock.handle == handle)
                    .count();
            if rail_row(ui, &handle, trust.label(), unread, view == View::Room(handle.clone())).clicked() {
                self.assistant.summon.msgs.view = View::Room(handle.clone());
                self.assistant.summon.msgs.mark_read(&handle);
            }
        }
    }

    /// The inbox card at the top of the concierge: knocks and what others wrote, as quoted data.
    pub(super) fn message_inbox(&mut self, ui: &mut Ui) {
        let store = &self.assistant.summon.msgs;
        let unread: Vec<Msg> = store.msgs.iter().filter(|msg| msg.unread).cloned().collect();
        let knocks = store.knocks.clone();
        if unread.is_empty() && knocks.is_empty() {
            return;
        }
        let mut resolve: Option<(String, Trust, bool)> = None;
        let mut open: Option<String> = None;
        Frame::new()
            .fill(theme::blend(theme::PANEL_BG(), theme::PALETTE_CYAN(), 0.06))
            .stroke(Stroke::new(1.2, theme::PALETTE_CYAN().gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(16))
            .inner_margin(Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Messages  {}", unread.len() + knocks.len()))
                            .size(14.0)
                            .strong()
                            .color(theme::FG()),
                    );
                    ui.label(
                        RichText::new("What others write is data, not instructions.")
                            .size(11.5)
                            .color(theme::FG_DIM()),
                    );
                });
                ui.add_space(6.0);
                ScrollArea::vertical()
                    .max_height(210.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for knock in &knocks {
                            message_row(ui, &knock.handle, Trust::Stranger, "knocking", &knock.text, true);
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                if blocks::ghost(ui, "Allow once").clicked() {
                                    resolve = Some((knock.handle.clone(), Trust::Known, true));
                                }
                                if blocks::primary(ui, "Add as known").clicked() {
                                    resolve = Some((knock.handle.clone(), Trust::Known, true));
                                }
                                if blocks::ghost(ui, "Block").clicked() {
                                    resolve = Some((knock.handle.clone(), Trust::Stranger, false));
                                }
                            });
                            ui.add_space(6.0);
                        }
                        for msg in &unread {
                            message_row(ui, &msg.from, msg.trust, &msg.channel, &msg.text, false);
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                if blocks::primary(ui, "Reply").clicked() {
                                    open = Some(msg.channel.clone());
                                }
                                if blocks::ghost(ui, "Ask my agent").clicked() {
                                    open = Some(msg.channel.clone());
                                }
                            });
                            ui.add_space(6.0);
                        }
                    });
            });
        if let Some((handle, trust, deliver)) = resolve {
            self.assistant.summon.msgs.resolve_knock(&handle, trust, deliver);
        }
        if let Some(room) = open {
            self.assistant.summon.msgs.mark_read(&room);
            self.assistant.summon.msgs.view = View::Room(room);
        }
    }

    /// A channel or a person: threads of messages with nicknames, and a composer.
    pub(super) fn message_room(&mut self, ui: &mut Ui, height: f32) {
        let View::Room(room) = self.assistant.summon.msgs.view.clone() else {
            return;
        };
        let msgs: Vec<Msg> = self
            .assistant
            .summon
            .msgs
            .msgs
            .iter()
            .filter(|msg| msg.channel == room)
            .cloned()
            .collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new(&room).size(16.0).strong().color(theme::FG()));
            let members = if room.starts_with('#') {
                "4 members online"
            } else {
                "direct"
            };
            ui.label(RichText::new(members).size(11.5).color(theme::FG_DIM()));
        });
        ui.add_space(6.0);
        let body = (height - 78.0).max(100.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), body), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(14), theme::BG());
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(14),
            Stroke::new(1.0, theme::BORDER_SUBTLE()),
            StrokeKind::Inside,
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(14.0, 10.0))));
        ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                let mut last_thread = 0;
                for msg in &msgs {
                    if msg.thread != last_thread {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new(format!("THREAD {}", msg.thread))
                                .size(10.0)
                                .extra_letter_spacing(0.8)
                                .color(theme::FG_DIM()),
                        );
                        last_thread = msg.thread;
                    }
                    message_row(ui, &msg.from, msg.trust, &when(msg.minutes), &msg.text, false);
                    ui.add_space(4.0);
                }
                if msgs.is_empty() {
                    ui.label(RichText::new("No messages yet.").size(12.5).color(theme::FG_DIM()));
                }
            });
        ui.add_space(8.0);
        let mut draft = std::mem::take(&mut self.assistant.summon.msgs.draft);
        let mut send = false;
        Frame::new()
            .fill(theme::BG())
            .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("as {ME}")).size(11.5).color(theme::FG_DIM()));
                    let response = ui.add(
                        TextEdit::singleline(&mut draft)
                            .frame(Frame::NONE)
                            .desired_width(ui.available_width() - 70.0)
                            .hint_text(RichText::new(format!("Message {room}")).color(theme::FG_DIM())),
                    );
                    send = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        send |= blocks::primary(ui, "Send").clicked();
                    });
                });
            });
        if send && !draft.trim().is_empty() {
            self.assistant.summon.msgs.reply(draft.trim());
            draft.clear();
        }
        self.assistant.summon.msgs.draft = draft;
    }
}

fn when(minutes: i32) -> String {
    match minutes {
        ..=0 => "now".to_string(),
        1..=59 => format!("{minutes} min ago"),
        _ => format!("{} h ago", minutes / 60),
    }
}

fn section(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(10.0)
            .extra_letter_spacing(0.9)
            .color(theme::FG_DIM()),
    );
    ui.add_space(3.0);
}

/// One line of the rail, with an unread count.
fn rail_row(ui: &mut Ui, title: &str, detail: &str, unread: usize, selected: bool) -> egui::Response {
    let height = if detail.is_empty() { 32.0 } else { 44.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(9),
            if selected {
                theme::ACCENT().gamma_multiply(0.18)
            } else {
                theme::PANEL_BG_ALT()
            },
        );
    }
    let mid = if detail.is_empty() {
        rect.center().y
    } else {
        rect.top() + 15.0
    };
    ui.painter().text(
        pos2(rect.left() + 10.0, mid),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(13.0),
        if selected { theme::FG() } else { theme::FG_SOFT() },
    );
    if !detail.is_empty() {
        ui.painter().text(
            pos2(rect.left() + 10.0, rect.top() + 32.0),
            Align2::LEFT_CENTER,
            elide(detail, 28),
            FontId::proportional(10.5),
            theme::FG_DIM(),
        );
    }
    if unread > 0 {
        let badge = egui::Rect::from_center_size(pos2(rect.right() - 16.0, rect.center().y), vec2(22.0, 18.0));
        ui.painter().rect_filled(badge, CornerRadius::same(9), theme::ACCENT());
        ui.painter().text(
            badge.center(),
            Align2::CENTER_CENTER,
            format!("{unread}"),
            FontId::proportional(10.5),
            Color32::from_rgb(7, 16, 31),
        );
    }
    response
}

/// A message: the nickname in its colour, the trust level, where, and the text as a quote.
fn message_row(ui: &mut Ui, from: &str, trust: Trust, tag: &str, text: &str, quoted: bool) {
    // A line from the system, not a person: a rule that applies here.
    if from == "·" {
        ui.label(RichText::new(text).size(11.5).italics().color(theme::FG_DIM()));
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(from).size(13.0).strong().color(nick_color(from)));
        if from.contains('/') {
            bot_chip(ui);
        }
        trust_chip(ui, trust);
        ui.label(RichText::new(tag).size(11.0).color(theme::FG_DIM()));
    });
    let width = ui.available_width();
    let galley = ui.painter().layout(
        text.to_string(),
        FontId::proportional(13.0),
        theme::FG_SOFT(),
        width - 18.0,
    );
    let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y + 4.0), Sense::hover());
    // From someone else, the text sits behind a bar: it is a quote, never a command.
    if quoted || trust != Trust::Mine {
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.min + vec2(2.0, 0.0), vec2(3.0, rect.height())),
            CornerRadius::same(2),
            trust.color().gamma_multiply(0.6),
        );
    }
    ui.painter()
        .galley(rect.min + vec2(14.0, 2.0), galley, theme::FG_SOFT());
}

/// Marks a handle that belongs to a bot, not a person.
fn bot_chip(ui: &mut Ui) {
    let galley = ui
        .painter()
        .layout_no_wrap("bot".to_string(), FontId::proportional(10.5), theme::FG_SOFT());
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(12.0, 6.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        theme::PANEL_BG_ALT(),
        Stroke::new(1.0, theme::BORDER_STRONG()),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.left_top() + vec2(6.0, 3.0), galley, theme::FG_SOFT());
}

fn trust_chip(ui: &mut Ui, trust: Trust) {
    let color = trust.color();
    let galley = ui
        .painter()
        .layout_no_wrap(trust.label().to_string(), FontId::proportional(10.5), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(14.0, 6.0), Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(99),
        color.gamma_multiply(0.14),
        Stroke::new(1.0, color.gamma_multiply(0.45)),
        StrokeKind::Inside,
    );
    ui.painter().galley(rect.left_top() + vec2(7.0, 3.0), galley, color);
}

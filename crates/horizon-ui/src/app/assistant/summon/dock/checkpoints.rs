//! A mock of approvals by checkpoint (prototype: fixture data, nothing is enforced).
//!
//! Agents run on their own and stop only at checkpoints the person agreed on: ready to push, ready to
//! merge, publish outward, spend, credentials, anything destructive. Each is one card with its evidence on
//! it; several agents waiting at the same kind of checkpoint share one card. A standing approval lets a
//! kind pass by itself while its evidence is clean, and what passed is reported afterwards in a digest.

use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Id, Margin, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2,
};

use super::super::super::num;
use super::super::HorizonApp;
use super::super::desk_bar::paint::elide;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app::assistant) enum Kind {
    Push,
    Pr,
    Merge,
    Publish,
    Cloud,
    Credentials,
    Destructive,
}

impl Kind {
    pub(in crate::app::assistant) fn parse(word: &str) -> Self {
        match word.to_lowercase().as_str() {
            "push" => Self::Push,
            "pr" => Self::Pr,
            "publish" => Self::Publish,
            "cloud" => Self::Cloud,
            "credentials" | "secret" => Self::Credentials,
            "destructive" | "delete" => Self::Destructive,
            _ => Self::Merge,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Push => "Ready to push",
            Self::Pr => "Ready to open a PR",
            Self::Merge => "Ready to merge",
            Self::Publish => "Publish outward",
            Self::Cloud => "Spend or change cloud",
            Self::Credentials => "Use a credential",
            Self::Destructive => "Destructive action",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Self::Push => "branches",
            Self::Pr | Self::Merge => "PRs",
            Self::Publish => "uploads",
            Self::Cloud => "machines",
            Self::Credentials => "secrets",
            Self::Destructive => "actions",
        }
    }

    /// How much care it asks for: lower is more careful and comes first.
    fn rank(self) -> u8 {
        match self {
            Self::Destructive => 0,
            Self::Credentials => 1,
            Self::Cloud => 2,
            Self::Publish => 3,
            Self::Pr => 4,
            Self::Push => 5,
            Self::Merge => 6,
        }
    }

    fn color(self) -> Color32 {
        match self.rank() {
            0..=3 => theme::PALETTE_RED(),
            4 | 5 => theme::PALETTE_YELLOW(),
            _ => theme::PALETTE_GREEN(),
        }
    }
}

#[derive(Clone)]
struct Item {
    agent: String,
    subject: String,
}

#[derive(Clone)]
struct Row {
    kind: Kind,
    items: Vec<Item>,
    /// What was checked, and whether it is fine.
    evidence: Vec<(bool, String)>,
}

impl Row {
    fn rank(&self) -> u8 {
        self.kind.rank()
    }

    fn heading(&self) -> String {
        if self.items.len() > 1 {
            format!(
                "{}  \u{b7}  {} {}",
                self.kind.title(),
                self.items.len(),
                self.kind.unit()
            )
        } else {
            self.kind.title().to_string()
        }
    }

    fn who(&self) -> String {
        match self.items.as_slice() {
            [only] => format!("{}  \u{b7}  {}", only.agent, only.subject),
            items => {
                let names: Vec<&str> = items.iter().take(3).map(|item| item.agent.as_str()).collect();
                let more = items.len().saturating_sub(3);
                if more > 0 {
                    format!("{}, +{more} more", names.join(", "))
                } else {
                    names.join(", ")
                }
            }
        }
    }

    fn clean(&self) -> bool {
        self.evidence.iter().all(|(ok, _)| *ok)
    }
}

#[derive(Default)]
pub(in crate::app::assistant) struct Fleet {
    pub(in crate::app::assistant) total: u32,
    pub(in crate::app::assistant) working: u32,
    pub(in crate::app::assistant) parked: u32,
}

#[derive(Default)]
pub(in crate::app::assistant) struct Store {
    rows: Vec<Row>,
    /// What passed or was decided, with how many times, for the digest.
    log: Vec<(String, u32)>,
    /// Merge by itself while every check is green.
    pub(in crate::app::assistant) standing_merge: bool,
    pub(in crate::app::assistant) fleet: Option<Fleet>,
}

#[derive(Clone, Copy)]
pub(super) enum Press {
    Approve(usize),
    Hold(usize),
}

impl Store {
    /// `kind|agent|subject|check;check;!problem`. A `!` in front marks a check that is not fine.
    pub(in crate::app::assistant) fn add(&mut self, spec: &str) {
        let parts: Vec<&str> = spec.split('|').map(str::trim).collect();
        let [kind, agent, subject, checks] = parts[..] else {
            return;
        };
        let kind = Kind::parse(kind);
        let evidence: Vec<(bool, String)> = checks
            .split(';')
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| match text.strip_prefix('!') {
                Some(problem) => (false, problem.trim().to_string()),
                None => (true, text.to_string()),
            })
            .collect();
        let item = Item {
            agent: agent.to_string(),
            subject: subject.to_string(),
        };
        // A merge passes by itself under a standing approval, as long as every check is green.
        if kind == Kind::Merge && self.standing_merge && evidence.iter().all(|(ok, _)| *ok) {
            self.note("Merged {n} PR{s} under your standing rule".to_string());
            return;
        }
        match self.rows.iter_mut().find(|row| row.kind == kind) {
            Some(row) => {
                row.items.push(item);
                for entry in evidence {
                    if !entry.0 && !row.evidence.contains(&entry) {
                        row.evidence.push(entry);
                    }
                }
            }
            None => self.rows.push(Row {
                kind,
                items: vec![item],
                evidence,
            }),
        }
        self.rows.sort_by_key(Row::rank);
    }

    fn note(&mut self, text: String) {
        match self.log.iter_mut().find(|(line, _)| *line == text) {
            Some((_, count)) => *count += 1,
            None => self.log.push((text, 1)),
        }
    }

    pub(in crate::app::assistant) fn has(&self) -> bool {
        !self.rows.is_empty()
    }

    pub(in crate::app::assistant) fn digest_is_empty(&self) -> bool {
        self.log.is_empty()
    }

    fn top(&self) -> Option<&Row> {
        self.rows.first()
    }

    fn waiting(&self) -> usize {
        self.rows.iter().map(|row| row.items.len()).sum()
    }

    /// Approves the card at `index`, all of its agents at once.
    pub(in crate::app::assistant) fn approve(&mut self, index: usize) {
        if index < self.rows.len() {
            let row = self.rows.remove(index);
            self.note(format!("Approved {}: {}", row.kind.title().to_lowercase(), row.who()));
        }
    }

    pub(in crate::app::assistant) fn hold(&mut self, index: usize) {
        if index < self.rows.len() {
            let row = self.rows.remove(index);
            self.note(format!("Held {}: {}", row.kind.title().to_lowercase(), row.who()));
        }
    }

    pub(in crate::app::assistant) fn clear(&mut self) {
        self.rows.clear();
        self.log.clear();
    }

    /// The lines of the digest, with the counts filled in.
    fn digest(&self) -> Vec<String> {
        self.log
            .iter()
            .map(|(text, count)| {
                if text.contains("{n}") {
                    text.replace("{n}", &count.to_string())
                        .replace("{s}", if *count == 1 { "" } else { "s" })
                } else if *count > 1 {
                    format!("{text}  (x{count})")
                } else {
                    text.clone()
                }
            })
            .collect()
    }
}

impl HorizonApp {
    /// What the pill shows for the most careful checkpoint: who, what was checked, and Approve or Hold.
    /// Returns a press, if any.
    pub(super) fn paint_top_checkpoint(&self, ui: &mut Ui, pill: Rect, (left, right): (f32, f32)) -> Option<Press> {
        let store = &self.assistant.summon.ckpt;
        let top = store.top()?;
        let mut pressed = None;
        let mid = pill.center().y;
        let color = top.kind.color();
        let approve = Rect::from_center_size(
            pos2(right - 128.0, mid),
            vec2(if top.items.len() > 1 { 104.0 } else { 74.0 }, 30.0),
        );
        let hold = Rect::from_center_size(pos2(right - 38.0, mid), vec2(62.0, 30.0));
        let approve = Rect::from_center_size(pos2(hold.left() - 10.0 - approve.width() / 2.0, mid), approve.size());
        let more = store.waiting().saturating_sub(top.items.len());
        let mut x = left;
        x = kind_chip(ui, pos2(x, mid), top.kind.title(), color) + 10.0;
        let room = ((approve.left() - if more > 0 { 56.0 } else { 0.0 }) - x - 8.0).max(60.0);
        ui.painter().text(
            pos2(x, mid - 9.0),
            Align2::LEFT_CENTER,
            elide(&top.who(), num::index(room / 6.6)),
            FontId::proportional(11.5),
            theme::FG_DIM(),
        );
        let checks: Vec<String> = top
            .evidence
            .iter()
            .map(|(ok, text)| format!("{} {text}", if *ok { "\u{2713}" } else { "!" }))
            .collect();
        ui.painter().text(
            pos2(x, mid + 9.0),
            Align2::LEFT_CENTER,
            elide(&checks.join("   "), num::index(room / 7.0)),
            FontId::proportional(13.0),
            if top.clean() { theme::FG() } else { theme::PALETTE_RED() },
        );
        if more > 0 {
            super::concierge::count_chip(ui, pos2(approve.left() - 10.0, mid), &format!("+{more}"));
        }
        let label = if top.items.len() > 1 { "Approve all" } else { "Approve" };
        if button(ui, approve, label, true, 0).clicked() {
            pressed = Some(Press::Approve(0));
        }
        if button(ui, hold, "Hold", false, 1).clicked() {
            pressed = Some(Press::Hold(0));
        }
        ui.ctx().request_repaint();
        pressed
    }

    /// The checkpoint card of the sheet and, under it, what passed on its own.
    pub(super) fn checkpoint_card(&mut self, ui: &mut Ui) -> Option<Press> {
        let store = &self.assistant.summon.ckpt;
        let mut pressed = None;
        if store.has() {
            Frame::new()
                .fill(theme::blend(theme::PANEL_BG(), theme::PALETTE_RED(), 0.06))
                .stroke(Stroke::new(1.2, theme::PALETTE_RED().gamma_multiply(0.45)))
                .corner_radius(CornerRadius::same(16))
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} checkpoint{} for you",
                                store.rows.len(),
                                if store.rows.len() == 1 { "" } else { "s" }
                            ))
                            .size(14.0)
                            .strong()
                            .color(theme::FG()),
                        );
                        ui.label(
                            RichText::new("Agents work on their own and stop only here")
                                .size(11.5)
                                .color(theme::FG_DIM()),
                        );
                    });
                    ui.add_space(6.0);
                    for (index, row) in store.rows.iter().enumerate() {
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 62.0), Sense::hover());
                        let mid = rect.center().y;
                        let after =
                            kind_chip(ui, pos2(rect.left(), mid - 14.0), &row.heading(), row.kind.color()) + 10.0;
                        ui.painter().text(
                            pos2(after, mid - 14.0),
                            Align2::LEFT_CENTER,
                            elide(&row.who(), num::index((rect.right() - after - 190.0) / 6.6)),
                            FontId::proportional(11.5),
                            theme::FG_DIM(),
                        );
                        let mut cx = rect.left();
                        for (ok, text) in &row.evidence {
                            let line = format!("{} {text}", if *ok { "\u{2713}" } else { "!" });
                            let galley = ui.painter().layout_no_wrap(
                                line,
                                FontId::proportional(12.5),
                                if *ok {
                                    theme::PALETTE_GREEN()
                                } else {
                                    theme::PALETTE_RED()
                                },
                            );
                            let width = galley.size().x;
                            if cx + width > rect.right() - 190.0 {
                                break;
                            }
                            ui.painter()
                                .galley(pos2(cx, mid + 10.0 - galley.size().y / 2.0), galley, Color32::WHITE);
                            cx += width + 16.0;
                        }
                        let approve_width = if row.items.len() > 1 { 104.0 } else { 74.0 };
                        let hold = Rect::from_center_size(pos2(rect.right() - 31.0, mid), vec2(58.0, 28.0));
                        let approve = Rect::from_center_size(
                            pos2(hold.left() - 8.0 - approve_width / 2.0, mid),
                            vec2(approve_width, 28.0),
                        );
                        let label = if row.items.len() > 1 { "Approve all" } else { "Approve" };
                        if button(ui, approve, label, true, 10 + index as u64).clicked() {
                            pressed = Some(Press::Approve(index));
                        }
                        if button(ui, hold, "Hold", false, 100 + index as u64).clicked() {
                            pressed = Some(Press::Hold(index));
                        }
                    }
                });
        }
        digest_card(ui, &store.digest());
        pressed
    }

    pub(super) fn apply_checkpoint_press(&mut self, press: Option<Press>) {
        match press {
            Some(Press::Approve(index)) => self.assistant.summon.ckpt.approve(index),
            Some(Press::Hold(index)) => self.assistant.summon.ckpt.hold(index),
            None => {}
        }
    }

    /// The header line of the sheet when a fleet is being run: how many agents, how many need the person.
    pub(super) fn fleet_status(&self) -> Option<(String, Color32)> {
        let fleet = self.assistant.summon.ckpt.fleet.as_ref()?;
        let need = self.assistant.summon.ckpt.waiting();
        let text = format!(
            "{} agents  \u{b7}  {} working  \u{b7}  {} parked  \u{b7}  {} {} you",
            fleet.total,
            fleet.working,
            fleet.parked,
            need,
            if need == 1 { "needs" } else { "need" }
        );
        Some((
            text,
            if need > 0 {
                theme::PALETTE_YELLOW()
            } else {
                theme::PALETTE_GREEN()
            },
        ))
    }
}

/// What passed on its own or was decided, newest last, as a quiet card.
fn digest_card(ui: &mut Ui, digest: &[String]) {
    if digest.is_empty() {
        return;
    }
    ui.add_space(8.0);
    Frame::new()
        .fill(theme::PANEL_BG())
        .stroke(Stroke::new(1.0, theme::BORDER_SUBTLE()))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("WHILE YOU WERE BUSY")
                    .size(10.0)
                    .extra_letter_spacing(0.9)
                    .color(theme::FG_DIM()),
            );
            for line in digest.iter().rev().take(3) {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("\u{2713}").size(12.0).color(theme::PALETTE_GREEN()));
                    ui.label(RichText::new(elide(line, 110)).size(12.5).color(theme::FG_SOFT()));
                });
            }
        });
}

/// A small coloured pill with text; returns the right edge.
fn kind_chip(ui: &Ui, left_centre: Pos2, text: &str, color: Color32) -> f32 {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(11.5), color);
    let size = vec2(galley.size().x + 18.0, 22.0);
    let rect = Rect::from_min_size(pos2(left_centre.x, left_centre.y - size.y / 2.0), size);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), color.gamma_multiply(0.16));
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(11),
        Stroke::new(1.0, color.gamma_multiply(0.7)),
        egui::StrokeKind::Inside,
    );
    ui.painter().galley(
        pos2(rect.left() + 9.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    rect.right()
}

fn button(ui: &mut Ui, rect: Rect, text: &str, primary: bool, key: u64) -> egui::Response {
    let response = ui.interact(rect, Id::new(("checkpoint_button", text, key)), Sense::click());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(14),
        if primary {
            theme::ACCENT()
        } else if response.hovered() {
            theme::BORDER_SUBTLE()
        } else {
            theme::PANEL_BG_ALT()
        },
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(12.5),
        if primary {
            Color32::from_rgb(7, 16, 31)
        } else {
            theme::FG_SOFT()
        },
    );
    response
}

#[cfg(test)]
mod tests {
    use super::Store;

    #[test]
    fn the_same_kind_shares_one_card_and_the_most_careful_comes_first() {
        let mut store = Store::default();
        store.add("merge|a1|PR 1|CI green");
        store.add("merge|a2|PR 2|CI green");
        store.add("cloud|infra|Stop hetzner-cx42|Idle 12 hours");
        assert_eq!(store.rows.len(), 2);
        assert_eq!(store.top().map(|row| row.kind.title()), Some("Spend or change cloud"));
        assert_eq!(store.waiting(), 3);
    }

    #[test]
    fn a_standing_merge_passes_only_while_every_check_is_green() {
        let mut store = Store {
            standing_merge: true,
            ..Store::default()
        };
        store.add("merge|a1|PR 1|CI green;Review clean");
        store.add("merge|a2|PR 2|CI green;!Review has an open thread");
        assert_eq!(store.rows.len(), 1);
        assert_eq!(store.digest(), ["Merged 1 PR under your standing rule"]);
        store.approve(0);
        assert!(!store.has());
    }
}

//! The row of a proposal of the front desk, and the two answers of the person.
//!
//! The front desk proposes work items; the person decides. The row draws the
//! title, the first lines of the task and the folder of each item, and the two
//! controls that start the proposal or leave it. Nothing starts before the press:
//! the row is the whole right of the person over the words of the program.
//!
//! Every text of an item is text of a program, so it enters this module cut: the
//! title to one line, the task to two lines and the folder to the width of its
//! line with its end showing.
//!
//! The folder of an item is checked with `is_dir` only after the press, and only
//! on the worker of the dock ([`Starting`]): no frame reads a directory.

use egui::{Color32, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use horizon_core::agent_bridge::{ProposalItem, ProposalState};

use crate::theme;
use crate::typeface::Weight;

use super::list::CARD_INDENT;
use super::text;

/// The caption above the items of a proposal.
pub(super) const CAPTION: &str = "The front desk wants to start";
/// The control that starts every item of a proposal.
pub(super) const START_ONE: &str = "Start 1 work item";
/// The control that leaves the proposal without starting it.
pub(super) const DECLINE: &str = "Not now";
/// The line of a proposal that the person started.
pub(super) const STARTED: &str = "Started";
/// The line of a proposal that the person did not start.
pub(super) const NOT_STARTED: &str = "Not started";
/// The line of a proposal that a newer proposal replaced while it was open.
pub(super) const REPLACED: &str = "Replaced by a newer proposal";
/// The notice of the conversation of the front desk for a press that started work
/// items.
pub(super) const STARTED_NOTICE: &str = "You started";
/// The notice of the conversation of the front desk for a proposal that the
/// person left.
pub(super) const NOT_STARTED_NOTICE: &str = NOT_STARTED;
/// The line that the program of the front desk reads when the person starts the
/// work items of a proposal.
pub(super) const STARTED_LINE: &str = "The person started the work items of proposal";
/// The line that the program of the front desk reads when the person leaves a
/// proposal.
pub(super) const NOT_STARTED_LINE: &str = "The person did not start the work items of proposal";
/// The notice of an item whose folder the check did not find.
pub(super) const FOLDER_MISSING: &str = "the folder was not found, so it starts with none";

const RADIUS: u8 = 12;
pub(super) const PADDING: f32 = 12.0;
/// The alpha of the fill of a row of a proposal.
const FILL_ALPHA: u8 = 18;
/// The alpha of its outline.
const OUTLINE_ALPHA: u8 = 115;
const CAPTION_SIZE: f32 = 11.0;
const TITLE_SIZE: f32 = 12.5;
const TASK_SIZE: f32 = 11.5;
const FOLDER_SIZE: f32 = 10.5;
/// The air between two items, and under the caption.
const ITEM_GAP: f32 = 8.0;
/// The air between the last item and the controls, and between the two controls.
const CONTROLS_GAP: f32 = 10.0;
pub(super) const BUTTON_GAP: f32 = 8.0;
pub(super) const BUTTON_HEIGHT: f32 = 28.0;
const BUTTON_RADIUS: u8 = 8;
pub(super) const BUTTON_PADDING: f32 = 12.0;
const BUTTON_SIZE: f32 = 12.0;
/// The rows of the task that the row draws.
const TASK_ROWS: usize = 2;
const TASK_LINE_HEIGHT: f32 = 1.45;
/// The metrics of the task of one item: 11.5 Regular, two rows high, in the colour
/// of a soft line of the dock.
fn task_metrics() -> text::Metrics {
    text::Metrics {
        size: TASK_SIZE,
        weight: Weight::Regular,
        color: theme::FG_SOFT(),
        line_height: TASK_LINE_HEIGHT,
    }
}
const STATE_SIZE: f32 = 11.5;

/// What the person pressed on the row of a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Choice {
    /// Start every item of the proposal.
    Start,
    /// Start nothing.
    Decline,
}

/// What the dock holds while the worker checks the folders of a proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Starting {
    /// The check of this proposal runs on the worker of the dock.
    Checking(String),
    /// The worker answered: true for an item whose folder is a directory of this
    /// computer, false for one whose folder it did not find.
    Ready { id: String, found: Vec<bool> },
}

/// The label of the control that starts `items` work items.
pub(super) fn start_label(items: usize) -> String {
    if items == 1 {
        START_ONE.to_owned()
    } else {
        format!("Start {items} work items")
    }
}

/// The line under the items of a proposal that is no longer open.
pub(super) fn state_line(state: ProposalState) -> Option<&'static str> {
    match state {
        ProposalState::Open => None,
        ProposalState::Started => Some(STARTED),
        ProposalState::Declined => Some(NOT_STARTED),
        ProposalState::Replaced => Some(REPLACED),
    }
}

/// Checks the folder of each item, as the worker of the dock does after the press:
/// true for a folder that is a directory of this computer. An item without a
/// folder answers false and no path is read for it.
pub(super) fn check_folders(items: &[ProposalItem]) -> Vec<bool> {
    items
        .iter()
        .map(|item| {
            item.folder
                .as_deref()
                .is_some_and(|folder| std::path::Path::new(folder).is_dir())
        })
        .collect()
}

/// The notice of a press that started `count` work items.
pub(super) fn started_notice(count: usize) -> String {
    if count == 1 {
        format!("{STARTED_NOTICE} 1 work item")
    } else {
        format!("{STARTED_NOTICE} {count} work items")
    }
}

/// The line of the proposal `id` that the program of the front desk reads after a
/// press.
pub(super) fn bridge_line(id: &str, choice: Choice) -> String {
    let head = match choice {
        Choice::Start => STARTED_LINE,
        Choice::Decline => NOT_STARTED_LINE,
    };
    format!("{head} {id}.")
}

/// The galleys of one item of a proposal: its title, the first rows of its task and,
/// when the item names a folder, the end of that path.
struct ItemGalleys {
    title: std::sync::Arc<egui::Galley>,
    task: std::sync::Arc<egui::Galley>,
    folder: Option<std::sync::Arc<egui::Galley>>,
}

impl ItemGalleys {
    fn new(ui: &Ui, item: &ProposalItem, width: f32) -> Self {
        Self {
            title: text::line(ui, &item.title, TITLE_SIZE, Weight::Semibold, theme::FG(), width),
            task: text::rows(ui, &item.task, task_metrics(), width, TASK_ROWS),
            folder: item.folder.as_deref().map(|folder| folder_line(ui, folder, width)),
        }
    }

    /// The height of the item in a row.
    fn height(&self) -> f32 {
        let folder = self.folder.as_ref().map_or(0.0, |folder| folder.size().y);
        self.title.size().y + self.task.size().y + folder + ITEM_GAP
    }
}

/// Draws the row of one proposal in `width` points, and answers the control that
/// the person pressed. A proposal that is no longer open draws its state and takes
/// no press.
pub(super) fn render(
    ui: &mut Ui,
    id: &str,
    items: &[ProposalItem],
    state: ProposalState,
    width: f32,
    loaded_open: bool,
) -> Option<Choice> {
    let row_width = (width - CARD_INDENT).max(1.0);
    let inner = (row_width - 2.0 * PADDING).max(1.0);
    let caption = text::line(ui, CAPTION, CAPTION_SIZE, Weight::Regular, theme::FG_DIM(), inner);
    let items: Vec<ItemGalleys> = items.iter().map(|item| ItemGalleys::new(ui, item, inner)).collect();
    let controls: Vec<_> = [start_label(items.len()), DECLINE.to_owned()]
        .iter()
        .map(|label| {
            text::line(
                ui,
                label,
                BUTTON_SIZE,
                Weight::Semibold,
                theme::FG(),
                inner - 2.0 * BUTTON_PADDING,
            )
        })
        .collect();
    // A press on a proposal that a restart loaded as open starts nothing: the
    // program that proposed it is gone, so the row reads `Not started` and takes no
    // press. A proposal that the person already decided reads its own line.
    let line = match (state, loaded_open) {
        (ProposalState::Open, true) => Some(NOT_STARTED),
        (state, _) => state_line(state),
    };
    let state_line = line.map(|line| text::line(ui, line, STATE_SIZE, Weight::Regular, theme::FG_DIM(), inner));
    let height = row_height(&caption, &items, state_line.as_ref());
    let (row, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let rect = Rect::from_min_size(pos2(row.min.x + CARD_INDENT, row.min.y), vec2(row_width, height));
    draw_box(ui.painter(), rect);
    draw_text(ui.painter(), rect, &caption, &items, state_line.as_ref());
    if state_line.is_some() {
        return None;
    }
    draw_controls(ui, rect, id, &controls)
}

/// The box of one proposal: the accent fill and the accent outline of a row that
/// the person can decide.
fn draw_box(painter: &egui::Painter, rect: Rect) {
    painter.rect(
        rect,
        RADIUS,
        theme::alpha(theme::ACCENT(), FILL_ALPHA),
        Stroke::new(1.0_f32, theme::alpha(theme::ACCENT(), OUTLINE_ALPHA)),
        StrokeKind::Inside,
    );
}

/// The caption, the items and, when the proposal is decided, the one line of its
/// state at the bottom padding.
fn draw_text(
    painter: &egui::Painter,
    rect: Rect,
    caption: &std::sync::Arc<egui::Galley>,
    items: &[ItemGalleys],
    state_line: Option<&std::sync::Arc<egui::Galley>>,
) {
    let left = rect.min.x + PADDING;
    let mut at = rect.min.y + PADDING;
    let caption_height = caption.size().y;
    painter.galley(pos2(left, at), caption.clone(), theme::FG_DIM());
    at += caption_height + ITEM_GAP;
    for item in items {
        painter.galley(pos2(left, at), item.title.clone(), theme::FG());
        at += item.title.size().y;
        painter.galley(pos2(left, at), item.task.clone(), theme::FG_SOFT());
        at += item.task.size().y;
        if let Some(folder) = &item.folder {
            painter.galley(pos2(left, at), folder.clone(), theme::FG_DIM());
            at += folder.size().y;
        }
        at += ITEM_GAP;
    }
    if let Some(line) = state_line {
        let top = rect.max.y - PADDING - line.size().y;
        painter.galley(pos2(left, top), line.clone(), theme::FG_DIM());
    }
}

/// The two controls of a proposal that the person did not decide, and the control
/// that the person pressed.
fn draw_controls(ui: &mut Ui, rect: Rect, id: &str, controls: &[std::sync::Arc<egui::Galley>]) -> Option<Choice> {
    let mut pressed = None;
    let mut x = rect.min.x + PADDING;
    let top = rect.max.y - PADDING - BUTTON_HEIGHT;
    for (index, galley) in controls.iter().enumerate() {
        let area = Rect::from_min_size(
            pos2(x, top),
            vec2(galley.size().x + 2.0 * BUTTON_PADDING, BUTTON_HEIGHT),
        );
        let response = ui.interact(
            area,
            ui.make_persistent_id(("orchestrator_dock_proposal", id, index)),
            Sense::click(),
        );
        let hovered = response.hovered();
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let (fill, outline, color) = button_colors(index, hovered);
        let painter = ui.painter();
        painter.rect(
            area,
            BUTTON_RADIUS,
            fill,
            Stroke::new(1.0_f32, outline),
            StrokeKind::Inside,
        );
        painter.galley(
            pos2(
                area.center().x - galley.size().x / 2.0,
                area.center().y - galley.size().y / 2.0,
            ),
            galley.clone(),
            color,
        );
        if response.clicked() {
            pressed = Some(if index == 0 { Choice::Start } else { Choice::Decline });
        }
        x = area.max.x + BUTTON_GAP;
    }
    pressed
}

/// The fill, the outline and the label colour of the control at `index`: the
/// control that starts the proposal carries the accent, and the other one the
/// plain panel.
fn button_colors(index: usize, hovered: bool) -> (Color32, Color32, Color32) {
    let index_0 = index == 0;
    if index_0 {
        let fill = if hovered {
            theme::alpha(theme::ACCENT(), 220)
        } else {
            theme::ACCENT()
        };
        (fill, theme::ACCENT(), theme::BG())
    } else {
        let fill = theme::PANEL_BG_ALT();
        let outline = if hovered {
            theme::FG_DIM()
        } else {
            theme::BORDER_SUBTLE()
        };
        (fill, outline, theme::FG())
    }
}

/// The height of one row of a proposal, from the parts that it draws.
fn row_height(
    caption: &std::sync::Arc<egui::Galley>,
    items: &[ItemGalleys],
    state_line: Option<&std::sync::Arc<egui::Galley>>,
) -> f32 {
    let mut height = 2.0 * PADDING + caption.size().y + ITEM_GAP;
    for item in items {
        height += item.height();
    }
    match state_line {
        Some(line) => height + line.size().y,
        None => height + CONTROLS_GAP + BUTTON_HEIGHT,
    }
}

/// One line of the folder of an item, in the monospace family, cut at its left so
/// the end of the path shows: the person reads which folder the work item runs in,
/// and the end names it.
///
/// The end of the path is the part that the row keeps, so the cut measures the
/// suffixes of the folder from the shortest that fits, in a binary search: the
/// layout of a text is cached while the text and its font stay the same, so a
/// row that draws again lays out no new galley.
fn folder_line(ui: &Ui, folder: &str, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let width = |text: &str| {
        text::mono_line(ui, text, FOLDER_SIZE, theme::FG_DIM(), f32::INFINITY)
            .size()
            .x
    };
    let shown = if width(folder) <= max_width {
        folder.to_owned()
    } else {
        shortest_suffix_that_fits(folder, max_width, &width)
    };
    text::mono_line(ui, &shown, FOLDER_SIZE, theme::FG_DIM(), max_width)
}

/// The longest end of `text` whose line fits in `max_width`, with an ellipsis in
/// front of it, found in a binary search over the characters that the end keeps.
fn shortest_suffix_that_fits(text: &str, max_width: f32, width: &dyn Fn(&str) -> f32) -> String {
    let characters: Vec<char> = text.chars().collect();
    let (mut low, mut high, mut best) = (1_usize, characters.len(), 0_usize);
    while low <= high {
        let middle = low + (high - low) / 2;
        if width(&with_ellipsis(&characters, middle)) <= max_width {
            best = middle;
            low = middle + 1;
        } else {
            high = middle - 1;
        }
    }
    with_ellipsis(&characters, best.max(1))
}

/// The ellipsis and the last `keep` characters of `characters`.
fn with_ellipsis(characters: &[char], keep: usize) -> String {
    let start = characters.len().saturating_sub(keep);
    let mut shown = String::from("\u{2026}");
    shown.extend(&characters[start..]);
    shown
}

// The check of the folders of a proposal runs on the worker of the dock, after the
// press of the person and never on a frame.

use super::OrchestratorDock;

impl OrchestratorDock {
    /// Starts the check of the folders of the proposal `id` on the worker of the
    /// dock, so no frame reads a directory. It answers whether a worker took the
    /// job.
    pub(in crate::app) fn begin_proposal_start(
        &mut self,
        id: &str,
        items: &[horizon_core::agent_bridge::ProposalItem],
    ) -> bool {
        let items = items.to_vec();
        let handed_id = id.to_owned();
        let applied_id = id.to_owned();
        self.proposal_start = Some(Starting::Checking(id.to_owned()));
        let handed = self.worker.then(
            move || check_folders(&items),
            move |dock, found| dock.apply_proposal_check(&applied_id, found),
        );
        if !handed {
            // No worker took the check, so nothing was read: the proposal stays
            // open, and the person reads that the work items did not start.
            self.proposal_start = None;
            let _ = handed_id;
        }
        handed
    }

    /// Keeps the folders that the worker checked, which the frame of this pass
    /// starts the work items from.
    fn apply_proposal_check(&mut self, id: &str, found: Option<Vec<bool>>) {
        self.proposal_start = found.map(|found| Starting::Ready {
            id: id.to_owned(),
            found,
        });
    }

    /// Takes the check of the folders that the worker finished: the proposal and
    /// the answer for each of its items.
    pub(in crate::app) fn take_proposal_check(&mut self) -> Option<(String, Vec<bool>)> {
        match self.proposal_start.take() {
            Some(Starting::Ready { id, found }) => Some((id, found)),
            Some(checking) => {
                self.proposal_start = Some(checking);
                None
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Choice, START_ONE, bridge_line, check_folders, shortest_suffix_that_fits, start_label, started_notice,
        state_line, with_ellipsis,
    };
    use horizon_core::agent_bridge::{ProposalItem, ProposalState};

    fn item(folder: Option<&str>) -> ProposalItem {
        ProposalItem {
            title: "Fix login".to_owned(),
            task: "Fix issue 1987".to_owned(),
            folder: folder.map(str::to_owned),
        }
    }

    #[test]
    fn the_control_of_a_proposal_names_how_many_work_items_it_starts() {
        assert_eq!(start_label(1), START_ONE);
        assert_eq!(start_label(2), "Start 2 work items");
        assert_eq!(start_label(6), "Start 6 work items");
    }

    #[test]
    fn the_state_of_a_proposal_that_is_no_longer_open_takes_no_control() {
        assert_eq!(state_line(ProposalState::Open), None);
        assert_eq!(state_line(ProposalState::Started), Some("Started"));
        assert_eq!(state_line(ProposalState::Declined), Some("Not started"));
        assert_eq!(
            state_line(ProposalState::Replaced),
            Some("Replaced by a newer proposal")
        );
    }

    #[test]
    fn the_notice_of_a_press_counts_the_work_items() {
        assert_eq!(started_notice(1), "You started 1 work item");
        assert_eq!(started_notice(2), "You started 2 work items");
    }

    #[test]
    fn the_line_of_the_program_names_the_proposal_and_the_press() {
        assert_eq!(
            bridge_line("p-1", Choice::Start),
            "The person started the work items of proposal p-1."
        );
        assert_eq!(
            bridge_line("p-1", Choice::Decline),
            "The person did not start the work items of proposal p-1."
        );
    }

    #[test]
    fn the_check_of_a_folder_answers_false_for_an_item_without_one() {
        let folder = tempfile::tempdir().expect("a folder");
        let path = folder.path().display().to_string();
        let missing = format!("{path}/not-there");
        let found = check_folders(&[item(Some(&path)), item(Some(&missing)), item(None)]);
        assert_eq!(found, vec![true, false, false]);
    }

    #[test]
    fn the_end_of_a_long_folder_shows() {
        let folder = "/home/person/a/very/long/folder/work";
        // A line of 12 points holds one character of the monospace font of a test,
        // so the search keeps one character and the ellipsis in front of it.
        let width = |text: &str| f32::from(u16::try_from(text.chars().count()).unwrap_or(u16::MAX));
        let shown = shortest_suffix_that_fits(folder, 12.0, &width);
        assert!(shown.starts_with('\u{2026}'), "{shown}");
        assert!(shown.ends_with("work"), "the end of the path shows: {shown}");
        assert!(shown.chars().count() <= 12, "{shown}");
        assert_eq!(with_ellipsis(&['a', 'b', 'c'], 3), "\u{2026}abc");
        assert_eq!(with_ellipsis(&['a', 'b', 'c'], 9), "\u{2026}abc");
    }
}
